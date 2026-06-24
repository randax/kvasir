use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::pricing::PriceTable;
use crate::rpc::{
    ContentAvailability, ContentKindAvailability, ContentQuery, ContentReplay, ContentReplayItem,
    ContentUnavailableReason, CostRollup, CostRollupQuery, CostSource, HarnessName, ModelName,
    PromptId, RollupDay, RollupQuery, SessionId, SpanId, SpanName, TimestampMillis, TokenRollup,
    ToolCallRollup, ToolCallRollupQuery, ToolName, Trace, TraceDurationMeasures, TraceId,
    TraceQuery, TraceSpan, TraceSpanKind,
};
use crate::usage::{
    ContentKind, ContentRecord, ContentText, CostUsageRecord, RepoBucket, RepoIdentity, RepoName,
    RepoPath, TokenCount, TokenMeasure, TokenUsageKind, TokenUsageRecord, TokenUsageSignal,
    ToolCallKind, ToolCallRecord, UsageRecords,
};

const CURRENT_SCHEMA_VERSION: i64 = 11;
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
    #[error("non-monotonic cumulative tool-call counter")]
    NonMonotonicToolCallCounter {
        harness: HarnessName,
        tool_name: ToolName,
        counter_start: TimestampMillis,
        previous_value: i64,
        current_value: i64,
    },
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
        Self::ingest_trace_spans_in_transaction(&transaction, &records.trace_spans)?;
        Self::ingest_content_in_transaction(&transaction, &records.content)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn token_rollups(&self, query: RollupQuery) -> Result<Vec<TokenRollup>, StoreError> {
        let repo_filter = query.repo.as_ref().map(StoredRepo::from_bucket);
        let repo_bucket_filter = repo_filter.as_ref().map(|repo| repo.bucket);
        let repo_name_filter = repo_filter.as_ref().map(|repo| repo.name);
        let repo_path_filter = repo_filter.as_ref().map(|repo| repo.path);
        let input_signal = TokenUsageSignal::authoritative_for(TokenMeasure::Input).storage_name();
        let output_signal =
            TokenUsageSignal::authoritative_for(TokenMeasure::Output).storage_name();
        let cache_signal = TokenUsageSignal::authoritative_for(TokenMeasure::Cache).storage_name();
        let opencode_trace_signal = TokenUsageSignal::OpenCodeTraces.storage_name();
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                model,
                SUM(CASE WHEN measure = 'input' AND token_signal = ?6 THEN token_count ELSE 0 END)
                    + SUM(CASE WHEN measure = 'input' AND token_signal = ?9 AND superseded_metric_token_usage_id IS NULL THEN token_count ELSE 0 END)
                    AS input_tokens,
                SUM(CASE WHEN measure = 'output' AND token_signal = ?7 THEN token_count ELSE 0 END)
                    + SUM(CASE WHEN measure = 'output' AND token_signal = ?9 AND superseded_metric_token_usage_id IS NULL THEN token_count ELSE 0 END)
                    AS output_tokens,
                SUM(CASE WHEN measure = 'cache' AND token_signal = ?8 THEN token_count ELSE 0 END)
                    + SUM(CASE WHEN measure = 'cache' AND token_signal = ?9 AND superseded_metric_token_usage_id IS NULL THEN token_count ELSE 0 END)
                    AS cache_tokens
             FROM canonical_token_usage
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
                AND (?3 IS NULL OR repo_name = ?3)
                AND (?4 IS NULL OR repo_path = ?4)
                AND (?5 IS NULL OR repo_bucket = ?5)
                AND (
                    (measure = 'input' AND token_signal IN (?6, ?9))
                    OR (measure = 'output' AND token_signal IN (?7, ?9))
                    OR (measure = 'cache' AND token_signal IN (?8, ?9))
                )
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
                input_signal,
                output_signal,
                cache_signal,
                opencode_trace_signal,
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

    pub fn traces(&self, query: TraceQuery) -> Result<Vec<Trace>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                trace_id,
                span_id,
                parent_span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms,
                tool_name
             FROM canonical_trace_spans
             WHERE harness = ?1 AND session_id = ?2 AND prompt_id = ?3
             ORDER BY trace_id, started_at_ms, span_id",
        )?;
        let rows = statement
            .query_map(
                params![
                    query.harness.as_str(),
                    query.session_id.as_str(),
                    query.prompt_id.as_str()
                ],
                |row| {
                    let kind: String = row.get(3)?;
                    let parent_span_id: Option<String> = row.get(2)?;
                    let tool_name: Option<String> = row.get(8)?;
                    Ok(StoredTraceSpan {
                        trace_id: row.get(0)?,
                        span: TraceSpan {
                            span_id: SpanId::new(row.get::<_, String>(1)?),
                            parent_span_id: parent_span_id.map(SpanId::new),
                            kind: trace_span_kind_from_storage(&kind)?,
                            name: SpanName::new(row.get::<_, String>(4)?),
                            started_at: TimestampMillis::from_millis(row.get(5)?),
                            ended_at: TimestampMillis::from_millis(row.get(6)?),
                            duration_ms: unsigned_token_column(row, 7)?,
                            tool_name: tool_name.map(ToolName::new),
                        },
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut spans_by_trace_id: BTreeMap<String, Vec<TraceSpan>> = BTreeMap::new();
        for row in rows {
            spans_by_trace_id
                .entry(row.trace_id)
                .or_default()
                .push(row.span);
        }
        Ok(spans_by_trace_id
            .into_iter()
            .map(|(trace_id, spans)| {
                let durations = trace_duration_measures(&spans);
                Trace {
                    session_id: query.session_id.clone(),
                    prompt_id: query.prompt_id.clone(),
                    trace_id: TraceId::new(trace_id),
                    spans,
                    durations,
                }
            })
            .collect())
    }

    pub fn content_replay(&self, query: ContentQuery) -> Result<ContentReplay, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                occurred_at_ms,
                harness,
                content_kind,
                content
             FROM canonical_content_records
             WHERE harness = ?1 AND session_id = ?2 AND prompt_id = ?3
             ORDER BY occurred_at_ms, id",
        )?;
        let items = statement
            .query_map(
                params![
                    query.harness.as_str(),
                    query.session_id.as_str(),
                    query.prompt_id.as_str()
                ],
                |row| {
                    let content_kind: String = row.get(2)?;
                    let content: String = row.get(3)?;
                    Ok(ContentReplayItem {
                        occurred_at: TimestampMillis::from_millis(row.get(0)?),
                        harness: HarnessName::new(row.get::<_, String>(1)?),
                        kind: content_kind_from_storage(&content_kind)?,
                        content: ContentText::new(content).ok_or_else(|| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Text,
                                Box::<dyn std::error::Error + Send + Sync>::from(
                                    "content replay row has empty content",
                                ),
                            )
                        })?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let prompt_exists = !items.is_empty()
            || self.prompt_exists(&query.harness, &query.session_id, &query.prompt_id)?;
        let availability = content_availability(&query.harness, &items, prompt_exists);
        Ok(ContentReplay {
            session_id: query.session_id,
            prompt_id: query.prompt_id,
            items,
            availability,
        })
    }

    fn prompt_exists(
        &self,
        harness: &HarnessName,
        session_id: &SessionId,
        prompt_id: &PromptId,
    ) -> Result<bool, StoreError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM canonical_trace_spans
                    WHERE harness = ?1 AND session_id = ?2 AND prompt_id = ?3
                )",
                params![harness.as_str(), session_id.as_str(), prompt_id.as_str()],
                |row| row.get(0),
            )
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
            10 => {}
            9 => migrate_v9_to_v10(&transaction)?,
            8 => {
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            7 => {
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            6 => {
                migrate_v6_to_v7(&transaction)?;
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            5 => {
                migrate_v5_to_v6(&transaction)?;
                migrate_v6_to_v7(&transaction)?;
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            4 => {
                migrate_v4_to_v5(&transaction)?;
                migrate_v5_to_v6(&transaction)?;
                migrate_v6_to_v7(&transaction)?;
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            3 => {
                migrate_v3_to_v4(&transaction)?;
                migrate_v4_to_v5(&transaction)?;
                migrate_v5_to_v6(&transaction)?;
                migrate_v6_to_v7(&transaction)?;
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            2 => {
                migrate_v2_to_v4(&transaction)?;
                migrate_v4_to_v5(&transaction)?;
                migrate_v5_to_v6(&transaction)?;
                migrate_v6_to_v7(&transaction)?;
                migrate_v7_to_v8(&transaction)?;
                migrate_v8_to_v9(&transaction)?;
                migrate_v9_to_v10(&transaction)?;
            }
            _ if schema_version < 2 => drop_incompatible_usage_tables(&transaction)?,
            _ => {}
        }
        if schema_version < 11 {
            migrate_v10_to_v11(&transaction)?;
        }

        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                token_signal TEXT NOT NULL DEFAULT 'metrics',
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL,
                superseded_metric_token_usage_id INTEGER
            );

            CREATE TABLE IF NOT EXISTS cumulative_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                token_signal TEXT NOT NULL DEFAULT 'metrics',
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, token_signal, model, measure, counter_start_ms)
            );

            CREATE TABLE IF NOT EXISTS token_delta_events (
                event_key TEXT PRIMARY KEY
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
                estimated_token_price_nanos INTEGER,
                estimated_token_usage_id INTEGER
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
            );

            CREATE TABLE IF NOT EXISTS tool_call_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, harness, tool_name, counter_start_ms)
            );

            CREATE TABLE IF NOT EXISTS canonical_trace_spans (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness TEXT NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL,
                parent_span_id TEXT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                tool_name TEXT,
                UNIQUE(harness, session_id, prompt_id, trace_id, span_id)
            );

            CREATE TABLE IF NOT EXISTS canonical_content_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content TEXT NOT NULL
            );",
        )?;
        transaction.execute(
            "CREATE INDEX IF NOT EXISTS canonical_trace_spans_session_prompt
             ON canonical_trace_spans (harness, session_id, prompt_id, started_at_ms, span_id)",
            [],
        )?;
        transaction.execute(
            "CREATE INDEX IF NOT EXISTS canonical_content_records_session_prompt
             ON canonical_content_records (session_id, prompt_id, occurred_at_ms, id)",
            [],
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
        migrate_v5_to_v6(&transaction)?;
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
            let Some(delta) = Self::token_delta(transaction, record)? else {
                continue;
            };
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            transaction.execute(
                "INSERT INTO canonical_token_usage (
                    occurred_at_ms, day, repo_bucket, repo_name, repo_path, token_signal, model, measure, token_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.occurred_at.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.signal.storage_name(),
                    record.model.as_str(),
                    record.measure.storage_name(),
                    delta,
                ],
            )?;
            let token_usage_id = transaction.last_insert_rowid();

            let superseding_existing_metric_token_usage_id =
                if record.signal == TokenUsageSignal::OpenCodeTraces {
                    Self::claim_one_matching_metric_token_for_opencode_trace(
                        transaction,
                        token_usage_id,
                        record,
                        &stored_repo,
                        day.as_str(),
                        delta,
                    )?
                } else {
                    None
                };
            let superseded_by_existing_metric =
                superseding_existing_metric_token_usage_id.is_some();
            if let Some(metric_token_usage_id) = superseding_existing_metric_token_usage_id {
                transaction.execute(
                    "UPDATE canonical_token_usage
                     SET superseded_metric_token_usage_id = ?1
                     WHERE id = ?2",
                    params![metric_token_usage_id, token_usage_id],
                )?;
            }

            let superseded_opencode_token_usage_id = if record.signal == TokenUsageSignal::Metrics {
                Self::supersede_one_matching_opencode_trace_token(
                    transaction,
                    token_usage_id,
                    record,
                    &stored_repo,
                    day.as_str(),
                    delta,
                )?
            } else {
                None
            };

            if record.signal.is_authoritative_for(record.measure) && !superseded_by_existing_metric
            {
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
                if let Some(superseded_token_usage_id) = superseded_opencode_token_usage_id {
                    transaction.execute(
                        "UPDATE token_rollups
                         SET input_tokens = input_tokens - ?1,
                             output_tokens = output_tokens - ?2,
                             cache_tokens = cache_tokens - ?3
                         WHERE day = ?4
                            AND repo_bucket = ?5
                            AND repo_name = ?6
                            AND repo_path = ?7
                            AND model = ?8",
                        params![
                            input_tokens,
                            output_tokens,
                            cache_tokens,
                            day,
                            stored_repo.bucket,
                            stored_repo.name,
                            stored_repo.path,
                            record.model.as_str(),
                        ],
                    )?;
                    Self::remove_estimated_cost_for_superseded_opencode_trace_token(
                        transaction,
                        superseded_token_usage_id,
                    )?;
                }
                token_deltas.push(TokenUsageDelta {
                    token_usage_id,
                    occurred_at: record.occurred_at,
                    counter_start: record.counter_start,
                    repo: record.repo.clone(),
                    model: record.model.clone(),
                    measure: record.measure,
                    token_count: TokenCount::new(u64::try_from(delta).expect("delta is positive")),
                });
            }
        }
        Ok(token_deltas)
    }

    fn claim_one_matching_metric_token_for_opencode_trace(
        transaction: &rusqlite::Transaction<'_>,
        opencode_token_usage_id: i64,
        record: &TokenUsageRecord,
        stored_repo: &StoredRepo<'_>,
        day: &str,
        token_count: i64,
    ) -> Result<Option<i64>, StoreError> {
        let metric_token_usage_id = transaction.query_row(
            "SELECT metrics.id
             FROM canonical_token_usage AS metrics
             WHERE metrics.day = ?1
                AND metrics.repo_bucket = ?2
                AND metrics.repo_name = ?3
                AND metrics.repo_path = ?4
                AND metrics.token_signal = ?5
                AND metrics.model = ?6
                AND metrics.measure = ?7
                AND metrics.token_count = ?8
                AND NOT EXISTS (
                    SELECT 1
                    FROM canonical_token_usage AS opencode
                    WHERE opencode.superseded_metric_token_usage_id = metrics.id
                        AND opencode.id != ?9
                )
             ORDER BY ABS(metrics.occurred_at_ms - ?10), metrics.id
             LIMIT 1",
            params![
                day,
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                TokenUsageSignal::Metrics.storage_name(),
                record.model.as_str(),
                record.measure.storage_name(),
                token_count,
                opencode_token_usage_id,
                record.occurred_at.value(),
            ],
            |row| row.get::<_, i64>(0),
        );
        match metric_token_usage_id {
            Ok(token_usage_id) => Ok(Some(token_usage_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(StoreError::from(err)),
        }
    }

    fn supersede_one_matching_opencode_trace_token(
        transaction: &rusqlite::Transaction<'_>,
        metric_token_usage_id: i64,
        record: &TokenUsageRecord,
        stored_repo: &StoredRepo<'_>,
        day: &str,
        token_count: i64,
    ) -> Result<Option<i64>, StoreError> {
        let token_usage_id = transaction.query_row(
            "SELECT id
             FROM canonical_token_usage
             WHERE day = ?1
                AND repo_bucket = ?2
                AND repo_name = ?3
                AND repo_path = ?4
                AND token_signal = ?5
                AND model = ?6
                AND measure = ?7
                AND token_count = ?8
                AND superseded_metric_token_usage_id IS NULL
             ORDER BY ABS(occurred_at_ms - ?9), id
             LIMIT 1",
            params![
                day,
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                TokenUsageSignal::OpenCodeTraces.storage_name(),
                record.model.as_str(),
                record.measure.storage_name(),
                token_count,
                record.occurred_at.value(),
            ],
            |row| row.get::<_, i64>(0),
        );
        match token_usage_id {
            Ok(token_usage_id) => {
                transaction.execute(
                    "UPDATE canonical_token_usage
                     SET superseded_metric_token_usage_id = ?1
                     WHERE id = ?2",
                    params![metric_token_usage_id, token_usage_id],
                )?;
                Ok(Some(token_usage_id))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(StoreError::from(err)),
        }
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
            let Some(call_count) = Self::tool_call_delta(transaction, record, &stored_repo)? else {
                continue;
            };
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO canonical_tool_calls (
                    event_key, occurred_at_ms, day, repo_bucket, repo_name, repo_path, harness, tool_name, call_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.event_key.as_str(),
                    record.occurred_at.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.harness.as_str(),
                    record.tool_name.as_str(),
                    call_count,
                ],
            )?;
            if inserted == 0 {
                continue;
            }

            transaction.execute(
                "INSERT INTO tool_call_rollups (
                    day, repo_bucket, repo_name, repo_path, harness, tool_name, call_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, harness, tool_name) DO UPDATE SET
                    call_count = call_count + excluded.call_count",
                params![
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.harness.as_str(),
                    record.tool_name.as_str(),
                    call_count,
                ],
            )?;
        }
        Ok(())
    }

    fn tool_call_delta(
        transaction: &rusqlite::Transaction<'_>,
        record: &ToolCallRecord,
        stored_repo: &StoredRepo<'_>,
    ) -> Result<Option<i64>, StoreError> {
        let current_value = record.call_count.storage_value();
        let ToolCallKind::Cumulative { counter_start } = record.kind else {
            return Ok(Some(current_value));
        };

        let previous_value = transaction.query_row(
            "SELECT last_occurred_at_ms, last_value
             FROM tool_call_counter_snapshots
             WHERE repo_bucket = ?1
                AND repo_name = ?2
                AND repo_path = ?3
                AND harness = ?4
                AND tool_name = ?5
                AND counter_start_ms = ?6",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.harness.as_str(),
                record.tool_name.as_str(),
                counter_start.value(),
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
            Ok((_last_occurred_at_ms, previous_value)) => {
                return Err(StoreError::NonMonotonicToolCallCounter {
                    harness: record.harness.clone(),
                    tool_name: record.tool_name.clone(),
                    counter_start,
                    previous_value,
                    current_value,
                });
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => current_value,
            Err(err) => return Err(StoreError::Sqlite(err)),
        };

        transaction.execute(
            "INSERT INTO tool_call_counter_snapshots (
                repo_bucket, repo_name, repo_path, harness, tool_name, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(repo_bucket, repo_name, repo_path, harness, tool_name, counter_start_ms) DO UPDATE SET
                last_occurred_at_ms = excluded.last_occurred_at_ms,
                last_value = excluded.last_value",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.harness.as_str(),
                record.tool_name.as_str(),
                counter_start.value(),
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

    fn ingest_trace_spans_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[crate::usage::TraceSpanRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            transaction.execute(
                "INSERT OR REPLACE INTO canonical_trace_spans (
                    harness,
                    session_id,
                    prompt_id,
                    trace_id,
                    span_id,
                    parent_span_id,
                    kind,
                    name,
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    tool_name
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    record.harness.as_str(),
                    record.session_id.as_str(),
                    record.prompt_id.as_str(),
                    record.trace_id.as_str(),
                    record.span_id.as_str(),
                    record.parent_span_id.as_ref().map(SpanId::as_str),
                    record.kind.storage_name(),
                    record.name.as_str(),
                    record.started_at.value(),
                    record.ended_at.value(),
                    i64::try_from(record.duration_ms).unwrap_or(i64::MAX),
                    record.tool_name.as_ref().map(ToolName::as_str),
                ],
            )?;
        }
        Ok(())
    }

    fn ingest_content_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[ContentRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            transaction.execute(
                "INSERT OR IGNORE INTO canonical_content_records (
                    event_key,
                    occurred_at_ms,
                    session_id,
                    prompt_id,
                    day,
                    repo_bucket,
                    repo_name,
                    repo_path,
                    harness,
                    content_kind,
                    content
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    record.event_key.as_str(),
                    record.occurred_at.value(),
                    record.session_id.as_str(),
                    record.prompt_id.as_str(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.harness.as_str(),
                    record.kind.storage_name(),
                    record.content.as_str(),
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
                    cost_usd_nanos, cost_source, estimated_token_count, estimated_token_price_nanos,
                    estimated_token_usage_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    delta.token_usage_id,
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

    fn remove_estimated_cost_for_superseded_opencode_trace_token(
        transaction: &rusqlite::Transaction<'_>,
        token_usage_id: i64,
    ) -> Result<(), StoreError> {
        let mut statement = transaction.prepare(
            "SELECT
                id,
                day,
                repo_bucket,
                repo_name,
                repo_path,
	                model,
	                cost_usd_nanos
	             FROM canonical_cost_usage
	             WHERE estimated_token_usage_id = ?1
	                AND cost_source = ?2",
        )?;
        let superseded = statement
            .query_map(params![token_usage_id, ESTIMATED_COST_SOURCE], |row| {
                Ok(EstimatedCostRow {
                    id: row.get(0)?,
                    day: row.get(1)?,
                    repo_bucket: row.get(2)?,
                    repo_name: row.get(3)?,
                    repo_path: row.get(4)?,
                    model: row.get(5)?,
                    cost_usd_nanos: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        for row in superseded {
            transaction.execute(
                "UPDATE cost_rollups
                 SET estimated_cost_usd_nanos = estimated_cost_usd_nanos - ?1
                 WHERE day = ?2
                    AND repo_bucket = ?3
                    AND repo_name = ?4
                    AND repo_path = ?5
                    AND model = ?6",
                params![
                    row.cost_usd_nanos,
                    row.day,
                    row.repo_bucket,
                    row.repo_name,
                    row.repo_path,
                    row.model,
                ],
            )?;
            transaction.execute(
                "DELETE FROM canonical_cost_usage WHERE id = ?1",
                params![row.id],
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
             WHERE repo_bucket = ?1
                AND repo_name = ?2
                AND repo_path = ?3
                AND token_signal = ?4
                AND model = ?5
                AND measure = ?6
                AND counter_start_ms = ?7",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.signal.storage_name(),
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
                repo_bucket, repo_name, repo_path, token_signal, model, measure, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(repo_bucket, repo_name, repo_path, token_signal, model, measure, counter_start_ms) DO UPDATE SET
                last_occurred_at_ms = excluded.last_occurred_at_ms,
                last_value = excluded.last_value",
            params![
                stored_repo.bucket,
                stored_repo.name,
                stored_repo.path,
                record.signal.storage_name(),
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

    fn token_delta(
        transaction: &rusqlite::Transaction<'_>,
        record: &TokenUsageRecord,
    ) -> Result<Option<i64>, StoreError> {
        match &record.kind {
            TokenUsageKind::Cumulative => Self::counter_delta(transaction, record),
            TokenUsageKind::Delta { event_key } => {
                let inserted = transaction.execute(
                    "INSERT OR IGNORE INTO token_delta_events (event_key) VALUES (?1)",
                    params![event_key.as_str()],
                )?;
                if inserted == 0 {
                    Ok(None)
                } else {
                    Ok(Some(record.token_count.storage_value()))
                }
            }
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
        DROP TABLE IF EXISTS tool_call_counter_snapshots;
        DROP TABLE IF EXISTS canonical_trace_spans;
        DROP TABLE IF EXISTS canonical_content_records;
        DROP TABLE IF EXISTS cumulative_counter_snapshots;
        DROP TABLE IF EXISTS cost_counter_snapshots;
        DROP TABLE IF EXISTS token_rollups;
        DROP TABLE IF EXISTS cost_rollups;
        DROP TABLE IF EXISTS tool_call_rollups;",
    )?;
    Ok(())
}

fn migrate_v4_to_v5(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS canonical_trace_spans (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            harness TEXT NOT NULL DEFAULT 'unknown',
            session_id TEXT NOT NULL,
            prompt_id TEXT NOT NULL,
            trace_id TEXT NOT NULL,
            span_id TEXT NOT NULL,
            parent_span_id TEXT,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            started_at_ms INTEGER NOT NULL,
            ended_at_ms INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            tool_name TEXT,
            UNIQUE(harness, session_id, prompt_id, trace_id, span_id)
        );

        CREATE INDEX IF NOT EXISTS canonical_trace_spans_session_prompt
        ON canonical_trace_spans (harness, session_id, prompt_id, started_at_ms, span_id);",
    )?;
    Ok(())
}

fn migrate_v5_to_v6(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if !canonical_token_signal_column_exists(transaction)? {
        transaction.execute(
            "ALTER TABLE canonical_token_usage
             ADD COLUMN token_signal TEXT NOT NULL DEFAULT 'metrics'",
            [],
        )?;
    }

    if !snapshot_token_signal_column_exists(transaction)? {
        transaction.execute_batch(
            "ALTER TABLE cumulative_counter_snapshots RENAME TO cumulative_counter_snapshots_v5;

            CREATE TABLE cumulative_counter_snapshots (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                token_signal TEXT NOT NULL DEFAULT 'metrics',
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, token_signal, model, measure, counter_start_ms)
            );

            INSERT INTO cumulative_counter_snapshots (
                repo_bucket,
                repo_name,
                repo_path,
                token_signal,
                model,
                measure,
                counter_start_ms,
                last_occurred_at_ms,
                last_value
            )
            SELECT
                repo_bucket,
                repo_name,
                repo_path,
                'metrics',
                model,
                measure,
                counter_start_ms,
                last_occurred_at_ms,
                last_value
            FROM cumulative_counter_snapshots_v5;

            DROP TABLE cumulative_counter_snapshots_v5;",
        )?;
    }

    Ok(())
}

fn migrate_v6_to_v7(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if table_exists(transaction, "canonical_token_usage")?
        && !table_column_exists(
            transaction,
            "canonical_token_usage",
            "superseded_metric_token_usage_id",
        )?
    {
        transaction.execute(
            "ALTER TABLE canonical_token_usage
             ADD COLUMN superseded_metric_token_usage_id INTEGER",
            [],
        )?;
    }
    if table_exists(transaction, "canonical_cost_usage")?
        && !table_column_exists(
            transaction,
            "canonical_cost_usage",
            "estimated_token_usage_id",
        )?
    {
        transaction.execute(
            "ALTER TABLE canonical_cost_usage
             ADD COLUMN estimated_token_usage_id INTEGER",
            [],
        )?;
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS canonical_content_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_key TEXT NOT NULL UNIQUE,
            occurred_at_ms INTEGER NOT NULL,
            session_id TEXT NOT NULL DEFAULT '',
            prompt_id TEXT NOT NULL DEFAULT '',
            day TEXT NOT NULL,
            repo_bucket TEXT NOT NULL,
            repo_name TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            harness TEXT NOT NULL,
            content_kind TEXT NOT NULL,
            content TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn migrate_v7_to_v8(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if table_exists(transaction, "canonical_tool_calls")?
        && !table_column_exists(transaction, "canonical_tool_calls", "call_count")?
    {
        transaction.execute(
            "ALTER TABLE canonical_tool_calls
             ADD COLUMN call_count INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }

    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_call_counter_snapshots (
            repo_bucket TEXT NOT NULL,
            repo_name TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            harness TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            counter_start_ms INTEGER NOT NULL,
            last_occurred_at_ms INTEGER NOT NULL,
            last_value INTEGER NOT NULL,
            PRIMARY KEY(repo_bucket, repo_name, repo_path, harness, tool_name, counter_start_ms)
        );",
    )?;
    Ok(())
}

fn migrate_v8_to_v9(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if table_exists(transaction, "canonical_content_records")?
        && !table_column_exists(transaction, "canonical_content_records", "session_id")?
    {
        transaction.execute_batch(
            "ALTER TABLE canonical_content_records
             ADD COLUMN session_id TEXT NOT NULL DEFAULT '';
             ALTER TABLE canonical_content_records
             ADD COLUMN prompt_id TEXT NOT NULL DEFAULT '';
             DELETE FROM canonical_content_records
             WHERE session_id = '' OR prompt_id = '';",
        )?;
    }
    if table_exists(transaction, "canonical_content_records")? {
        transaction.execute(
            "CREATE INDEX IF NOT EXISTS canonical_content_records_session_prompt
             ON canonical_content_records (session_id, prompt_id, occurred_at_ms, id)",
            [],
        )?;
    }
    Ok(())
}

fn migrate_v9_to_v10(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if table_exists(transaction, "canonical_trace_spans")? {
        let has_harness = table_column_exists(transaction, "canonical_trace_spans", "harness")?;
        transaction.execute_batch(
            "CREATE TABLE canonical_trace_spans_v10 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness TEXT NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL,
                parent_span_id TEXT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                tool_name TEXT,
                UNIQUE(harness, session_id, prompt_id, trace_id, span_id)
            );",
        )?;
        if has_harness {
            transaction.execute_batch(
                "INSERT INTO canonical_trace_spans_v10 (
                    id,
                    harness,
                    session_id,
                    prompt_id,
                    trace_id,
                    span_id,
                    parent_span_id,
                    kind,
                    name,
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    tool_name
                )
                SELECT
                    id,
                    harness,
                    session_id,
                    prompt_id,
                    trace_id,
                    span_id,
                    parent_span_id,
                    kind,
                    name,
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    tool_name
                FROM canonical_trace_spans;",
            )?;
        } else {
            transaction.execute_batch(
                "INSERT INTO canonical_trace_spans_v10 (
                    id,
                    harness,
                    session_id,
                    prompt_id,
                    trace_id,
                    span_id,
                    parent_span_id,
                    kind,
                    name,
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    tool_name
                )
                SELECT
                    id,
                    'unknown',
                    session_id,
                    prompt_id,
                    trace_id,
                    span_id,
                    parent_span_id,
                    kind,
                    name,
                    started_at_ms,
                    ended_at_ms,
                    duration_ms,
                    tool_name
                FROM canonical_trace_spans;",
            )?;
        }
        transaction.execute_batch(
            "DROP TABLE canonical_trace_spans;
             ALTER TABLE canonical_trace_spans_v10 RENAME TO canonical_trace_spans;
             CREATE INDEX canonical_trace_spans_session_prompt
             ON canonical_trace_spans (harness, session_id, prompt_id, started_at_ms, span_id);",
        )?;
    }
    Ok(())
}

fn migrate_v10_to_v11(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    if table_exists(transaction, "canonical_tool_calls")? {
        transaction.execute_batch(
            "CREATE TABLE canonical_tool_calls_v11 (
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
             INSERT OR IGNORE INTO canonical_tool_calls_v11 (
                id,
                event_key,
                occurred_at_ms,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                call_count
             )
             SELECT
                id,
                replace(
                    event_key,
                    'harness=' || harness || char(10),
                    'harness=' || replace(lower(trim(harness)), '-', '_') || char(10)
                ),
                occurred_at_ms,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                tool_name,
                call_count
             FROM canonical_tool_calls
             ORDER BY id;
             DROP TABLE canonical_tool_calls;
             ALTER TABLE canonical_tool_calls_v11 RENAME TO canonical_tool_calls;",
        )?;
    }
    if table_exists(transaction, "canonical_content_records")? {
        transaction.execute_batch(
            "CREATE TABLE canonical_content_records_v11 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content TEXT NOT NULL
             );
             INSERT OR IGNORE INTO canonical_content_records_v11 (
                id,
                event_key,
                occurred_at_ms,
                session_id,
                prompt_id,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                content_kind,
                content
             )
             SELECT
                id,
                replace(
                    event_key,
                    'harness=' || harness || char(10),
                    'harness=' || replace(lower(trim(harness)), '-', '_') || char(10)
                ),
                occurred_at_ms,
                session_id,
                prompt_id,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                content_kind,
                content
             FROM canonical_content_records
             ORDER BY id;
             DROP TABLE canonical_content_records;
             ALTER TABLE canonical_content_records_v11 RENAME TO canonical_content_records;",
        )?;
    }
    if table_exists(transaction, "canonical_trace_spans")? {
        transaction.execute_batch(
            "DROP INDEX IF EXISTS canonical_trace_spans_session_prompt;
             CREATE TABLE canonical_trace_spans_v11 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness TEXT NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL,
                parent_span_id TEXT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                tool_name TEXT,
                UNIQUE(harness, session_id, prompt_id, trace_id, span_id)
             );
             INSERT OR IGNORE INTO canonical_trace_spans_v11 (
                id,
                harness,
                session_id,
                prompt_id,
                trace_id,
                span_id,
                parent_span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms,
                tool_name
             )
             SELECT
                id,
                replace(lower(trim(harness)), '-', '_'),
                session_id,
                prompt_id,
                trace_id,
                span_id,
                parent_span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms,
                tool_name
             FROM canonical_trace_spans
             ORDER BY id;
             DROP TABLE canonical_trace_spans;
             ALTER TABLE canonical_trace_spans_v11 RENAME TO canonical_trace_spans;
             CREATE INDEX canonical_trace_spans_session_prompt
             ON canonical_trace_spans (harness, session_id, prompt_id, started_at_ms, span_id);",
        )?;
    }
    if table_exists(transaction, "tool_call_rollups")? {
        transaction.execute_batch(
            "CREATE TABLE tool_call_rollups_v11 (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                call_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, harness, tool_name)
             );
             INSERT INTO tool_call_rollups_v11 (
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                call_count
             )
             SELECT
                day,
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                tool_name,
                SUM(call_count)
             FROM tool_call_rollups
             GROUP BY
                day,
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                tool_name;
             DROP TABLE tool_call_rollups;
             ALTER TABLE tool_call_rollups_v11 RENAME TO tool_call_rollups;",
        )?;
    }
    if table_exists(transaction, "tool_call_counter_snapshots")? {
        transaction.execute_batch(
            "CREATE TABLE tool_call_counter_snapshots_v11 (
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_bucket, repo_name, repo_path, harness, tool_name, counter_start_ms)
             );
             INSERT INTO tool_call_counter_snapshots_v11 (
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                counter_start_ms,
                last_occurred_at_ms,
                last_value
             )
             SELECT
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                tool_name,
                counter_start_ms,
                MAX(last_occurred_at_ms),
                MAX(last_value)
             FROM tool_call_counter_snapshots
             GROUP BY
                repo_bucket,
                repo_name,
                repo_path,
                replace(lower(trim(harness)), '-', '_'),
                tool_name,
                counter_start_ms;
             DROP TABLE tool_call_counter_snapshots;
             ALTER TABLE tool_call_counter_snapshots_v11 RENAME TO tool_call_counter_snapshots;",
        )?;
    }
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

fn trace_span_kind_from_storage(value: &str) -> rusqlite::Result<TraceSpanKind> {
    TraceSpanKind::from_storage(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "invalid trace span kind {value}"
            )),
        )
    })
}

fn content_kind_from_storage(value: &str) -> rusqlite::Result<ContentKind> {
    ContentKind::from_storage(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "invalid content kind {value}"
            )),
        )
    })
}

