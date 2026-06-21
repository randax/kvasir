use std::path::Path;

use rusqlite::{Connection, params};

use crate::rpc::{CostRollup, CostRollupQuery, ModelName, RollupDay, RollupQuery, TokenRollup};
use crate::usage::{
    CostUsageRecord, RepoBucket, RepoIdentity, RepoName, RepoPath, TokenMeasure, TokenUsageRecord,
    UsageRecords,
};

const CURRENT_SCHEMA_VERSION: i64 = 2;
const REPO_BUCKET: &str = "repo";
const NO_REPO_BUCKET: &str = "no_repo";
const NO_REPO_STORAGE_VALUE: &str = "";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite failed")]
    Sqlite(#[from] rusqlite::Error),
    #[error("sqlite schema version {found} is newer than supported version {supported}")]
    IncompatibleSchema { found: i64, supported: i64 },
}

pub struct UsageStore {
    connection: Connection,
}

impl UsageStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn ingest_token_usage(&mut self, records: &[TokenUsageRecord]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        Self::ingest_token_usage_in_transaction(&transaction, records)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn ingest_usage(&mut self, records: &UsageRecords) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        Self::ingest_token_usage_in_transaction(&transaction, &records.token_usage)?;
        Self::ingest_cost_usage_in_transaction(&transaction, &records.cost_usage)?;
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
                SUM(cost_usd_nanos) AS cost_usd_nanos
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
        if schema_version < CURRENT_SCHEMA_VERSION {
            transaction.execute_batch(
                "DROP TABLE IF EXISTS canonical_token_usage;
                DROP TABLE IF EXISTS canonical_cost_usage;
                DROP TABLE IF EXISTS cumulative_counter_snapshots;
                DROP TABLE IF EXISTS cost_counter_snapshots;
                DROP TABLE IF EXISTS token_rollups;
                DROP TABLE IF EXISTS cost_rollups;",
            )?;
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
                day TEXT NOT NULL,
                repo_bucket TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                model TEXT NOT NULL,
                cost_usd_nanos INTEGER NOT NULL
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
                cost_usd_nanos INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, repo_bucket, repo_name, repo_path, model)
            );",
        )?;
        transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    fn ingest_token_usage_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[TokenUsageRecord],
    ) -> Result<(), StoreError> {
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
        }
        Ok(())
    }

    fn ingest_cost_usage_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        records: &[CostUsageRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            let Some(delta) = Self::cost_counter_delta(transaction, record)? else {
                continue;
            };
            let day = record.occurred_at.day().as_date().to_string();
            let stored_repo = StoredRepo::from_bucket(&record.repo);
            transaction.execute(
                "INSERT INTO canonical_cost_usage (
                    occurred_at_ms, day, repo_bucket, repo_name, repo_path, model, cost_usd_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    record.occurred_at.value(),
                    day,
                    stored_repo.bucket,
                    stored_repo.name,
                    stored_repo.path,
                    record.model.as_str(),
                    delta,
                ],
            )?;

            transaction.execute(
                "INSERT INTO cost_rollups (
                    day, repo_bucket, repo_name, repo_path, model, cost_usd_nanos
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(day, repo_bucket, repo_name, repo_path, model) DO UPDATE SET
                    cost_usd_nanos = cost_usd_nanos + excluded.cost_usd_nanos",
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

struct StoredRepo<'a> {
    bucket: &'static str,
    name: &'a str,
    path: &'a str,
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::rpc::TimestampMillis;
    use crate::usage::{CostUsd, RepoIdentity, RepoName, RepoPath, TokenCount};

    #[test]
    fn persists_daily_rollups_from_cumulative_counter_deltas()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let mut store = UsageStore::open(temp.path().join("usage.sqlite3"))?;

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
        let mut store = UsageStore::open(temp.path().join("usage.sqlite3"))?;

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
        let mut store = UsageStore::open(temp.path().join("usage.sqlite3"))?;
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
        let mut store = UsageStore::open(temp.path().join("usage.sqlite3"))?;
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
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("1.5"),
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-sonnet-4-20250514"),
                    cost_usd: cost_usd("0.2"),
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-21")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("0.5"),
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
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: kvasir.clone(),
                    model: ModelName::new("claude-sonnet-4-20250514"),
                    cost_usd: cost_usd("0.2"),
                },
                CostRollup {
                    day: RollupDay::parse("2026-06-21")?,
                    repo: kvasir,
                    model: ModelName::new("claude-opus-4-20250514"),
                    cost_usd: cost_usd("0.5"),
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn opening_old_schema_recreates_repo_aware_tables() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let connection = Connection::open(&database_path)?;
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

        let mut store = UsageStore::open(database_path)?;
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
        let connection = Connection::open(&database_path)?;
        connection.execute_batch(
            "CREATE TABLE keep_me (value TEXT NOT NULL);
            INSERT INTO keep_me (value) VALUES ('still here');
            PRAGMA user_version = 3;",
        )?;
        drop(connection);

        let result = UsageStore::open(&database_path);

        assert!(matches!(
            result,
            Err(StoreError::IncompatibleSchema {
                found: 3,
                supported: CURRENT_SCHEMA_VERSION,
            })
        ));

        let connection = Connection::open(&database_path)?;
        let value: String =
            connection.query_row("SELECT value FROM keep_me", [], |row| row.get(0))?;
        assert_eq!(value, "still here");

        Ok(())
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
