use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::pricing::PriceTable;
use crate::rpc::{
    CostRollup, CostRollupQuery, CostSource, HarnessName, ModelName, RollupDay, RollupQuery,
    TimestampMillis, TokenRollup, ToolCallRollup, ToolCallRollupQuery, ToolName,
};
use crate::usage::{
    CostUsageRecord, RepoBucket, RepoIdentity, RepoName, RepoPath, TokenCount, TokenMeasure,
    TokenUsageRecord, ToolCallRecord, UsageRecords,
};

const CURRENT_SCHEMA_VERSION: i64 = 4;
const REPO_BUCKET: &str = "repo";
const NO_REPO_BUCKET: &str = "no_repo";
const NO_REPO_STORAGE_VALUE: &str = "";
const NATIVE_COST_SOURCE: i64 = 1;
const ESTIMATED_COST_SOURCE: i64 = 2;
const MIXED_COST_SOURCE: i64 = 3;
const MILLIS_PER_DAY: i64 = 86_400_000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite failed")]
    Sqlite(#[from] rusqlite::Error),
    #[error("sqlite schema version {found} is newer than supported version {supported}")]
    IncompatibleSchema { found: i64, supported: i64 },
}

#[derive(Debug, thiserror::Error)]
pub enum StoreKeyError {
    #[error("store key must be 64 hex characters")]
    InvalidHexLength,
    #[error("store key contains non-hex character")]
    InvalidHexCharacter,
    #[error("store key generation failed")]
    Random,
}

pub struct UsageStore {
    connection: Connection,
    price_table: PriceTable,
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct StoreKey {
    bytes: [u8; 32],
}

impl std::fmt::Debug for StoreKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StoreKey(<redacted>)")
    }
}

impl StoreKey {
    pub fn generate() -> Result<Self, StoreKeyError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| StoreKeyError::Random)?;
        Ok(Self { bytes })
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    pub fn from_hex(encoded: &str) -> Result<Self, StoreKeyError> {
        if encoded.len() != 64 {
            return Err(StoreKeyError::InvalidHexLength);
        }

        let mut bytes = [0_u8; 32];
        for (index, chunk) in encoded.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (hex_to_nibble(chunk[0])? << 4) | hex_to_nibble(chunk[1])?;
        }
        Ok(Self { bytes })
    }

    pub fn to_hex_secret(&self) -> Zeroizing<String> {
        let mut encoded = String::with_capacity(self.bytes.len() * 2);
        for byte in self.bytes {
            encoded.push(nibble_to_hex(byte >> 4));
            encoded.push(nibble_to_hex(byte & 0x0f));
        }
        Zeroizing::new(encoded)
    }

    fn sqlcipher_raw_key(&self) -> Zeroizing<String> {
        self.to_hex_secret()
    }

    #[cfg(test)]
    fn from_bytes_for_test(bytes: [u8; 32]) -> Self {
        Self::from_bytes(bytes)
    }
}