fn content_availability(
    harness: &HarnessName,
    items: &[ContentReplayItem],
    prompt_exists: bool,
) -> ContentAvailability {
    if !prompt_exists {
        return ContentAvailability::Unavailable {
            reason: ContentUnavailableReason::PromptNotFound,
        };
    }
    let mut captured = Vec::new();
    for item in items {
        if !captured.contains(&item.kind) {
            captured.push(item.kind);
        }
    }
    let provided = content_kinds_provided_by(harness);
    let mut kinds = Vec::new();
    for kind in &captured {
        kinds.push(ContentKindAvailability::Captured { kind: *kind });
    }
    for kind in ContentKind::ALL {
        if !captured.contains(&kind) {
            kinds.push(ContentKindAvailability::Unavailable {
                kind,
                reason: if provided.contains(&kind) {
                    ContentUnavailableReason::NotCapturedForPrompt
                } else {
                    ContentUnavailableReason::NotProvidedByHarness
                },
            });
        }
    }
    ContentAvailability::Captured {
        harness: harness.clone(),
        kinds,
    }
}

fn content_kinds_provided_by(harness: &HarnessName) -> Vec<ContentKind> {
    match harness.as_str() {
        "claude" | "claude_code" => ContentKind::ALL.to_vec(),
        "codex" => ContentKind::ALL.to_vec(),
        "github_copilot" => ContentKind::ALL.to_vec(),
        "opencode" => ContentKind::ALL.to_vec(),
        _ => Vec::new(),
    }
}

