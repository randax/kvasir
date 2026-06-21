use std::path::Path;

use rusqlite::{Connection, params};

use crate::rpc::{ModelName, RollupDay, RollupQuery, TokenRollup};
use crate::usage::{TokenMeasure, TokenUsageRecord};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite failed")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct UsageStore {
    connection: Connection,
}

impl UsageStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn ingest_token_usage(&mut self, records: &[TokenUsageRecord]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        for record in records {
            let Some(delta) = Self::counter_delta(&transaction, record)? else {
                continue;
            };
            let day = record.occurred_at.day().as_date().to_string();
            transaction.execute(
                "INSERT INTO canonical_token_usage (
                    occurred_at_ms, day, repo_name, model, measure, token_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.occurred_at.value(),
                    day,
                    record.repo.name.as_str(),
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
                    day, model, input_tokens, output_tokens, cache_tokens
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(day, model) DO UPDATE SET
                    input_tokens = input_tokens + excluded.input_tokens,
                    output_tokens = output_tokens + excluded.output_tokens,
                    cache_tokens = cache_tokens + excluded.cache_tokens",
                params![
                    day,
                    record.model.as_str(),
                    input_tokens,
                    output_tokens,
                    cache_tokens
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn token_rollups(&self, query: RollupQuery) -> Result<Vec<TokenRollup>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                day,
                model,
                SUM(CASE WHEN measure = 'input' THEN token_count ELSE 0 END) AS input_tokens,
                SUM(CASE WHEN measure = 'output' THEN token_count ELSE 0 END) AS output_tokens,
                SUM(CASE WHEN measure = 'cache' THEN token_count ELSE 0 END) AS cache_tokens
             FROM canonical_token_usage
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
             GROUP BY day, model
             ORDER BY day, model",
        )?;
        let rows = statement.query_map(params![query.start.value(), query.end.value()], |row| {
            let day: String = row.get(0)?;
            let model: String = row.get(1)?;
            Ok(TokenRollup {
                day: RollupDay::parse(&day).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                model: ModelName::new(model),
                input_tokens: unsigned_token_column(row, 2)?,
                output_tokens: unsigned_token_column(row, 3)?,
                cache_tokens: unsigned_token_column(row, 4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn persisted_daily_token_rollups(&self) -> Result<Vec<TokenRollup>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT day, model, input_tokens, output_tokens, cache_tokens
             FROM token_rollups
             ORDER BY day, model",
        )?;
        let rows = statement.query_map([], |row| {
            let day: String = row.get(0)?;
            let model: String = row.get(1)?;
            Ok(TokenRollup {
                day: RollupDay::parse(&day).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                model: ModelName::new(model),
                input_tokens: unsigned_token_column(row, 2)?,
                output_tokens: unsigned_token_column(row, 3)?,
                cache_tokens: unsigned_token_column(row, 4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS canonical_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at_ms INTEGER NOT NULL,
                day TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                token_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cumulative_counter_snapshots (
                repo_name TEXT NOT NULL,
                model TEXT NOT NULL,
                measure TEXT NOT NULL,
                counter_start_ms INTEGER NOT NULL,
                last_occurred_at_ms INTEGER NOT NULL,
                last_value INTEGER NOT NULL,
                PRIMARY KEY(repo_name, model, measure, counter_start_ms)
            );

            CREATE TABLE IF NOT EXISTS token_rollups (
                day TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_tokens INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(day, model)
            );",
        )?;
        Ok(())
    }

    fn counter_delta(
        transaction: &rusqlite::Transaction<'_>,
        record: &TokenUsageRecord,
    ) -> Result<Option<i64>, StoreError> {
        let current_value = record.token_count.storage_value();
        let previous_value = transaction.query_row(
            "SELECT last_occurred_at_ms, last_value
             FROM cumulative_counter_snapshots
             WHERE repo_name = ?1 AND model = ?2 AND measure = ?3 AND counter_start_ms = ?4",
            params![
                record.repo.name.as_str(),
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
                repo_name, model, measure, counter_start_ms, last_occurred_at_ms, last_value
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(repo_name, model, measure, counter_start_ms) DO UPDATE SET
                last_occurred_at_ms = excluded.last_occurred_at_ms,
                last_value = excluded.last_value",
            params![
                record.repo.name.as_str(),
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::rpc::TimestampMillis;
    use crate::usage::{RepoIdentity, RepoName, RepoPath, TokenCount};

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
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 1100,
                    output_tokens: 500,
                    cache_tokens: 0,
                },
                TokenRollup {
                    day: RollupDay::parse("2026-06-21")?,
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
            model: ModelName::new("claude-opus-4-20250514"),
            input_tokens: 1100,
            output_tokens: 0,
            cache_tokens: 0,
        }];

        assert_eq!(
            store.token_rollups(RollupQuery {
                start: TimestampMillis::new_for_test(1_781_956_000_000),
                end: TimestampMillis::new_for_test(1_781_970_000_000),
            })?,
            expected
        );
        assert_eq!(store.persisted_daily_token_rollups()?, expected);

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
            RepoIdentity::new(RepoName::new("kvasir"), RepoPath::new("/not/persisted")),
            ModelName::new(model),
            measure,
            TokenCount::new(token_count),
        )
    }
}