impl UsageStore {
    pub fn open(path: impl AsRef<Path>, key: &StoreKey) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        apply_database_key(&connection, key)?;
        let mut store = Self {
            connection,
            price_table: PriceTable::bundled_defaults(),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_with_price_table(
        path: impl AsRef<Path>,
        key: &StoreKey,
        price_table: PriceTable,
    ) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        apply_database_key(&connection, key)?;
        let mut store = Self {
            connection,
            price_table,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn ingest_token_usage(&mut self, records: &[TokenUsageRecord]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        let token_deltas = Self::ingest_token_usage_in_transaction(&transaction, records)?;
        Self::ingest_estimated_cost_usage_in_transaction(
            &transaction,
            &self.price_table,
            &token_deltas,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn ingest_usage(&mut self, records: &UsageRecords) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        let token_deltas =
            Self::ingest_token_usage_in_transaction(&transaction, &records.token_usage)?;
        Self::ingest_estimated_cost_usage_in_transaction(
            &transaction,
            &self.price_table,
            &token_deltas,
        )?;
        Self::ingest_cost_usage_in_transaction(&transaction, &records.cost_usage)?;
        Self::ingest_tool_calls_in_transaction(&transaction, &records.tool_calls)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn token_rollups(&self, query: RollupQuery) -> Result<Vec<TokenRollup>, StoreError> {
        let repo_filter = query.repo.as_ref().map(StoredRepo::from_bucket);
        let repo_bucket_filter = repo_filter.as_ref().map(|repo| repo.bucket);
        let repo_name_filter = repo_filter.as_ref().map(|repo| repo.name);
        let repo_path_filter = repo_filter.as_ref().map(|repo| repo.path);
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                model,
                SUM(CASE WHEN measure = 'input' THEN token_count ELSE 0 END) AS input_tokens,
                SUM(CASE WHEN measure = 'output' THEN token_count ELSE 0 END) AS output_tokens,
                SUM(CASE WHEN measure = 'cache' THEN token_count ELSE 0 END) AS cache_tokens
             FROM canonical_token_usage
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
                AND (?3 IS NULL OR repo_name = ?3)
                AND (?4 IS NULL OR repo_path = ?4)
                AND (?5 IS NULL OR repo_bucket = ?5)
             GROUP BY day, repo_bucket, repo_name, repo_path, model
             ORDER BY day, repo_bucket, repo_name, repo_path, model",
        )?;
        let rows = statement.query_map(
            params![
                query.start.value(),
                query.end.value(),
                repo_name_filter,
                repo_path_filter,
                repo_bucket_filter,
            ],
            |row| {
                let day: String = row.get(0)?;
                let repo_bucket: String = row.get(1)?;
                let repo_name: String = row.get(2)?;
                let repo_path: String = row.get(3)?;
                let model: String = row.get(4)?;
                Ok(TokenRollup {
                    day: RollupDay::parse(&day).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
                    repo: repo_bucket_from_storage(&repo_bucket, repo_name, repo_path),
                    model: ModelName::new(model),
                    input_tokens: unsigned_token_column(row, 5)?,
                    output_tokens: unsigned_token_column(row, 6)?,
                    cache_tokens: unsigned_token_column(row, 7)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn persisted_daily_token_rollups(&self) -> Result<Vec<TokenRollup>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT day, repo_bucket, repo_name, repo_path, model, input_tokens, output_tokens, cache_tokens
             FROM token_rollups
             ORDER BY day, repo_bucket, repo_name, repo_path, model",
        )?;
        let rows = statement.query_map([], |row| {
            let day: String = row.get(0)?;
            let repo_bucket: String = row.get(1)?;
            let repo_name: String = row.get(2)?;
            let repo_path: String = row.get(3)?;
            let model: String = row.get(4)?;
            Ok(TokenRollup {
                day: RollupDay::parse(&day).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                repo: repo_bucket_from_storage(&repo_bucket, repo_name, repo_path),
                model: ModelName::new(model),
                input_tokens: unsigned_token_column(row, 5)?,
                output_tokens: unsigned_token_column(row, 6)?,
                cache_tokens: unsigned_token_column(row, 7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn cost_rollups(&self, query: CostRollupQuery) -> Result<Vec<CostRollup>, StoreError> {
        let repo_filter = query.repo.as_ref().map(StoredRepo::from_bucket);
        let repo_bucket_filter = repo_filter.as_ref().map(|repo| repo.bucket);
        let repo_name_filter = repo_filter.as_ref().map(|repo| repo.name);
        let repo_path_filter = repo_filter.as_ref().map(|repo| repo.path);
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                model,
                SUM(cost_usd_nanos) AS cost_usd_nanos,
                CASE
                    WHEN MIN(cost_source) = MAX(cost_source) THEN MIN(cost_source)
                    ELSE ?6
                END AS cost_source
             FROM canonical_cost_usage
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
                AND (?3 IS NULL OR repo_name = ?3)
                AND (?4 IS NULL OR repo_path = ?4)
                AND (?5 IS NULL OR repo_bucket = ?5)
             GROUP BY day, repo_bucket, repo_name, repo_path, model
             ORDER BY day, repo_bucket, repo_name, repo_path, model",
        )?;
        let rows = statement.query_map(
            params![
                query.start.value(),
                query.end.value(),
                repo_name_filter,
                repo_path_filter,
                repo_bucket_filter,
                MIXED_COST_SOURCE,
            ],
            |row| {
                let day: String = row.get(0)?;
                let repo_bucket: String = row.get(1)?;
                let repo_name: String = row.get(2)?;
                let repo_path: String = row.get(3)?;
                let model: String = row.get(4)?;
                Ok(CostRollup {
                    day: RollupDay::parse(&day).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
                    repo: repo_bucket_from_storage(&repo_bucket, repo_name, repo_path),
                    model: ModelName::new(model),
                    cost_usd: cost_column(row, 5)?,
                    source: cost_source_column(row, 6)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn tool_call_rollups(
        &self,
        query: ToolCallRollupQuery,
    ) -> Result<Vec<ToolCallRollup>, StoreError> {
        if is_day_aligned(query.start) && is_day_aligned(query.end) {
            return self.persisted_tool_call_rollups(query);
        }
        self.canonical_tool_call_rollups(query)
    }

    fn persisted_tool_call_rollups(
        &self,
        query: ToolCallRollupQuery,
    ) -> Result<Vec<ToolCallRollup>, StoreError> {
        let repo_filter = query.repo.as_ref().map(StoredRepo::from_bucket);
        let repo_bucket_filter = repo_filter.as_ref().map(|repo| repo.bucket);
        let repo_name_filter = repo_filter.as_ref().map(|repo| repo.name);
        let repo_path_filter = repo_filter.as_ref().map(|repo| repo.path);
        let start_day = query.start.day().as_date().to_string();
        let end_day = query.end.day().as_date().to_string();
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                call_count
             FROM tool_call_rollups
             WHERE day >= ?1 AND day < ?2
                AND (?3 IS NULL OR repo_name = ?3)
                AND (?4 IS NULL OR repo_path = ?4)
                AND (?5 IS NULL OR repo_bucket = ?5)
             ORDER BY day, repo_bucket, repo_name, repo_path, harness, tool_name",
        )?;
        let rows = statement.query_map(
            params![
                start_day,
                end_day,
                repo_name_filter,
                repo_path_filter,
                repo_bucket_filter,
            ],
            |row| {
                let day: String = row.get(0)?;
                let repo_bucket: String = row.get(1)?;
                let repo_name: String = row.get(2)?;
                let repo_path: String = row.get(3)?;
                let harness: String = row.get(4)?;
                let tool_name: String = row.get(5)?;
                Ok(ToolCallRollup {
                    day: RollupDay::parse(&day).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
                    repo: repo_bucket_from_storage(&repo_bucket, repo_name, repo_path),
                    harness: HarnessName::new(harness),
                    tool_name: ToolName::new(tool_name),
                    call_count: unsigned_token_column(row, 6)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn canonical_tool_call_rollups(
        &self,
        query: ToolCallRollupQuery,
    ) -> Result<Vec<ToolCallRollup>, StoreError> {
        let repo_filter = query.repo.as_ref().map(StoredRepo::from_bucket);
        let repo_bucket_filter = repo_filter.as_ref().map(|repo| repo.bucket);
        let repo_name_filter = repo_filter.as_ref().map(|repo| repo.name);
        let repo_path_filter = repo_filter.as_ref().map(|repo| repo.path);
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                SUM(call_count) AS call_count
             FROM canonical_tool_calls
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
                AND (?3 IS NULL OR repo_name = ?3)
                AND (?4 IS NULL OR repo_path = ?4)
                AND (?5 IS NULL OR repo_bucket = ?5)
             GROUP BY day, repo_bucket, repo_name, repo_path, harness, tool_name
             ORDER BY day, repo_bucket, repo_name, repo_path, harness, tool_name",
        )?;
        let rows = statement.query_map(
            params![
                query.start.value(),
                query.end.value(),
                repo_name_filter,
                repo_path_filter,
                repo_bucket_filter,
            ],
            |row| {
                let day: String = row.get(0)?;
                let repo_bucket: String = row.get(1)?;
                let repo_name: String = row.get(2)?;
                let repo_path: String = row.get(3)?;
                let harness: String = row.get(4)?;
                let tool_name: String = row.get(5)?;
                Ok(ToolCallRollup {
                    day: RollupDay::parse(&day).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
                    repo: repo_bucket_from_storage(&repo_bucket, repo_name, repo_path),
                    harness: HarnessName::new(harness),
                    tool_name: ToolName::new(tool_name),
                    call_count: unsigned_token_column(row, 6)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let schema_version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;

        if schema_version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::IncompatibleSchema {
                found: schema_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        let transaction = self.connection.transaction()?;
        match schema_version {
            CURRENT_SCHEMA_VERSION => {}
            3 => migrate_v3_to_v4(&transaction)?,
            2 => migrate_v2_to_v4(&transaction)?,
            _ if schema_version < 2 => drop_incompatible_usage_tables(&transaction)?,
            _ => {}
        }

        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cumulative_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, measure, counter_start_ms)
            );

            CREATE TABLE IF NOT EXISTS canonical_cost_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                counter_start_ms INTEGER,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                cost_usd_nanos INTEGER NOT NULL,
                cost_source INTEGER NOT NULL,
                estimated_token_count INTEGER,
                estimated_token_price_nanos INTEGER
            );

            CREATE TABLE IF NOT EXISTS cost_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, counter_start_ms)
            );

            CREATE TABLE IF NOT EXISTS token_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            CREATE TABLE IF NOT EXISTS cost_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                native_cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                estimated_cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            CREATE TABLE IF NOT EXISTS canonical_tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                call_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tool_call_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                call_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, harness, tool_name)
            );",
        )?;
        transaction.execute(
            "CREATE INDEX IF NOT EXISTS canonical_cost_usage_native_lookup
             ON canonical_cost_usage (
                repo_bucket,
                repo_name,
                repo_path,
                model,
                counter_start_ms,
                cost_source,
                occurred_at_ms
             )",
            [],
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    fn ingest_token_usage_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[TokenUsageRecord],
    ) -> Result<Vec<TokenUsageDelta>, StoreError> {
        let mut token_deltas = Vec::new();
        for record in records {
            let Some(delta) = Self::counter_delta(transaction, record)? else {
                continue;
            };
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            transaction.execute(
                "INSERT INTO canonical_token_usage (
                    occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, measure, token_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    record.occurred_at.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    record.measure.storage_name(),
                    delta,
                ],
            )?;

            let (input_tokens, output_tokens, cache_tokens) = match record.measure {
                TokenMeasure::Input => (delta, 0, 0),
                TokenMeasure::Output => (0, delta, 0),
                TokenMeasure::Cache => (0, 0, delta),
            };
            transaction.execute(
                "INSERT INTO token_rollups (
                    day, repo_bucket, repo_name, repo_path, model, input_tokens, output_tokens, cache_tokens
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, model) DO UPDATE SET
                    input_tokens = input_tokens + excluded.input_tokens,
                    output_tokens = output_tokens + excluded.output_tokens,
                    cache_tokens = cache_tokens + excluded.cache_tokens",
                params![
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    input_tokens,
                    output_tokens,
                    cache_tokens
                ],
            )?;
            token_deltas.push(TokenUsageDelta {
                occurred_at: record.occurred_at,
                counter_start: record.counter_start,
                repo: record.repo.clone(),
                model: record.model.clone(),
                measure: record.measure,
                token_count: TokenCount::new(u64::try_from(delta).expect("delta is positive")),
            });
        }
        Ok(token_deltas)
    }

    fn ingest_cost_usage_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[CostUsageRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            let Some(delta) = Self::cost_counter_delta(transaction, record)? else {
                continue;
            };
            Self::remove_estimated_cost_for_native_record(transaction, record)?;
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            transaction.execute(
                "INSERT INTO canonical_cost_usage (
                    occurred_at_ms, counter_start_ms, day, repo_bucket, repo_name, repo_path, model,
                    cost_usd_nanos, cost_source, estimated_token_count, estimated_token_price_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)",
                params![
                    record.occurred_at.value(),
                    record.counter_start.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    delta,
                    NATIVE_COST_SOURCE,
                ],
            )?;

            transaction.execute(
                "INSERT INTO cost_rollups (
                    day, repo_bucket, repo_name, repo_path, model, native_cost_usd_nanos, estimated_cost_usd_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, model) DO UPDATE SET
                    native_cost_usd_nanos = native_cost_usd_nanos + excluded.native_cost_usd_nanos",
                params![
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    delta,
                ],
            )?;
        }
        Ok(())
    }

    fn ingest_tool_calls_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[ToolCallRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO canonical_tool_calls (
                    event_key, occurred_at_ms, day, repo_bucket, repo_name, repo_path, harness, tool_name, call_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
                params![
                    record.event_key.as_str(),
                    record.occurred_at.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.harness.as_str(),
                    record.tool_name.as_str(),
                ],
            )?;
            if inserted == 0 {
                continue;
            }

            transaction.execute(
                "INSERT INTO tool_call_rollups (
                    day, repo_bucket, repo_name, repo_path, harness, tool_name, call_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, harness, tool_name) DO UPDATE SET
                    call_count = call_count + excluded.call_count",
                params![
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.harness.as_str(),
                    record.tool_name.as_str(),
                ],
            )?;
        }
        Ok(())
    }

    fn remove_estimated_cost_for_native_record(
        transaction: &rusqlite::Transaction<'_>,
        record: &CostUsageRecord,
    ) -> Result<(), StoreError> {
        let stored_repo = StoredRepo::from_bucket(&record.repo);
        let mut statement = transaction.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                model,
                SUM(cost_usd_nanos) AS cost_usd_nanos
             FROM canonical_cost_usage
             WHERE repo_bucket = ?1
                AND repo_name = ?2
                AND repo_path = ?3
                AND model = ?4
                AND (counter_start_ms = ?5 OR counter_start_ms IS NULL)
                AND occurred_at_ms <= ?6
                AND cost_source = ?7
             GROUP BY day, repo_bucket, repo_name, repo_path, model",
        )?;
        let estimated_rollups = statement
            .query_map(
                params![
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    record.counter_start.value(),
                    record.occurred_at.value(),
                    ESTIMATED_COST_SOURCE,
                ],
                |row| {
                    Ok(EstimatedCostRollup {
                        day: row.get(0)?,
                        repo_bucket: row.get(1)?,
                        repo_name: row.get(2)?,
                        repo_path: row.get(3)?,
                        model: row.get(4)?,
                        cost_usd_nanos: row.get(5)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        for rollup in estimated_rollups {
            transaction.execute(
                "UPDATE cost_rollups
                 SET estimated_cost_usd_nanos = estimated_cost_usd_nanos - ?1
                 WHERE day = ?2
                    AND repo_bucket = ?3
                    AND repo_name = ?4
                    AND repo_path = ?5
                    AND model = ?6",
                params![
                    rollup.cost_usd_nanos,
                    rollup.day,
                    rollup.repo_bucket,
                    rollup.repo_name,
                    rollup.repo_path,
                    rollup.model,
                ],
            )?;
        }

        transaction.execute(
            "DELETE FROM canonical_cost_usage
             WHERE repo_bucket = ?1
                AND repo_name = ?2
                AND repo_path = ?3
                AND model = ?4
                AND (counter_start_ms = ?5 OR counter_start_ms IS NULL)
                AND occurred_at_ms <= ?6
                AND cost_source = ?7",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.model.as_str(),
                record.counter_start.value(),
                record.occurred_at.value(),
                ESTIMATED_COST_SOURCE,
            ],
        )?;
        Ok(())
    }

    fn ingest_estimated_cost_usage_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        price_table: &PriceTable,
        token_deltas: &[TokenUsageDelta],
    ) -> Result<(), StoreError> {
        let native_cost_coverage = NativeCostCoverage::load(transaction, token_deltas)?;
        for delta in token_deltas {
            if native_cost_coverage.covers(delta) {
                continue;
            }
            let Some(model_prices) = price_table.price_for(&delta.model) else {
                continue;
            };
            let token_price = model_prices.price_for_measure(delta.measure);
            let Some(cost) = token_price.checked_mul(delta.token_count.value()) else {
                continue;
            };
            let day = delta.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&delta.repo);
            transaction.execute(
                "INSERT INTO canonical_cost_usage (
                    occurred_at_ms, counter_start_ms, day, repo_bucket, repo_name, repo_path, model,
                    cost_usd_nanos, cost_source, estimated_token_count, estimated_token_price_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    delta.occurred_at.value(),
                    delta.counter_start.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    delta.model.as_str(),
                    cost.storage_value(),
                    ESTIMATED_COST_SOURCE,
                    delta.token_count.storage_value(),
                    token_price.storage_value(),
                ],
            )?;

            transaction.execute(
                "INSERT INTO cost_rollups (
                    day, repo_bucket, repo_name, repo_path, model, native_cost_usd_nanos, estimated_cost_usd_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, model) DO UPDATE SET
                    estimated_cost_usd_nanos = estimated_cost_usd_nanos + excluded.estimated_cost_usd_nanos",
                params![
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    delta.model.as_str(),
                    cost.storage_value(),
                ],
            )?;
        }
        Ok(())
    }

    fn counter_delta(
        transaction: &rusqlite::Transaction<'_>,
        record: &TokenUsageRecord,
    ) -> Result<Option<i64>, StoreError> {
        let current_value = record.token_count.storage_value();
        let stored_repo = StoredRepo::from_bucket(&record.repo);
        let previous_value = transaction.query_row(
            "SELECT last_occurred_at_ms, last_value
             FROM cumulative_counter_snapshots
             WHERE repo_bucket = ?1 AND repo_name = ?2 AND repo_path = ?3 AND model = ?4 AND measure = ?5 AND counter_start_ms = ?6",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.model.as_str(),
                record.measure.storage_name(),
                record.counter_start.value(),
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        );

        let delta = match previous_value {
            Ok((last_occurred_at_ms, _previous_value))
                if record.occurred_at.value() <= last_occurred_at_ms =>
            {
                return Ok(None);
            }
            Ok((_last_occurred_at_ms, previous_value)) if current_value > previous_value => {
                current_value - previous_value
            }
            Ok((_last_occurred_at_ms, previous_value)) if current_value == previous_value => 0,
            Ok((_last_occurred_at_ms, _reset_value)) => current_value,
            Err(rusqlite::Error::QueryReturnedNoRows) => current_value,
            Err(err) => return Err(StoreError::Sqlite(err)),
        };

        transaction.execute(
            "INSERT INTO cumulative_counter_snapshots (
                repo_bucket, repo_name, repo_path, model, measure, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(repo_bucket, repo_name, repo_path, model, measure, counter_start_ms) DO UPDATE SET
                last_occurred_at_ms = excluded.last_occurred_at_ms,
                last_value = excluded.last_value",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.model.as_str(),
                record.measure.storage_name(),
                record.counter_start.value(),
                record.occurred_at.value(),
                current_value,
            ],
        )?;

        if delta == 0 {
            Ok(None)
        } else {
            Ok(Some(delta))
        }
    }

    fn cost_counter_delta(
        transaction: &rusqlite::Transaction<'_>,
        record: &CostUsageRecord,
    ) -> Result<Option<i64>, StoreError> {
        let current_value = record.cost_usd.storage_value();
        let stored_repo = StoredRepo::from_bucket(&record.repo);
        let previous_value = transaction.query_row(
            "SELECT last_occurred_at_ms, last_value
             FROM cost_counter_snapshots
             WHERE repo_bucket = ?1 AND repo_name = ?2 AND repo_path = ?3 AND model = ?4 AND counter_start_ms = ?5",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.model.as_str(),
                record.counter_start.value(),
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        );

        let delta = match previous_value {
            Ok((last_occurred_at_ms, _previous_value))
                if record.occurred_at.value() <= last_occurred_at_ms =>
            {
                return Ok(None);
            }
            Ok((_last_occurred_at_ms, previous_value)) if current_value > previous_value => {
                current_value - previous_value
            }
            Ok((_last_occurred_at_ms, previous_value)) if current_value == previous_value => 0,
            Ok((_last_occurred_at_ms, _reset_value)) => current_value,
            Err(rusqlite::Error::QueryReturnedNoRows) => current_value,
            Err(err) => return Err(StoreError::Sqlite(err)),
        };

        transaction.execute(
            "INSERT INTO cost_counter_snapshots (
                repo_bucket, repo_name, repo_path, model, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(repo_bucket, repo_name, repo_path, model, counter_start_ms) DO UPDATE SET
                last_occurred_at_ms = excluded.last_occurred_at_ms,
                last_value = excluded.last_value",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.model.as_str(),
                record.counter_start.value(),
                record.occurred_at.value(),
                current_value,
            ],
        )?;

        if delta == 0 {
            Ok(None)
        } else {
            Ok(Some(delta))
        }
    }
}

fn drop_incompatible_usage_tables(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StoreError> {
    transaction.execute_batch(
        "DROP TABLE IF EXISTS canonical_token_usage;
        DROP TABLE IF EXISTS canonical_cost_usage;
        DROP TABLE IF EXISTS canonical_tool_calls;
        DROP TABLE IF EXISTS cumulative_counter_snapshots;
        DROP TABLE IF EXISTS cost_counter_snapshots;
        DROP TABLE IF EXISTS token_rollups;
        DROP TABLE IF EXISTS cost_rollups;
        DROP TABLE IF EXISTS tool_call_rollups;",
    )?;
    Ok(())
}

fn migrate_v2_to_v4(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "ALTER TABLE canonical_cost_usage ADD COLUMN counter_start_ms INTEGER;
        ALTER TABLE canonical_cost_usage ADD COLUMN cost_source INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE canonical_cost_usage ADD COLUMN estimated_token_count INTEGER;
        ALTER TABLE canonical_cost_usage ADD COLUMN estimated_token_price_nanos INTEGER;
        ALTER TABLE cost_rollups ADD COLUMN native_cost_usd_nanos INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE cost_rollups ADD COLUMN estimated_cost_usd_nanos INTEGER NOT NULL DEFAULT 0;
        UPDATE cost_rollups SET native_cost_usd_nanos = cost_usd_nanos;",
    )?;
    Ok(())
}

fn migrate_v3_to_v4(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if cost_source_column_type(transaction)? != Some("TEXT".to_owned()) {
        return Ok(());
    }

    transaction.execute_batch(
        "ALTER TABLE canonical_cost_usage RENAME TO canonical_cost_usage_v3_text_source;

        CREATE TABLE canonical_cost_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            occurred_at_ms INTEGER NOT NULL,
            counter_start_ms INTEGER,
            day TEXT NOT NULL,
            repo_bucket TEXT NOT NULL,
            repo_name TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            model TEXT NOT NULL,
            cost_usd_nanos INTEGER NOT NULL,
            cost_source INTEGER NOT NULL,
            estimated_token_count INTEGER,
            estimated_token_price_nanos INTEGER
        );

        INSERT INTO canonical_cost_usage (
            id,
            occurred_at_ms,
            counter_start_ms,
            day,
            repo_bucket,
            repo_name,
            repo_path,
            model,
            cost_usd_nanos,
            cost_source,
            estimated_token_count,
            estimated_token_price_nanos
        )
        SELECT
            id,
            occurred_at_ms,
            counter_start_ms,
            day,
            repo_bucket,
            repo_name,
            repo_path,
            model,
            cost_usd_nanos,
            CASE cost_source
                WHEN 'native' THEN 1
                WHEN 'estimated' THEN 2
                WHEN 'mixed' THEN 3
            END,
            estimated_token_count,
            estimated_token_price_nanos
        FROM canonical_cost_usage_v3_text_source
        WHERE cost_source IN ('native', 'estimated', 'mixed');

        DROP TABLE canonical_cost_usage_v3_text_source;",
    )?;
    Ok(())
}

fn cost_source_column_type(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Option<String>, StoreError> {
    let mut statement = transaction.prepare("PRAGMA table_info(canonical_cost_usage)")?;
    let columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns
        .into_iter()
        .find_map(|(name, column_type)| (name == "cost_source").then_some(column_type)))
}

fn apply_database_key(connection: &Connection, key: &StoreKey) -> Result<(), StoreError> {
    let raw_key = key.sqlcipher_raw_key();
    let sql = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", raw_key.as_str()));
    connection.execute_batch(&sql)?;
    Ok(())
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
}

fn hex_to_nibble(character: u8) -> Result<u8, StoreKeyError> {
    match character {
        b'0'..=b'9' => Ok(character - b'0'),
        b'a'..=b'f' => Ok(character - b'a' + 10),
        b'A'..=b'F' => Ok(character - b'A' + 10),
        _ => Err(StoreKeyError::InvalidHexCharacter),
    }
}

struct StoredRepo<'a> {
    bucket: &'static str,
    name: &'a str,
    path: &'a str,
}

struct TokenUsageDelta {
    occurred_at: TimestampMillis,
    counter_start: TimestampMillis,
    repo: RepoBucket,
    model: ModelName,
    measure: TokenMeasure,
    token_count: TokenCount,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct NativeCostKey {
    repo_bucket: &'static str,
    repo_name: String,
    repo_path: String,
    model: String,
    counter_start_ms: i64,
}

struct NativeCostCoverage {
    latest_occurred_at_by_key: HashMap<NativeCostKey, i64>,
}

struct EstimatedCostRollup {
    day: String,
    repo_bucket: String,
    repo_name: String,
    repo_path: String,
    model: String,
    cost_usd_nanos: i64,
}

impl NativeCostCoverage {
    fn load(
        transaction: &rusqlite::Transaction<'_>,
        token_deltas: &[TokenUsageDelta],
    ) -> Result<Self, StoreError> {
        let mut keys = HashSet::new();
        for delta in token_deltas {
            keys.insert(NativeCostKey::from_delta(delta));
        }

        let mut statement = transaction.prepare(
            "SELECT MAX(occurred_at_ms)
             FROM canonical_cost_usage
             WHERE repo_bucket = ?1
                AND repo_name = ?2
                AND repo_path = ?3
                AND model = ?4
                AND (counter_start_ms = ?5 OR counter_start_ms IS NULL)
                AND cost_source = ?6",
        )?;
        let mut latest_occurred_at_by_key = HashMap::new();
        for key in keys {
            let latest_occurred_at_ms = statement.query_row(
                params![
                    key.repo_bucket,
                    key.repo_name.as_str(),
                    key.repo_path.as_str(),
                    key.model.as_str(),
                    key.counter_start_ms,
                    NATIVE_COST_SOURCE,
                ],
                |row| row.get::<_, Option<i64>>(0),
            )?;
            if let Some(latest_occurred_at_ms) = latest_occurred_at_ms {
                latest_occurred_at_by_key.insert(key, latest_occurred_at_ms);
            }
        }

        Ok(Self {
            latest_occurred_at_by_key,
        })
    }

    fn covers(&self, token_delta: &TokenUsageDelta) -> bool {
        let key = NativeCostKey::from_delta(token_delta);
        self.latest_occurred_at_by_key
            .get(&key)
            .is_some_and(|latest_occurred_at_ms| {
                *latest_occurred_at_ms >= token_delta.occurred_at.value()
            })
    }
}

impl NativeCostKey {
    fn from_delta(delta: &TokenUsageDelta) -> Self {
        let stored_repo = StoredRepo::from_bucket(&delta.repo);
        Self {
            repo_bucket: stored_repo.bucket,
            repo_name: stored_repo.name.to_owned(),
            repo_path: stored_repo.path.to_owned(),
            model: delta.model.as_str().to_owned(),
            counter_start_ms: delta.counter_start.value(),
        }
    }
}

impl<'a> StoredRepo<'a> {
    fn from_bucket(repo: &'a RepoBucket) -> Self {
        match repo {
            RepoBucket::NoRepo => Self {
                bucket: NO_REPO_BUCKET,
                name: NO_REPO_STORAGE_VALUE,
                path: NO_REPO_STORAGE_VALUE,
            },
            RepoBucket::Repo(identity) => Self {
                bucket: REPO_BUCKET,
                name: identity
                    .name
                    .as_ref()
                    .map(RepoName::as_str)
                    .unwrap_or(NO_REPO_STORAGE_VALUE),
                path: identity
                    .path
                    .as_ref()
                    .map(RepoPath::as_str)
                    .unwrap_or(NO_REPO_STORAGE_VALUE),
            },
        }
    }
}

fn repo_bucket_from_storage(bucket: &str, name: String, path: String) -> RepoBucket {
    match bucket {
        REPO_BUCKET => RepoIdentity::from_parts(
            non_empty_storage_value(name).map(RepoName::new),
            non_empty_storage_value(path).map(RepoPath::new),
        )
        .map(RepoBucket::repo)
        .unwrap_or_else(RepoBucket::no_repo),
        NO_REPO_BUCKET => RepoBucket::no_repo(),
        _ => RepoBucket::no_repo(),
    }
}

fn non_empty_storage_value(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn unsigned_token_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

fn cost_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<crate::usage::CostUsd> {
    let value = row.get::<_, i64>(index)?;
    crate::usage::CostUsd::from_storage_value(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            "cost must be non-negative and fit storage".into(),
        )
    })
}

fn cost_source_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<CostSource> {
    let value = row.get::<_, i64>(index)?;
    match value {
        NATIVE_COST_SOURCE => Ok(CostSource::Native),
        ESTIMATED_COST_SOURCE => Ok(CostSource::Estimated),
        MIXED_COST_SOURCE => Ok(CostSource::Mixed),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            "cost source must be native, estimated, or mixed".into(),
        )),
    }
}

fn is_day_aligned(timestamp: TimestampMillis) -> bool {
    timestamp.value().rem_euclid(MILLIS_PER_DAY) == 0
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::pricing::ModelTokenPrices;
    use crate::rpc::TimestampMillis;
    use crate::usage::{CostUsd, RepoIdentity, RepoName, RepoPath, TokenCount, ToolCallEventKey};

    #[test]
    fn reopens_encrypted_store_with_the_same_key() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let store_key = StoreKey::from_bytes_for_test([7; 32]);
        let expected = vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo("/not/persisted"),
            model: ModelName::new("claude-opus-4-20250514"),
            input_tokens: 100,
            output_tokens: 0,
            cache_tokens: 0,
        }];