fn trace_duration_measures(spans: &[TraceSpan]) -> TraceDurationMeasures {
    let first_interaction = spans
        .iter()
        .filter(|span| span.kind == TraceSpanKind::Interaction)
        .min_by_key(|span| span.started_at);
    let first_request = spans
        .iter()
        .filter(|span| span.kind == TraceSpanKind::LlmRequest)
        .min_by_key(|span| span.started_at);
    TraceDurationMeasures {
        ttft_ms: first_interaction
            .zip(first_request)
            .and_then(|(interaction, request)| {
                u64::try_from(request.started_at.value() - interaction.started_at.value()).ok()
            }),
        request_ms: trace_duration_sum(spans, TraceSpanKind::LlmRequest),
        tool_ms: trace_duration_sum(spans, TraceSpanKind::ToolCall),
    }
}

fn trace_duration_sum(spans: &[TraceSpan], kind: TraceSpanKind) -> Option<u64> {
    let mut matching_spans = spans.iter().filter(|span| span.kind == kind).peekable();
    matching_spans.peek()?;
    matching_spans.try_fold(0_u64, |total, span| total.checked_add(span.duration_ms))
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

fn canonical_token_signal_column_exists(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<bool, StoreError> {
    table_column_exists(transaction, "canonical_token_usage", "token_signal")
}

fn table_column_exists(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<bool, StoreError> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|name| name == column))
}

fn table_exists(transaction: &rusqlite::Transaction<'_>, table: &str) -> Result<bool, StoreError> {
    let count = transaction.query_row(
        "SELECT COUNT(*)
         FROM sqlite_master
         WHERE type = 'table' AND name = ?1",
        params![table],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count > 0)
}

fn snapshot_token_signal_column_exists(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<bool, StoreError> {
    let mut statement = transaction.prepare("PRAGMA table_info(cumulative_counter_snapshots)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|name| name == "token_signal"))
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
    token_usage_id: i64,
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

struct EstimatedCostRow {
    id: i64,
    day: String,
    repo_bucket: String,
    repo_name: String,
    repo_path: String,
    model: String,
    cost_usd_nanos: i64,
}

struct StoredTraceSpan {
    trace_id: String,
    span: TraceSpan,
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
    use crate::otlp::{
        parse_otlp_json_traces, parse_otlp_json_usage_logs, parse_otlp_json_usage_metrics,
        parse_otlp_protobuf_usage_metrics,
    };
    use crate::pricing::ModelTokenPrices;
    use crate::rpc::TimestampMillis;
    use crate::usage::{
        ContentEventKey, ContentKind, ContentRecord, ContentText, CostUsd, RepoIdentity, RepoName,
        RepoPath, TokenCount, TokenUsageEventKey, TokenUsageSignal, ToolCallCount,
        ToolCallEventKey,
    };

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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
    fn tool_call_rollups_span_claude_codex_and_copilot_by_tool_and_repo()
    -> Result<(), Box<dyn std::error::Error>> {
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
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            codex_tool_call_metric_json_payload("Read", 2).as_bytes(),
        )?)?;
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload("Read", 3).as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![
                ToolCallRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: repo.clone(),
                    harness: HarnessName::new("claude_code"),
                    tool_name: ToolName::new("Read"),
                    call_count: 1,
                },
                ToolCallRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: repo.clone(),
                    harness: HarnessName::new("codex"),
                    tool_name: ToolName::new("Read"),
                    call_count: 2,
                },
                ToolCallRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo,
                    harness: HarnessName::new("github_copilot"),
                    tool_name: ToolName::new("Read"),
                    call_count: 3,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counters_roll_up_only_new_calls() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_cumulative_tool_call_metric_json_payload("Read", 3, 5).as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 5,
            }]
        );

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counter_replays_do_not_double_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");
        let records = parse_otlp_json_usage_metrics(
            copilot_cumulative_tool_call_metric_json_payload("Read", 3, 5).as_bytes(),
        )?;

        store.ingest_usage(&records)?;
        store.ingest_usage(&records)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 5,
            }]
        );

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counter_ignores_out_of_order_samples()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_cumulative_tool_call_metric_json_payload("Read", 3, 5).as_bytes(),
        )?)?;
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload_at("Read", 4, "1781956850000000000").as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 5,
            }]
        );

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counter_rejects_non_monotonic_sample()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        let error = store
            .ingest_usage(&parse_otlp_json_usage_metrics(
                copilot_cumulative_tool_call_metric_json_payload("Read", 5, 2).as_bytes(),
            )?)
            .expect_err("same cumulative counter cannot decrease");

        assert!(matches!(
            error,
            StoreError::NonMonotonicToolCallCounter {
                harness,
                tool_name,
                previous_value: 5,
                current_value: 2,
                ..
            } if harness == HarnessName::new("github_copilot") && tool_name == ToolName::new("Read")
        ));

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counter_restart_rolls_up_current_value()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload("Read", 5).as_bytes(),
        )?)?;
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload_with_counter_start(
                "Read",
                2,
                "1781956900000000000",
                "1781956950000000000",
            )
            .as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 7,
            }]
        );

        Ok(())
    }

    #[test]
    fn copilot_tool_call_counter_restarted_series_do_not_collide_on_event_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload_with_counter_start(
                "Read",
                2,
                "1781956700000000000",
                "1781956800000000000",
            )
            .as_bytes(),
        )?)?;
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_tool_call_metric_json_payload_with_counter_start(
                "Read",
                2,
                "1781956750000000000",
                "1781956800000000000",
            )
            .as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 4,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_tool_call_histograms_roll_up_each_delta_point()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            codex_two_point_tool_call_metric_json_payload("Read", 2, 3).as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("codex"),
                tool_name: ToolName::new("Read"),
                call_count: 5,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_tool_call_histograms_keep_identical_points_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&parse_otlp_json_usage_metrics(
            codex_duplicate_tool_call_metric_json_payload("Read", 2).as_bytes(),
        )?)?;

        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(repo.clone())
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                harness: HarnessName::new("codex"),
                tool_name: ToolName::new("Read"),
                call_count: 4,
            }]
        );

        Ok(())
    }

    #[test]
    fn ingest_usage_persists_content_records() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: vec![ContentRecord {
                event_key: ContentEventKey::new("content-event-1"),
                occurred_at: TimestampMillis::new_for_test(1_781_956_800_000),
                session_id: crate::rpc::SessionId::new("session-1"),
                prompt_id: crate::rpc::PromptId::new("prompt-1"),
                repo: kvasir_repo("/repos/kvasir"),
                harness: HarnessName::new("opencode"),
                kind: ContentKind::AssistantMessage,
                content: ContentText::new("stored assistant text").unwrap(),
            }],
        })?;

        let row = store.connection.query_row(
            "SELECT session_id, prompt_id, repo_bucket, repo_name, repo_path, harness, content_kind, content
             FROM canonical_content_records",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )?;

        assert_eq!(
            row,
            (
                "session-1".to_owned(),
                "prompt-1".to_owned(),
                REPO_BUCKET.to_owned(),
                "kvasir".to_owned(),
                "/repos/kvasir".to_owned(),
                "opencode".to_owned(),
                "assistant_message".to_owned(),
                "stored assistant text".to_owned(),
            )
        );

        Ok(())
    }

    #[test]
    fn content_replay_is_scoped_by_harness() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let session_id = crate::rpc::SessionId::new("shared-session");
        let prompt_id = crate::rpc::PromptId::new("shared-prompt");

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: vec![
                ContentRecord {
                    event_key: ContentEventKey::new("opencode-content"),
                    occurred_at: TimestampMillis::new_for_test(1_781_956_800_000),
                    session_id: session_id.clone(),
                    prompt_id: prompt_id.clone(),
                    repo: kvasir_repo("/repos/kvasir"),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::AssistantMessage,
                    content: ContentText::new("opencode text").unwrap(),
                },
                ContentRecord {
                    event_key: ContentEventKey::new("codex-content"),
                    occurred_at: TimestampMillis::new_for_test(1_781_956_800_001),
                    session_id: session_id.clone(),
                    prompt_id: prompt_id.clone(),
                    repo: kvasir_repo("/repos/kvasir"),
                    harness: HarnessName::new("codex"),
                    kind: ContentKind::AssistantMessage,
                    content: ContentText::new("codex text").unwrap(),
                },
            ],
        })?;

        let replay = store.content_replay(ContentQuery {
            harness: HarnessName::new("opencode"),
            session_id,
            prompt_id,
        })?;

        assert_eq!(replay.items.len(), 1);
        assert_eq!(replay.items[0].harness, HarnessName::new("opencode"));
        assert_eq!(replay.items[0].content.as_str(), "opencode text");

        Ok(())
    }

    #[test]
    fn trace_and_content_queries_accept_raw_otlp_harness_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let trace_payload = br#"{
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "GitHub-Copilot" } },
                        { "key": "session.id", "value": { "stringValue": "session-12" } },
                        { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                    ]
                },
                "scopeSpans": [{
                    "spans": [{
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "1111111111111111",
                        "name": "github.copilot.interaction",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802750000000",
                        "attributes": [
                            { "key": "span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    }]
                }]
            }]
        }"#;
        let log_payload = br#"{
            "resourceLogs": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "GitHub-Copilot" } },
                        { "key": "session.id", "value": { "stringValue": "session-12" } },
                        { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956803000000000",
                        "eventName": "github.copilot.content",
                        "body": { "stringValue": "stored copilot text" },
                        "attributes": [
                            { "key": "content.opt_in", "value": { "boolValue": true } },
                            { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                        ]
                    }]
                }]
            }]
        }"#;

        store.ingest_usage(&parse_otlp_json_traces(trace_payload)?)?;
        store.ingest_usage(&parse_otlp_json_usage_logs(log_payload)?)?;

        let traces = store.traces(TraceQuery {
            harness: HarnessName::new("github-copilot"),
            session_id: crate::rpc::SessionId::new("session-12"),
            prompt_id: crate::rpc::PromptId::new("prompt-7"),
        })?;
        assert_eq!(traces.len(), 1);
        assert_eq!(
            traces[0].spans[0].name,
            SpanName::new("github.copilot.interaction")
        );

        let replay = store.content_replay(ContentQuery {
            harness: HarnessName::new("github-copilot"),
            session_id: crate::rpc::SessionId::new("session-12"),
            prompt_id: crate::rpc::PromptId::new("prompt-7"),
        })?;
        assert_eq!(replay.items.len(), 1);
        assert_eq!(replay.items[0].harness, HarnessName::new("github_copilot"));
        assert_eq!(replay.items[0].content.as_str(), "stored copilot text");

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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
            trace_spans: Vec::new(),
            content: Vec::new(),
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
    fn codex_metric_fixture_rolls_up_tokens_and_estimated_cost_by_repo_and_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        let records = parse_otlp_json_usage_metrics(include_bytes!(
            "../tests/fixtures/codex_turn_token_usage_otlp.json"
        ))?;
        store.ingest_usage(&records)?;

        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let query = RollupQuery::new(
            TimestampMillis::new_for_test(1_781_956_000_000),
            TimestampMillis::new_for_test(1_781_970_000_000),
        );

        assert_eq!(
            store.token_rollups(query.clone())?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 80,
            }]
        );
        assert_eq!(
            store.token_rollups(query.with_repo(repo.clone()))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 80,
            }]
        );
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(9_770_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn json_codex_metric_replays_do_not_double_count_tokens_or_estimated_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        let records = parse_otlp_json_usage_metrics(include_bytes!(
            "../tests/fixtures/codex_turn_token_usage_otlp.json"
        ))?;
        store.ingest_usage(&records)?;
        store.ingest_usage(&records)?;

        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 80,
            }]
        );
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(9_770_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_protobuf_metric_records_roll_up_tokens_and_estimated_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let records = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload(
            repo.clone(),
            vec![
                ("input", 1200.0),
                ("output", 450.0),
                ("cached_input", 80.0),
                ("total", 1730.0),
            ],
        ))?;

        store.ingest_usage(&records)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 80,
            }]
        );
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(9_770_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn late_native_cost_replaces_codex_delta_estimated_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let records = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload(
            repo.clone(),
            vec![("input", 1200.0), ("output", 450.0), ("cached_input", 80.0)],
        ))?;

        store.ingest_usage(&records)?;
        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: vec![cost_record_for_repo(
                repo.clone(),
                "gpt-5.4",
                1_781_956_800_000,
                1_781_956_799_000,
                "0.02",
            )],
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: cost_usd("0.02"),
                source: CostSource::Native,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_metric_rollups_keep_repo_and_model_buckets_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let kvasir_repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let other_repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("other"),
            RepoPath::new("/repos/other"),
        ));
        let kvasir_gpt = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            kvasir_repo.clone(),
            vec![codex_histogram_point(
                "gpt-5.4",
                "input",
                100.0,
                1_781_956_800_000_000_000,
            )],
        ))?;
        let kvasir_mini = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            kvasir_repo.clone(),
            vec![codex_histogram_point(
                "gpt-5.4-mini",
                "output",
                50.0,
                1_781_956_801_000_000_000,
            )],
        ))?;
        let other_gpt = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            other_repo.clone(),
            vec![codex_histogram_point(
                "gpt-5.4",
                "input",
                75.0,
                1_781_956_802_000_000_000,
            )],
        ))?;

        store.ingest_usage(&kvasir_gpt)?;
        store.ingest_usage(&kvasir_mini)?;
        store.ingest_usage(&other_gpt)?;

        let query = RollupQuery::new(
            TimestampMillis::new_for_test(1_781_956_000_000),
            TimestampMillis::new_for_test(1_781_970_000_000),
        );
        assert_eq!(
            store.token_rollups(query.clone())?,
            vec![
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir_repo.clone(),
                    model: ModelName::new("gpt-5.4"),
                    input_tokens: 100,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir_repo.clone(),
                    model: ModelName::new("gpt-5.4-mini"),
                    input_tokens: 0,
                    output_tokens: 50,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: other_repo,
                    model: ModelName::new("gpt-5.4"),
                    input_tokens: 75,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
            ]
        );
        assert_eq!(
            store.token_rollups(query.with_repo(kvasir_repo.clone()))?,
            vec![
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir_repo.clone(),
                    model: ModelName::new("gpt-5.4"),
                    input_tokens: 100,
                    output_tokens: 0,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir_repo,
                    model: ModelName::new("gpt-5.4-mini"),
                    input_tokens: 0,
                    output_tokens: 50,
                    cache_tokens: 0,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn codex_estimated_cost_uses_configured_price_table() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempdir()?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let price_table = PriceTable::from_prices(vec![ModelTokenPrices::new(
            ModelName::new("gpt-5.4"),
            CostUsd::from_nanos(10).unwrap(),
            CostUsd::from_nanos(20).unwrap(),
            CostUsd::from_nanos(5).unwrap(),
        )]);
        let mut store = UsageStore::open_with_price_table(
            temp.path().join("usage.sqlite3"),
            &test_store_key(),
            price_table,
        )?;
        let records = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload(
            repo.clone(),
            vec![("input", 1200.0), ("output", 450.0), ("cached_input", 80.0)],
        ))?;

        store.ingest_usage(&records)?;

        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(21_400).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_delta_records_do_not_collapse_same_millisecond_turns()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            codex_delta_record("codex-turn-1-input", "gpt-5.4", TokenMeasure::Input, 100),
            codex_delta_record("codex-turn-2-input", "gpt-5.4", TokenMeasure::Input, 125),
        ]))?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 225,
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
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(562_500).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn parsed_codex_same_millisecond_points_with_matching_counts_remain_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let records = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            repo.clone(),
            vec![
                codex_histogram_point("gpt-5.4", "input", 100.0, 1_781_956_800_000_000_001),
                codex_histogram_point("gpt-5.4", "input", 100.0, 1_781_956_800_000_000_002),
            ],
        ))?;

        store.ingest_usage(&records)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 200,
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
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(500_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn parsed_codex_identical_points_split_across_metrics_remain_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let records = parse_otlp_protobuf_usage_metrics(&codex_split_metric_histogram_payload(
            repo.clone(),
            codex_histogram_point("gpt-5.4", "input", 100.0, 1_781_956_800_000_000_000),
            codex_histogram_point("gpt-5.4", "input", 100.0, 1_781_956_800_000_000_000),
        ))?;

        store.ingest_usage(&records)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 200,
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
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(500_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_metric_replays_do_not_double_count_tokens_or_estimated_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let records = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload(
            repo.clone(),
            vec![("input", 1200.0), ("output", 450.0), ("cached_input", 80.0)],
        ))?;

        store.ingest_usage(&records)?;
        store.ingest_usage(&records)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 80,
            }]
        );
        assert_eq!(
            store.cost_rollups(CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(9_770_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn codex_metric_replays_dedupe_when_point_order_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        let input = codex_histogram_point("gpt-5.4", "input", 1200.0, 1_781_956_800_000_000_000);
        let output = codex_histogram_point("gpt-5.4", "output", 450.0, 1_781_956_800_000_000_000);
        let first = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            repo.clone(),
            vec![input.clone(), output.clone()],
        ))?;
        let replay = parse_otlp_protobuf_usage_metrics(&codex_histogram_payload_with_points(
            repo.clone(),
            vec![output, input],
        ))?;

        store.ingest_usage(&first)?;
        store.ingest_usage(&replay)?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
                output_tokens: 450,
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
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(9_750_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn token_rollups_use_metrics_as_authoritative_when_logs_overlap()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            token_usage_record_from_signal(
                TokenUsageSignal::Logs,
                repo.clone(),
                "gpt-5.4",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_799_000,
                1200,
            ),
            token_usage_record_from_signal(
                TokenUsageSignal::Metrics,
                repo.clone(),
                "gpt-5.4",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_799_000,
                1200,
            ),
        ]))?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
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
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(3_000_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn parsed_metric_and_log_token_overlap_counts_once() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let metrics = parse_otlp_json_usage_metrics(
            br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "startTimeUnixNano": "1781956799000000000",
                                "asInt": "1200",
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token.type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#,
        )?;
        let logs = parse_otlp_json_usage_logs(
            br#"{
            "resourceLogs": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "token_usage",
                        "body": { "intValue": "1200" },
                        "attributes": [
                            { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                            { "key": "token.type", "value": { "stringValue": "input" } }
                        ]
                    }]
                }]
            }]
        }"#,
        )?;

        store.ingest_usage(&logs)?;
        store.ingest_usage(&metrics)?;

        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));
        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 1200,
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
                repo,
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(3_000_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn token_rollups_prefer_metrics_when_later_opencode_trace_metrics_overlap()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: vec![
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Input,
                    1_781_956_800_000,
                    1_781_956_700_000,
                    100,
                ),
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Output,
                    1_781_956_800_000,
                    1_781_956_700_000,
                    50,
                ),
            ],
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;
        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record_for_repo(
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Input,
                1_781_956_900_000,
                1_781_956_650_000,
                100,
            ),
            usage_record_for_repo(
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Output,
                1_781_956_900_000,
                1_781_956_650_000,
                50,
            ),
        ]))?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 100,
                output_tokens: 50,
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
                repo,
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(600_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn token_rollups_keep_non_overlapping_opencode_and_metric_usage()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: vec![token_usage_record_from_signal(
                TokenUsageSignal::OpenCodeTraces,
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Input,
                1_781_956_800_000,
                1_781_956_700_000,
                100,
            )],
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;
        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record_for_repo(
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Input,
                1_781_956_900_000,
                1_781_956_650_000,
                200,
            ),
        ]))?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 300,
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
                repo,
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(600_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn later_metric_supersedes_only_one_matching_opencode_trace_delta()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords {
            token_usage: vec![
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Input,
                    1_781_956_800_000,
                    1_781_956_700_000,
                    100,
                ),
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Input,
                    1_781_956_810_000,
                    1_781_956_710_000,
                    100,
                ),
            ],
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;
        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record_for_repo(
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Input,
                1_781_956_900_000,
                1_781_956_650_000,
                100,
            ),
        ]))?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 200,
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
                repo,
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(400_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn earlier_metric_supersedes_only_one_later_matching_opencode_trace_delta()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let repo = kvasir_repo("/repos/kvasir");

        store.ingest_usage(&UsageRecords::from_token_usage(vec![
            usage_record_for_repo(
                repo.clone(),
                "gpt-4.1",
                TokenMeasure::Input,
                1_781_956_700_000,
                1_781_956_650_000,
                100,
            ),
        ]))?;
        store.ingest_usage(&UsageRecords {
            token_usage: vec![
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Input,
                    1_781_956_800_000,
                    1_781_956_700_000,
                    100,
                ),
                token_usage_record_from_signal(
                    TokenUsageSignal::OpenCodeTraces,
                    repo.clone(),
                    "gpt-4.1",
                    TokenMeasure::Input,
                    1_781_956_810_000,
                    1_781_956_710_000,
                    100,
                ),
            ],
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
            content: Vec::new(),
        })?;

        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: repo.clone(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 200,
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
                repo,
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(400_000).unwrap(),
                source: CostSource::Estimated,
            }]
        );

        Ok(())
    }

    #[test]
    fn non_authoritative_token_details_are_delta_normalized()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let records = parse_otlp_json_usage_logs(
            br#"{
            "resourceLogs": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [
                        {
                            "timeUnixNano": "1781956800000000000",
                            "eventName": "token_usage",
                            "body": { "intValue": "100" },
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                { "key": "token.type", "value": { "stringValue": "input" } },
                                { "key": "counter_start_unix_nano", "value": { "stringValue": "1781956799000000000" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781956801000000000",
                            "eventName": "token_usage",
                            "body": { "intValue": "120" },
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                { "key": "token.type", "value": { "stringValue": "input" } },
                                { "key": "counter_start_unix_nano", "value": { "stringValue": "1781956799000000000" } }
                            ]
                        }
                    ]
                }]
            }]
        }"#,
        )?;

        store.ingest_usage(&records)?;

        let retained_log_counts = store
            .connection
            .prepare(
                "SELECT token_count
                 FROM canonical_token_usage
                 WHERE token_signal = 'logs'
                 ORDER BY occurred_at_ms",
            )?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(retained_log_counts, vec![100, 20]);
        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            Vec::new()
        );

        Ok(())
    }

    #[test]
    fn log_token_usage_without_counter_start_replays_as_idempotent_delta()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = open_test_store(temp.path().join("usage.sqlite3"))?;
        let records = parse_otlp_json_usage_logs(
            br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "token_usage",
                        "body": { "intValue": "100" },
                        "attributes": [
                            { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                            { "key": "token.type", "value": { "stringValue": "input" } }
                        ]
                    }]
                }]
            }]
        }"#,
        )?;

        store.ingest_usage(&records)?;
        store.ingest_usage(&records)?;

        let retained_log_counts = store
            .connection
            .prepare(
                "SELECT token_count
                 FROM canonical_token_usage
                 WHERE token_signal = 'logs'
                 ORDER BY occurred_at_ms",
            )?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(retained_log_counts, vec![100]);

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
    fn opening_v4_schema_adds_trace_span_table() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let mut store = open_test_store(&database_path)?;
        store.ingest_token_usage(&[usage_record(
            "claude-opus-4-20250514",
            TokenMeasure::Input,
            1_781_956_800_000,
            1_781_956_700_000,
            100,
        )])?;
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "DROP TABLE canonical_trace_spans;
            PRAGMA user_version = 4;",
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
        let trace_query = TraceQuery {
            harness: HarnessName::new("claude"),
            session_id: crate::rpc::SessionId::new("session-12"),
            prompt_id: crate::rpc::PromptId::new("prompt-7"),
        };
        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: vec![crate::usage::TraceSpanRecord {
                harness: HarnessName::new("claude"),
                session_id: trace_query.session_id.clone(),
                prompt_id: trace_query.prompt_id.clone(),
                trace_id: TraceId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                span_id: SpanId::new("1111111111111111"),
                parent_span_id: None,
                kind: TraceSpanKind::Interaction,
                name: SpanName::new("claude.interaction"),
                started_at: TimestampMillis::new_for_test(1_781_956_800_000),
                ended_at: TimestampMillis::new_for_test(1_781_956_801_000),
                duration_ms: 1_000,
                tool_name: None,
            }],
            content: Vec::new(),
        })?;
        assert_eq!(store.traces(trace_query)?.len(), 1);
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        let token_signal: String = connection.query_row(
            "SELECT token_signal FROM cumulative_counter_snapshots",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(token_signal, "metrics");

        Ok(())
    }

    #[test]
    fn opening_v5_schema_adds_token_signal_to_token_tables()
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

            INSERT INTO canonical_token_usage (
                occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, measure, token_count
            ) VALUES (
                1781956800000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'gpt-5.4', 'input', 100
            );

            INSERT INTO cumulative_counter_snapshots (
                repo_bucket, repo_name, repo_path, model, measure, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (
                'repo', 'kvasir', '/not/persisted', 'gpt-5.4', 'input',
                1781956700000, 1781956800000, 100
            );

            PRAGMA user_version = 5;",
        )?;
        drop(connection);

        let store = open_test_store(&database_path)?;
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        let canonical_signal: String = connection.query_row(
            "SELECT token_signal FROM canonical_token_usage",
            [],
            |row| row.get(0),
        )?;
        let snapshot_signal: String = connection.query_row(
            "SELECT token_signal FROM cumulative_counter_snapshots",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(canonical_signal, "metrics");
        assert_eq!(snapshot_signal, "metrics");

        Ok(())
    }

    #[test]
    fn opening_v7_schema_adds_tool_call_count_storage() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL
            );

            INSERT INTO canonical_tool_calls (
                event_key,
                occurred_at_ms,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name
            ) VALUES (
                'legacy-tool-call',
                1781956800000,
                '2026-06-20',
                'repo',
                'kvasir',
                '/not/persisted',
                'claude_code',
                'Read'
            );

            PRAGMA user_version = 7;",
        )?;
        drop(connection);

        let mut store = open_test_store(&database_path)?;

        assert_eq!(
            store.tool_call_rollups(ToolCallRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                harness: HarnessName::new("claude_code"),
                tool_name: ToolName::new("Read"),
                call_count: 1,
            }]
        );
        store.ingest_usage(&parse_otlp_json_usage_metrics(
            copilot_cumulative_tool_call_metric_json_payload("Read", 3, 5).as_bytes(),
        )?)?;
        assert_eq!(
            store.tool_call_rollups(
                ToolCallRollupQuery::new(
                    TimestampMillis::new_for_test(1_781_956_000_000),
                    TimestampMillis::new_for_test(1_781_970_000_000),
                )
                .with_repo(kvasir_repo("/repos/kvasir"))
            )?,
            vec![ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/repos/kvasir"),
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 5,
            }]
        );
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        let call_count_column_count: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM pragma_table_info('canonical_tool_calls')
             WHERE name = 'call_count'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(call_count_column_count, 1);
        let snapshot_table_count: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM sqlite_master
             WHERE type = 'table' AND name = 'tool_call_counter_snapshots'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(snapshot_table_count, 1);

        Ok(())
    }

    #[test]
    fn opening_v8_schema_discards_unlinked_content_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_content_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content TEXT NOT NULL
            );

            INSERT INTO canonical_content_records (
                event_key,
                occurred_at_ms,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                content_kind,
                content
            ) VALUES (
                'legacy-content',
                1781956800000,
                '2026-06-20',
                'repo',
                'kvasir',
                '/not/persisted',
                'opencode',
                'assistant_message',
                'legacy unlinked text'
            );

            PRAGMA user_version = 8;",
        )?;
        drop(connection);

        let store = open_test_store(&database_path)?;
        assert_eq!(
            store.content_replay(ContentQuery {
                harness: HarnessName::new("opencode"),
                session_id: crate::rpc::SessionId::new("session-12"),
                prompt_id: crate::rpc::PromptId::new("prompt-7"),
            })?,
            ContentReplay {
                session_id: crate::rpc::SessionId::new("session-12"),
                prompt_id: crate::rpc::PromptId::new("prompt-7"),
                items: Vec::new(),
                availability: ContentAvailability::Unavailable {
                    reason: ContentUnavailableReason::PromptNotFound,
                },
            }
        );
        drop(store);

        let connection = open_raw_test_connection(database_path)?;
        let content_row_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM canonical_content_records",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(content_row_count, 0);

        Ok(())
    }

    #[test]
    fn opening_v9_schema_adds_trace_harness_scope() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_trace_spans (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL,
                parent_span_id TEXT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                tool_name TEXT,
                UNIQUE(session_id, prompt_id, trace_id, span_id)
            );

            INSERT INTO canonical_trace_spans (
                session_id,
                prompt_id,
                trace_id,
                span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms
            ) VALUES (
                'session-12',
                'prompt-7',
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                '1111111111111111',
                'interaction',
                'claude.interaction',
                1781956800000,
                1781956801000,
                1000
            );

            PRAGMA user_version = 9;",
        )?;
        drop(connection);

        let store = open_test_store(&database_path)?;
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        let harness_column_count: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM pragma_table_info('canonical_trace_spans')
             WHERE name = 'harness'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(harness_column_count, 1);
        let harness: String =
            connection.query_row("SELECT harness FROM canonical_trace_spans", [], |row| {
                row.get(0)
            })?;
        assert_eq!(harness, "unknown");
        let user_version: i64 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(user_version, CURRENT_SCHEMA_VERSION);
        connection.execute(
            "INSERT INTO canonical_trace_spans (
                harness,
                session_id,
                prompt_id,
                trace_id,
                span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "codex",
                "session-12",
                "prompt-7",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "1111111111111111",
                "interaction",
                "codex.interaction",
                1_781_956_801_000_i64,
                1_781_956_802_000_i64,
                1_000_i64,
            ],
        )?;

        Ok(())
    }

    #[test]
    fn opening_v4_schema_chains_through_token_signal_migration()
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

            CREATE TABLE token_delta_events (
                event_key TEXT PRIMARY KEY
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

            CREATE TABLE canonical_tool_calls (
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

            CREATE TABLE tool_call_rollups (
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                call_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, harness, tool_name)
            );

            INSERT INTO canonical_token_usage (
                occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, measure, token_count
            ) VALUES (
                1781956800000, '2026-06-20', 'repo', 'kvasir', '/not/persisted',
                'gpt-5.4', 'input', 100
            );

            INSERT INTO cumulative_counter_snapshots (
                repo_bucket, repo_name, repo_path, model, measure, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (
                'repo', 'kvasir', '/not/persisted', 'gpt-5.4', 'input',
                1781956700000, 1781956800000, 100
            );

            PRAGMA user_version = 4;",
        )?;
        drop(connection);

        let store = open_test_store(&database_path)?;
        assert_eq!(
            store.token_rollups(RollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            ))?,
            vec![TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo("/not/persisted"),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            }]
        );
        drop(store);

        let connection = open_raw_test_connection(&database_path)?;
        let snapshot_signal: String = connection.query_row(
            "SELECT token_signal FROM cumulative_counter_snapshots",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(snapshot_signal, "metrics");

        Ok(())
    }

    #[test]
    fn opening_v10_schema_canonicalizes_persisted_harnesses()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = open_raw_test_connection(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE canonical_trace_spans (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness TEXT NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                span_id TEXT NOT NULL,
                parent_span_id TEXT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                tool_name TEXT,
                UNIQUE(harness, session_id, prompt_id, trace_id, span_id)
            );

            CREATE TABLE canonical_content_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_key TEXT NOT NULL UNIQUE,
                occurred_at_ms INTEGER NOT NULL,
                session_id TEXT NOT NULL,
                prompt_id TEXT NOT NULL,
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                harness TEXT NOT NULL,
                content_kind TEXT NOT NULL,
                content TEXT NOT NULL
            );

            CREATE TABLE canonical_tool_calls (
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

            INSERT INTO canonical_trace_spans (
                harness,
                session_id,
                prompt_id,
                trace_id,
                span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms
            ) VALUES (
                'GitHub-Copilot',
                'session-12',
                'prompt-7',
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                '1111111111111111',
                'interaction',
                'github.copilot.interaction',
                1781956800000,
                1781956801000,
                1000
            );

            INSERT INTO canonical_content_records (
                event_key,
                occurred_at_ms,
                session_id,
                prompt_id,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                content_kind,
                content
            ) VALUES
            (
                'otlp-log-content
repo_bucket=repo
repo_name=kvasir
repo_path=/repos/kvasir
harness=GitHub-Copilot
session_id=session-12
prompt_id=prompt-7
kind=assistant_message
occurred_at_nanos=1781956802000000000
content_len=14
content_fingerprint=legacy-fingerprint
',
                1781956802000,
                'session-12',
                'prompt-7',
                '2026-06-20',
                'repo',
                'kvasir',
                '/repos/kvasir',
                'GitHub-Copilot',
                'assistant_message',
                'legacy content'
            ),
            (
                'otlp-log-content
repo_bucket=repo
repo_name=kvasir
repo_path=/repos/kvasir
harness=github_copilot
session_id=session-12
prompt_id=prompt-7
kind=assistant_message
occurred_at_nanos=1781956802000000000
content_len=14
content_fingerprint=legacy-fingerprint
',
                1781956802000,
                'session-12',
                'prompt-7',
                '2026-06-20',
                'repo',
                'kvasir',
                '/repos/kvasir',
                'github_copilot',
                'assistant_message',
                'legacy content'
            );

            INSERT INTO canonical_tool_calls (
                event_key,
                occurred_at_ms,
                day,
                repo_bucket,
                repo_name,
                repo_path,
                harness,
                tool_name,
                call_count
            ) VALUES
            (
                'otlp-log-tool-result
repo_bucket=repo
repo_name=kvasir
repo_path=/repos/kvasir
harness=GitHub-Copilot
tool_name=Read
occurred_at_nanos=1781956803000000000
',
                1781956803000,
                '2026-06-20',
                'repo',
                'kvasir',
                '/repos/kvasir',
                'GitHub-Copilot',
                'Read',
                1
            ),
            (
                'otlp-log-tool-result
repo_bucket=repo
repo_name=kvasir
repo_path=/repos/kvasir
harness=github_copilot
tool_name=Read
occurred_at_nanos=1781956803000000000
',
                1781956803000,
                '2026-06-20',
                'repo',
                'kvasir',
                '/repos/kvasir',
                'github_copilot',
                'Read',
                1
            );

            PRAGMA user_version = 10;",
        )?;
        drop(connection);

        let mut store = open_test_store(&database_path)?;
        let query = TraceQuery {
            harness: HarnessName::new("github-copilot"),
            session_id: crate::rpc::SessionId::new("session-12"),
            prompt_id: crate::rpc::PromptId::new("prompt-7"),
        };

        assert_eq!(store.traces(query)?.len(), 1);
        let replay = store.content_replay(ContentQuery {
            harness: HarnessName::new("github-copilot"),
            session_id: crate::rpc::SessionId::new("session-12"),
            prompt_id: crate::rpc::PromptId::new("prompt-7"),
        })?;
        assert_eq!(replay.items.len(), 1);
        assert_eq!(replay.items[0].harness, HarnessName::new("github_copilot"));
        assert_eq!(replay.items[0].content.as_str(), "legacy content");
        let content_row_count: i64 = store.connection.query_row(
            "SELECT COUNT(*) FROM canonical_content_records",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(content_row_count, 1);
        let content_event_key: String = store.connection.query_row(
            "SELECT event_key FROM canonical_content_records",
            [],
            |row| row.get(0),
        )?;
        assert!(content_event_key.contains("harness=github_copilot\n"));
        assert!(!content_event_key.contains("harness=GitHub-Copilot\n"));
        let tool_call_row_count: i64 =
            store
                .connection
                .query_row("SELECT COUNT(*) FROM canonical_tool_calls", [], |row| {
                    row.get(0)
                })?;
        assert_eq!(tool_call_row_count, 1);
        let tool_call_event_key: String = store.connection.query_row(
            "SELECT event_key FROM canonical_tool_calls",
            [],
            |row| row.get(0),
        )?;
        assert!(tool_call_event_key.contains("harness=github_copilot\n"));
        assert!(!tool_call_event_key.contains("harness=GitHub-Copilot\n"));

        store.ingest_usage(&UsageRecords {
            token_usage: Vec::new(),
            cost_usage: Vec::new(),
            tool_calls: vec![ToolCallRecord::new_counted(
                ToolCallEventKey::new(tool_call_event_key),
                TimestampMillis::new_for_test(1_781_956_803_000),
                kvasir_repo("/repos/kvasir"),
                HarnessName::new("github-copilot"),
                ToolName::new("Read"),
                ToolCallCount::new(1),
            )],
            trace_spans: Vec::new(),
            content: vec![ContentRecord {
                event_key: ContentEventKey::new(content_event_key),
                occurred_at: TimestampMillis::new_for_test(1_781_956_802_000),
                session_id: crate::rpc::SessionId::new("session-12"),
                prompt_id: crate::rpc::PromptId::new("prompt-7"),
                repo: kvasir_repo("/repos/kvasir"),
                harness: HarnessName::new("github-copilot"),
                kind: ContentKind::AssistantMessage,
                content: ContentText::new("legacy content").unwrap(),
            }],
        })?;
        let content_row_count_after_reingest: i64 = store.connection.query_row(
            "SELECT COUNT(*) FROM canonical_content_records",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(content_row_count_after_reingest, 1);
        let tool_call_row_count_after_reingest: i64 =
            store
                .connection
                .query_row("SELECT COUNT(*) FROM canonical_tool_calls", [], |row| {
                    row.get(0)
                })?;
        assert_eq!(tool_call_row_count_after_reingest, 1);

        Ok(())
    }

    #[test]
    fn trace_query_reports_kind_conversion_failure_on_selected_kind_column()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let store = open_test_store(&database_path)?;
        store.connection.execute(
            "INSERT INTO canonical_trace_spans (
                harness,
                session_id,
                prompt_id,
                trace_id,
                span_id,
                kind,
                name,
                started_at_ms,
                ended_at_ms,
                duration_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "claude",
                "session-12",
                "prompt-7",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "1111111111111111",
                "not-a-kind",
                "claude.unknown",
                1_781_956_800_000_i64,
                1_781_956_801_000_i64,
                1_000_i64,
            ],
        )?;

        let error = store
            .traces(TraceQuery {
                harness: HarnessName::new("claude"),
                session_id: crate::rpc::SessionId::new("session-12"),
                prompt_id: crate::rpc::PromptId::new("prompt-7"),
            })
            .expect_err("invalid stored trace kind should fail conversion");

        assert!(
            matches!(
                error,
                StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    _
                ))
            ),
            "{error:?}"
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
            INSERT INTO keep_me (value) VALUES ('still here');",
        )?;
        connection.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)?;
        drop(connection);

        let result = UsageStore::open(&database_path, &test_store_key());

        assert!(matches!(
            result,
            Err(StoreError::IncompatibleSchema {
                found,
                supported: CURRENT_SCHEMA_VERSION,
            }) if found == CURRENT_SCHEMA_VERSION + 1
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

    fn token_usage_record_from_signal(
        signal: TokenUsageSignal,
        repo: RepoBucket,
        model: &str,
        measure: TokenMeasure,
        occurred_at_ms: i64,
        counter_start_ms: i64,
        token_count: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord::new_from_signal(
            signal,
            TimestampMillis::new_for_test(occurred_at_ms),
            TimestampMillis::new_for_test(counter_start_ms),
            repo,
            ModelName::new(model),
            measure,
            TokenCount::new(token_count),
        )
    }

    fn codex_delta_record(
        event_key: &str,
        model: &str,
        measure: TokenMeasure,
        token_count: u64,
    ) -> TokenUsageRecord {
        TokenUsageRecord::new_delta(
            TokenUsageEventKey::new(event_key),
            TimestampMillis::new_for_test(1_781_956_800_000),
            TimestampMillis::new_for_test(1_781_956_799_000),
            kvasir_repo("/not/persisted"),
            ModelName::new(model),
            measure,
            TokenCount::new(token_count),
        )
    }

    fn codex_histogram_payload(repo: RepoBucket, points: Vec<(&str, f64)>) -> Vec<u8> {
        codex_histogram_payload_with_points(
            repo,
            points
                .into_iter()
                .map(|(token_type, sum)| {
                    codex_histogram_point("gpt-5.4", token_type, sum, 1_781_956_800_000_000_000)
                })
                .collect(),
        )
    }

    fn codex_histogram_payload_with_points(
        repo: RepoBucket,
        points: Vec<opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint>,
    ) -> Vec<u8> {
        use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
        use opentelemetry_proto::tonic::metrics::v1::{
            Histogram, Metric, ResourceMetrics, ScopeMetrics, metric::Data,
        };
        use opentelemetry_proto::tonic::resource::v1::Resource;
        use prost::Message;

        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: repo_resource_attributes(repo),
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "codex.turn.token_usage".to_owned(),
                        description: String::new(),
                        unit: "{token}".to_owned(),
                        metadata: Vec::new(),
                        data: Some(Data::Histogram(Histogram {
                            data_points: points,
                            aggregation_temporality: 1,
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn codex_split_metric_histogram_payload(
        repo: RepoBucket,
        first: opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint,
        second: opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint,
    ) -> Vec<u8> {
        use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
        use opentelemetry_proto::tonic::metrics::v1::{
            Histogram, Metric, ResourceMetrics, ScopeMetrics, metric::Data,
        };
        use opentelemetry_proto::tonic::resource::v1::Resource;
        use prost::Message;

        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: repo_resource_attributes(repo),
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![
                        Metric {
                            name: "codex.turn.token_usage".to_owned(),
                            description: String::new(),
                            unit: "{token}".to_owned(),
                            metadata: Vec::new(),
                            data: Some(Data::Histogram(Histogram {
                                data_points: vec![first],
                                aggregation_temporality: 1,
                            })),
                        },
                        Metric {
                            name: "codex.turn.token_usage".to_owned(),
                            description: String::new(),
                            unit: "{token}".to_owned(),
                            metadata: Vec::new(),
                            data: Some(Data::Histogram(Histogram {
                                data_points: vec![second],
                                aggregation_temporality: 1,
                            })),
                        },
                    ],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn codex_histogram_point(
        model: &str,
        token_type: &str,
        sum: f64,
        time_unix_nano: u64,
    ) -> opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint {
        opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint {
            attributes: vec![
                string_proto_attribute("model", model),
                string_proto_attribute("token_type", token_type),
            ],
            start_time_unix_nano: time_unix_nano.saturating_sub(1_000_000_000),
            time_unix_nano,
            count: 1,
            sum: Some(sum),
            bucket_counts: Vec::new(),
            explicit_bounds: Vec::new(),
            exemplars: Vec::new(),
            flags: 0,
            min: None,
            max: None,
        }
    }

    fn repo_resource_attributes(
        repo: RepoBucket,
    ) -> Vec<opentelemetry_proto::tonic::common::v1::KeyValue> {
        match repo {
            RepoBucket::NoRepo => Vec::new(),
            RepoBucket::Repo(identity) => {
                let mut attributes = Vec::new();
                if let Some(name) = identity.name {
                    attributes.push(string_proto_attribute("repo.name", name.as_str()));
                }
                if let Some(path) = identity.path {
                    attributes.push(string_proto_attribute("repo.path", path.as_str()));
                }
                attributes
            }
        }
    }

    fn string_proto_attribute(
        key: &str,
        value: &str,
    ) -> opentelemetry_proto::tonic::common::v1::KeyValue {
        opentelemetry_proto::tonic::common::v1::KeyValue {
            key: key.to_owned(),
            key_strindex: 0,
            value: Some(opentelemetry_proto::tonic::common::v1::AnyValue {
                value: Some(
                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                        value.to_owned(),
                    ),
                ),
            }),
        }
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

    fn codex_tool_call_metric_json_payload(tool_name: &str, count: u64) -> String {
        format!(
            r#"{{
                "resourceMetrics": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "codex" }} }},
                            {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                            {{ "key": "repo.path", "value": {{ "stringValue": "/repos/kvasir" }} }}
                        ]
                    }},
                    "scopeMetrics": [{{
                        "metrics": [{{
                            "name": "codex.turn.tool.call",
                            "histogram": {{
                                "aggregationTemporality": 1,
                                "dataPoints": [{{
                                    "startTimeUnixNano": "1781956799000000000",
                                    "timeUnixNano": "1781956800000000000",
                                    "count": "1",
                                    "sum": {count},
                                    "attributes": [
                                        {{ "key": "tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                    ]
                                }}]
                            }}
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn copilot_tool_call_metric_json_payload(tool_name: &str, count: u64) -> String {
        copilot_tool_call_metric_json_payload_at(tool_name, count, "1781956800000000000")
    }

    fn copilot_tool_call_metric_json_payload_at(
        tool_name: &str,
        count: u64,
        time_unix_nano: &str,
    ) -> String {
        copilot_tool_call_metric_json_payload_with_counter_start(
            tool_name,
            count,
            "1781956700000000000",
            time_unix_nano,
        )
    }

    fn copilot_tool_call_metric_json_payload_with_counter_start(
        tool_name: &str,
        count: u64,
        start_time_unix_nano: &str,
        time_unix_nano: &str,
    ) -> String {
        format!(
            r#"{{
                "resourceMetrics": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "github-copilot" }} }},
                            {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                            {{ "key": "repo.path", "value": {{ "stringValue": "/repos/kvasir" }} }}
                        ]
                    }},
                    "scopeMetrics": [{{
                        "metrics": [{{
                            "name": "github.copilot.chat.tool_calls",
                            "sum": {{
                                "aggregationTemporality": 2,
                                "isMonotonic": true,
                                "dataPoints": [{{
                                    "startTimeUnixNano": "{start_time_unix_nano}",
                                    "timeUnixNano": "{time_unix_nano}",
                                    "asInt": "{count}",
                                    "attributes": [
                                        {{ "key": "gen_ai.tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                    ]
                                }}]
                            }}
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn copilot_cumulative_tool_call_metric_json_payload(
        tool_name: &str,
        first_count: u64,
        second_count: u64,
    ) -> String {
        format!(
            r#"{{
                "resourceMetrics": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "github-copilot" }} }},
                            {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                            {{ "key": "repo.path", "value": {{ "stringValue": "/repos/kvasir" }} }}
                        ]
                    }},
                    "scopeMetrics": [{{
                        "metrics": [{{
                            "name": "github.copilot.chat.tool_calls",
                            "sum": {{
                                "aggregationTemporality": 2,
                                "isMonotonic": true,
                                "dataPoints": [
                                    {{
                                        "startTimeUnixNano": "1781956700000000000",
                                        "timeUnixNano": "1781956800000000000",
                                        "asInt": "{first_count}",
                                        "attributes": [
                                            {{ "key": "gen_ai.tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }},
                                    {{
                                        "startTimeUnixNano": "1781956700000000000",
                                        "timeUnixNano": "1781956900000000000",
                                        "asInt": "{second_count}",
                                        "attributes": [
                                            {{ "key": "gen_ai.tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }}
                                ]
                            }}
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn codex_two_point_tool_call_metric_json_payload(
        tool_name: &str,
        first_count: u64,
        second_count: u64,
    ) -> String {
        format!(
            r#"{{
                "resourceMetrics": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "codex" }} }},
                            {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                            {{ "key": "repo.path", "value": {{ "stringValue": "/repos/kvasir" }} }}
                        ]
                    }},
                    "scopeMetrics": [{{
                        "metrics": [{{
                            "name": "codex.turn.tool.call",
                            "histogram": {{
                                "aggregationTemporality": 1,
                                "dataPoints": [
                                    {{
                                        "startTimeUnixNano": "1781956799000000000",
                                        "timeUnixNano": "1781956800000000000",
                                        "count": "1",
                                        "sum": {first_count},
                                        "attributes": [
                                            {{ "key": "tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }},
                                    {{
                                        "startTimeUnixNano": "1781956799000000000",
                                        "timeUnixNano": "1781956900000000000",
                                        "count": "1",
                                        "sum": {second_count},
                                        "attributes": [
                                            {{ "key": "tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }}
                                ]
                            }}
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn codex_duplicate_tool_call_metric_json_payload(tool_name: &str, count: u64) -> String {
        format!(
            r#"{{
                "resourceMetrics": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "codex" }} }},
                            {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                            {{ "key": "repo.path", "value": {{ "stringValue": "/repos/kvasir" }} }}
                        ]
                    }},
                    "scopeMetrics": [{{
                        "metrics": [{{
                            "name": "codex.turn.tool.call",
                            "histogram": {{
                                "aggregationTemporality": 1,
                                "dataPoints": [
                                    {{
                                        "startTimeUnixNano": "1781956799000000000",
                                        "timeUnixNano": "1781956800000000000",
                                        "count": "1",
                                        "sum": {count},
                                        "attributes": [
                                            {{ "key": "tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }},
                                    {{
                                        "startTimeUnixNano": "1781956799000000000",
                                        "timeUnixNano": "1781956800000000000",
                                        "count": "1",
                                        "sum": {count},
                                        "attributes": [
                                            {{ "key": "tool.name", "value": {{ "stringValue": "{tool_name}" }} }}
                                        ]
                                    }}
                                ]
                            }}
                        }}]
                    }}]
                }}]
            }}"#
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