        {
            let mut store = UsageStore::open(&database_path, &store_key)?;
            store.ingest_token_usage(&[usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                100,
            )])?;
            assert_eq!(store.persisted_daily_token_rollups()?, expected);
        }

        let reopened = UsageStore::open(database_path, &store_key)?;

        assert_eq!(reopened.persisted_daily_token_rollups()?, expected);

        Ok(())
    }

    #[test]
    fn encrypted_store_cannot_be_read_without_the_key() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");

        {
            let mut store = open_test_store(&database_path)?;
            store.ingest_token_usage(&[usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                100,
            )])?;
        }

        let connection = Connection::open(database_path)?;
        let result = connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        });

        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn encrypted_store_cannot_be_opened_with_a_different_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let wrong_key = StoreKey::from_bytes_for_test([8; 32]);

        {
            let mut store = open_test_store(&database_path)?;
            store.ingest_token_usage(&[usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                100,
            )])?;
        }

        let result = UsageStore::open(&database_path, &wrong_key);

        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn persists_daily_rollups_from_cumulative_counter_deltas()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_token_usage(&[
            usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            ),
            usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_960_400_000,
                1_781_956_700_000,
                1_100,
            ),
            usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Output,
                1_781_956_800_000,
                1_781_956_700_000,
                500,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Cache,
                1_782_043_200_000,
                1_782_043_100_000,
                50,
            ),
        ])?;

        assert_eq!(
            store.persisted_daily_token_rollups()?,
            vec![
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir_repo("/not/persisted"),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 1100,
                    output_tokens: 500,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-21")?,
                    repo: kvasir_repo("/not/persisted"),
                    model: ModelName::new("claude-sonnet-4-20250514"),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_tokens: 50,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn ignores_out_of_order_cumulative_counter_snapshots() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_token_usage(&[
            usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_960_400_000,
                1_781_956_700_000,
                1_100,
            ),
            usage_record(
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            ),
        ])?;

        let expected = vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo("/not/persisted"),
            model: ModelName::new("claude-opus-4-20250514"),
            input_tokens: 1100,
            output_tokens: 0,
            cache_tokens: 0,
        }];

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            expected
        );
        assert_eq!(store.persisted_daily_token_rollups()?, expected);

        Ok(())
    }

    #[test]
    fn token_rollups_are_grouped_and_filtered_by_repo() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let kvasir = kvasir_repo("/repos/kvasir");
        let other_kvasir = kvasir_repo("/other/kvasir");
        let name_only = RepoBucket::repo(
            RepoIdentity::from_parts(Some(RepoName::new("name-only")), None).unwrap(),
        );
        let path_only = RepoBucket::repo(
            RepoIdentity::from_parts(None, Some(RepoPath::new("/repos/path-only"))).unwrap(),
        );
        let no_repo = RepoBucket::no_repo();

        store.ingest_token_usage(&[
            usage_record_for_repo(
                kvasir.clone(),
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            ),
            usage_record_for_repo(
                other_kvasir.clone(),
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                400,
            ),
            usage_record_for_repo(
                name_only.clone(),
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                150,
            ),
            usage_record_for_repo(
                path_only.clone(),
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                175,
            ),
            usage_record_for_repo(
                no_repo.clone(),
                "claude-opus-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                250,
            ),
        ])?;

        let query = RollupQuery::new(
            TimestampMillis::new_for_test(1_781_956_000_000),
            TimestampMillis::new_for_test(1_781_970_000_000),
        );

        assert_eq!(
            store.token_rollups(query.clone())?,
            vec![
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: no_repo.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 250,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: path_only.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 175,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: other_kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 400,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 1000,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: name_only.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 150,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
            ]
        );

        assert_eq!(
            store.token_rollups(query.with_repo(kvasir.clone()))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir,
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 1000,
                output_tokens: 0,
                cache_tokens: 0,
            }]
        );

        assert_eq!(
            store.token_rollups(
                RollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(name_only.clone())
            )?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: name_only,
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 150,
                output_tokens: 0,
                cache_tokens: 0,
            }]
        );

        Ok(())
    }

    #[test]
    fn cost_rollups_are_grouped_and_filtered_by_repo() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let kvasir = kvasir_repo("/repos/kvasir");
        let other_kvasir = kvasir_repo("/other/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: vec![
                cost_record_for_repo(
                    kvasir.clone(),
                    "claude-opus-4-20250514",
                    1_781_956_800_000,
                    1_781_956_700_000,
                    "1.25",
                ),
                cost_record_for_repo(
                    kvasir.clone(),
                    "claude-opus-4-20250514",
                    1_781_960_400_000,
                    1_781_956_700_000,
                    "1.5",
                ),
                cost_record_for_repo(
                    other_kvasir.clone(),
                    "claude-opus-4-20250514",
                    1_781_956_800_000,
                    1_781_956_700_000,
                    "0.375",
                ),
                cost_record_for_repo(
                    kvasir.clone(),
                    "claude-sonnet-4-20250514",
                    1_781_956_800_000,
                    1_781_956_700_000,
                    "0.2",
                ),
                cost_record_for_repo(
                    kvasir.clone(),
                    "claude-opus-4-20250514",
                    1_782_043_200_000,
                    1_781_956_700_000,
                    "2.0",
                ),
            ],
            tool_calls: Vec::new(),
        })?;

        let query = CostRollupQuery::new(
            TimestampMillis::new_for_test(1_781_956_000_000),
            TimestampMillis::new_for_test(1_782_050_000_000),
        );

        assert_eq!(
            store.cost_rollups(query.clone())?,
            vec![
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: other_kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("0.375"),
                    source: CostSource::Native,
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("1.5"),
                    source: CostSource::Native,
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-sonnet-4-20250514"),
                    cost_usd: cost_usd("0.2"),
                    source: CostSource::Native,
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-21")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("0.5"),
                    source: CostSource::Native,
                },
            ]
        );

        assert_eq!(
            store.cost_rollups(query.with_repo(kvasir.clone()))?,
            vec![
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("1.5"),
                    source: CostSource::Native,
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-sonnet-4-20250514"),
                    cost_usd: cost_usd("0.2"),
                    source: CostSource::Native,
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-21")?,
                    repo: kvasir,
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("0.5"),
                    source: CostSource::Native,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn tool_call_rollups_read_persisted_rollup_table() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: vec![tool_call_record_for_repo(
                repo.clone(),
                "claude_code",
                "Read",
                1_781_956_800_000,
            )],
        })?;
        store
            .connection
            .execute("DELETE FROM canonical_tool_calls", [])?;

        assert_eq!(
            store.tool_call_rollups(ToolCallRollupQuery::new(
                TimestampMillis::new_for_test(1_781_913_600_000),
                TimestampMillis::new_for_test(1_781_913_600_000),
            ))?,
            Vec::<ToolCallRollup>::new()
        );
        assert_eq!(
            store.tool_call_rollups(ToolCallRollupQuery::new(
                TimestampMillis::new_for_test(1_781_913_600_000),
                TimestampMillis::new_for_test(1_782_000_000_000),
            ))?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("claude_code"),
                tool_name: ToolName::new("Read"),
                call_count: 1,
            }]
        );

        Ok(())
    }

    #[test]
    fn tool_call_rollups_preserve_sub_day_time_window_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: vec![
                tool_call_record_for_repo(repo.clone(), "claude_code", "Read", 1_781_956_800_000),
                tool_call_record_for_repo(repo.clone(), "claude_code", "Write", 1_781_960_400_000),
            ],
        })?;

        assert_eq!(
            store.tool_call_rollups(ToolCallRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_800_000),
                TimestampMillis::new_for_test(1_781_956_800_000),
            ))?,
            Vec::<ToolCallRollup>::new()
        );
        assert_eq!(
            store.tool_call_rollups(ToolCallRollupQuery::new(
                TimestampMillis::new_for_test(1_781_960_400_000),
                TimestampMillis::new_for_test(1_781_964_000_000),
            ))?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("claude_code"),
                tool_name: ToolName::new("Write"),
                call_count: 1,
            }]
        );

        Ok(())
    }

    #[test]
    fn computes_estimated_cost_from_priced_token_usage() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Output,
                1_781_956_800_000,
                1_781_956_700_000,
                200,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Cache,
                1_781_956_800_000,
                1_781_956_700_000,
                50,
            ),
        ]))?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: CostUsd::from_nanos(6_015_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn token_only_ingest_api_also_computes_estimated_cost() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let mut store = open_test_store(&database_path)?;

        store.ingest_token_usage(&[usage_record(
            "claude-sonnet-4-20250514",
            TokenMeasure::Input,
            1_781_956_800_000,
            1_781_956_700_000,
            1_000,
        )])?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: CostUsd::from_nanos(3_000_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        drop(store);
        let connection = open_raw_test_connection(database_path)?;
        let (token_count, token_price): (i64, i64) = connection.query_row(
            "SELECT estimated_token_count, estimated_token_price_nanos
             FROM canonical_cost_usage
             WHERE cost_source = ?1",
            params![ESTIMATED_COST_SOURCE],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((token_count, token_price), (1_000, 3_000));

        Ok(())
    }

    #[test]
    fn native_cost_takes_precedence_over_computed_cost_for_the_same_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords {
            token_usage: vec![usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            )],
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_781_956_800_000,
                1_781_956_700_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            }]
        );

        Ok(())
    }

    #[test]
    fn late_native_cost_replaces_covered_estimated_cost() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                1_000,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Output,
                1_781_960_400_000,
                1_781_956_700_000,
                200,
            ),
        ]))?;
        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_781_960_400_000,
                1_781_956_700_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            }]
        );

        Ok(())
    }

    #[test]
    fn late_native_cost_replaces_estimates_across_days_and_preserves_later_estimates()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Input,
                1_782_039_600_000,
                1_782_039_000_000,
                1_000,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Output,
                1_782_043_200_000,
                1_782_039_000_000,
                200,
            ),
            usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Cache,
                1_782_046_800_000,
                1_782_039_000_000,
                50,
            ),
        ]))?;
        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_782_043_200_000,
                1_782_039_000_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_782_039_000_000),
                TimestampMillis::new_for_test(1_782_050_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-21")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: CostUsd::from_nanos(200_015_000).unwrap(),
                source: CostSource::Mixed,
            }]
        );

        Ok(())
    }

    #[test]
    fn native_cost_does_not_suppress_later_uncovered_estimates()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords {
            token_usage: vec![
                usage_record(
                    "claude-sonnet-4-20250514",
                    TokenMeasure::Input,
                    1_781_956_800_000,
                    1_781_956_700_000,
                    1_000,
                ),
                usage_record(
                    "claude-sonnet-4-20250514",
                    TokenMeasure::Output,
                    1_781_960_400_000,
                    1_781_956_700_000,
                    200,
                ),
            ],
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_781_956_800_000,
                1_781_956_700_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.203"),
                source: CostSource::Mixed,
            }]
        );

        Ok(())
    }

    #[test]
    fn zero_delta_native_record_does_not_suppress_estimated_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_781_956_800_000,
                1_781_956_700_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;
        store.ingest_usage(&UsageRecords {
            token_usage: vec![usage_record(
                "claude-sonnet-4-20250514",
                TokenMeasure::Output,
                1_781_960_400_000,
                1_781_956_700_000,
                200,
            )],
            cost_usage: vec![cost_record_for_repo(
                kvasir_repo("/not/persisted"),
                "claude-sonnet-4-20250514",
                1_781_960_400_000,
                1_781_956_700_000,
                "0.2",
            )],
            tool_calls: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.203"),
                source: CostSource::Mixed,
            }]
        );

        Ok(())
    }

    #[test]
    fn computes_estimated_cost_from_user_supplied_price_table()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let price_table = PriceTable::from_prices(vec![ModelTokenPrices::new(
            ModelName::new("local-test-model"),
            CostUsd::from_nanos(10).unwrap(),
            CostUsd::from_nanos(20).unwrap(),
            CostUsd::from_nanos(5).unwrap(),
        )]);
        let mut store = UsageStore::open_with_price_table(
            temp.path().join("usage.sqlite3"),
            &test_store_key(),
            price_table,
        )?;

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record(
                "local-test-model",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                100,
            ),
            usage_record(
                "local-test-model",
                TokenMeasure::Output,
                1_781_956_800_000,
                1_781_956_700_000,
                10,
            ),
            usage_record(
                "local-test-model",
                TokenMeasure::Cache,
                1_781_956_800_000,
                1_781_956_700_000,
                4,
            ),
        ]))?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("local-test-model"),
                cost_usd: CostUsd::from_nanos(1_220).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn opening_v2_schema_preserves_usage_and_adds_cost_columns()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL
            );

            CREATE TABLE cumulative_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, measure, counter_start_ms)
            );

            CREATE TABLE canonical_cost_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                cost_usd_nanos INTEGER NOT NULL
            );

            CREATE TABLE cost_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, counter_start_ms)
            );

            CREATE TABLE token_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            CREATE TABLE cost_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            INSERT INTO canonical_token_usage (
                occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, measure, token_count
            ) VALUES (
                1781956800000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'claude-opus-4-20250514', 'input', 100
            );

            INSERT INTO token_rollups (
                day, repo_bucket, repo_name, repo_path, model, input_tokens, output_tokens, cache_tokens
            ) VALUES (
                '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'claude-opus-4-20250514', 100, 0, 0
            );

            INSERT INTO canonical_cost_usage (
                occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, cost_usd_nanos
            ) VALUES (
                1781956800000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'claude-sonnet-4-20250514', 200000000
            );

            INSERT INTO cost_rollups (
                day, repo_bucket, repo_name, repo_path, model, cost_usd_nanos
            ) VALUES (
                '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'claude-sonnet-4-20250514', 200000000
            );

            PRAGMA user_version = 2;",
        )?;
        drop(connection);

        let mut store = open_test_store(&database_path)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            }]
        );
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            }]
        );
        store.ingest_usage(&UsageRecords::from_token_usage(vec![usage_record(
            "claude-sonnet-4-20250514",
            TokenMeasure::Output,
            1_781_956_750_000,
            1_781_956_700_000,
            200,
        )]))?;
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            }]
        );
        drop(store);

        let connection = open_raw_test_connection(database_path)?;
        let (counter_start, source): (Option<i64>, i64) = connection.query_row(
            "SELECT counter_start_ms, cost_source FROM canonical_cost_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let native_rollup: i64 = connection.query_row(
            "SELECT native_cost_usd_nanos FROM cost_rollups",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(counter_start, None);
        assert_eq!(source, NATIVE_COST_SOURCE);
        assert_eq!(native_rollup, 200_000_000);

        Ok(())
    }

    #[test]
    fn opening_v3_schema_converts_text_cost_sources_to_typed_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL
            );

            CREATE TABLE cumulative_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, measure, counter_start_ms)
            );

            CREATE TABLE canonical_cost_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                cost_usd_nanos INTEGER NOT NULL,
                cost_source TEXT NOT NULL,
                estimated_token_count INTEGER,
                estimated_token_price_nanos INTEGER
            );

            CREATE TABLE cost_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, model, counter_start_ms)
            );

            CREATE TABLE token_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            CREATE TABLE cost_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                native_cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                estimated_cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );

            INSERT INTO canonical_cost_usage (
                occurred_at_ms, counter_start_ms, day, repo_bucket, repo_name, repo_path, model,
                cost_usd_nanos, cost_source, estimated_token_count, estimated_token_price_nanos
            ) VALUES
                (
                    1781956800000, 1781956700000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                    'claude-sonnet-4-20250514', 200000000, 'native', NULL, NULL
                ),
                (
                    1781956900000, 1781956700000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                    'claude-sonnet-4-20250514', 3000000, 'estimated', 200, 15000
                );

            INSERT INTO cost_rollups (
                day, repo_bucket, repo_name, repo_path, model, native_cost_usd_nanos, estimated_cost_usd_nanos
            ) VALUES (
                '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'claude-sonnet-4-20250514', 200000000, 3000000
            );

            PRAGMA user_version = 3;",
        )?;
        drop(connection);

        let store = open_test_store(&database_path)?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.203"),
                source: CostSource::Mixed,
            }]
        );
        drop(store);

        let connection = open_raw_test_connection(database_path)?;
        let cost_sources = connection
            .prepare("SELECT cost_source FROM canonical_cost_usage ORDER BY cost_source")?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            cost_sources,
            vec![NATIVE_COST_SOURCE, ESTIMATED_COST_SOURCE]
        );

        Ok(())
    }

    #[test]
    fn opening_old_schema_recreates_repo_aware_tables() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL
            );

            CREATE TABLE cumulative_counter_snapshots (
                repo_name TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_name, model, measure, counter_start_ms)
            );

            CREATE TABLE token_rollups (
                day TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, model)
            );",
        )?;
        drop(connection);

        let mut store = open_test_store(database_path)?;
        store.ingest_token_usage(&[usage_record(
            "claude-opus-4-20250514",
            TokenMeasure::Input,
            1_781_956_800_000,
            1_781_956_700_000,
            100,
        )])?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            }]
        );

        Ok(())
    }

    #[test]
    fn opening_newer_schema_is_rejected_without_dropping_tables()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE keep_me (value TEXT NOT NULL);
            INSERT INTO keep_me (value) VALUES ('still here');
            PRAGMA user_version = 5;",
        )?;
        drop(connection);

        let result = UsageStore::open(&database_path, &test_store_key());

        assert!(matches!(
            result,
            Err(StoreError::IncompatibleSchema {
                found: 5,
                supported: CURRENT_SCHEMA_VERSION,
            })
        ));

        let connection = open_raw_test_connection(&database_path)?;
        let value: String =
            connection.query_row("SELECT value FROM keep_me", [], |row| row.get(0))?;
        assert_eq!(value, "still here");

        Ok(())
    }

    fn open_test_store(path: impl AsRef<Path>) -> Result<UsageStore, StoreError> {
        UsageStore::open(path, &test_store_key())
    }

    fn open_raw_test_connection(path: impl AsRef<Path>) -> Result<Connection, StoreError> {
        let connection = Connection::open(path)?;
        apply_database_key(&connection, &test_store_key())?;
        Ok(connection)
    }

    fn test_store_key() -> StoreKey {
        StoreKey::from_bytes_for_test([7; 32])
    }

    fn usage_record(
        model: &str,
        measure: TokenMeasure,
        occurred_at_ms: i64,
        counter_start_ms: i64,
        token_count: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord::new(
            TimestampMillis::new_for_test(occurred_at_ms),
            TimestampMillis::new_for_test(counter_start_ms),
            kvasir_repo("/not/persisted"),
            ModelName::new(model),
            measure,
            TokenCount::new(token_count),
        )
    }

    fn usage_record_for_repo(
        repo: RepoBucket,
        model: &str,
        measure: TokenMeasure,
        occurred_at_ms: i64,
        counter_start_ms: i64,
        token_count: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord::new(
            TimestampMillis::new_for_test(occurred_at_ms),
            TimestampMillis::new_for_test(counter_start_ms),
            repo,
            ModelName::new(model),
            measure,
            TokenCount::new(token_count),
        )
    }

    fn cost_record_for_repo(
        repo: RepoBucket,
        model: &str,
        occurred_at_ms: i64,
        counter_start_ms: i64,
        cost: &str,
    ) -> CostUsageRecord {
        CostUsageRecord::new(
            TimestampMillis::new_for_test(occurred_at_ms),
            TimestampMillis::new_for_test(counter_start_ms),
            repo,
            ModelName::new(model),
            cost_usd(cost),
        )
    }

    fn tool_call_record_for_repo(
        repo: RepoBucket,
        harness: &str,
        tool_name: &str,
        occurred_at_ms: i64,
    ) -> ToolCallRecord {
        ToolCallRecord::new(
            ToolCallEventKey::new(format!("test:{harness}:{tool_name}:{occurred_at_ms}")),
            TimestampMillis::new_for_test(occurred_at_ms),
            repo,
            HarnessName::new(harness),
            ToolName::new(tool_name),
        )
    }

    fn cost_usd(value: &str) -> CostUsd {
        CostUsd::from_decimal_str(value).expect("test cost must be valid")
    }

    fn kvasir_repo(path: &str) -> RepoBucket {
        RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new(path),
        ))
    }
}
