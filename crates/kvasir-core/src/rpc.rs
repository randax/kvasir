use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::usage::{CostUsd, RepoBucket};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn authorization_header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelName(String);

impl ModelName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimestampMillis(i64);

impl TimestampMillis {
    pub fn from_datetime(value: DateTime<Utc>) -> Self {
        Self(value.timestamp_millis())
    }

    pub fn from_unix_nanos(value: u64) -> Self {
        Self::try_from_unix_nanos(value).unwrap_or(Self(0))
    }

    pub fn try_from_unix_nanos(value: u64) -> Option<Self> {
        i64::try_from(value / 1_000_000).ok().map(Self)
    }

    pub fn value(self) -> i64 {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(value: i64) -> Self {
        Self(value)
    }

    pub fn day(self) -> RollupDay {
        let datetime = Utc
            .timestamp_millis_opt(self.0)
            .single()
            .unwrap_or_else(|| {
                Utc.timestamp_millis_opt(0)
                    .single()
                    .expect("unix epoch is a valid timestamp")
            });
        RollupDay(datetime.date_naive())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RollupDay(NaiveDate);

impl RollupDay {
    pub fn parse(value: &str) -> Result<Self, chrono::ParseError> {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map(Self)
    }

    pub fn as_date(self) -> NaiveDate {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollupQuery {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
    pub repo: Option<RepoBucket>,
}

impl RollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self {
            start,
            end,
            repo: None,
        }
    }

    pub fn with_repo(self, repo: RepoBucket) -> Self {
        Self {
            repo: Some(repo),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostRollupQuery {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
    pub repo: Option<RepoBucket>,
}

impl CostRollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self {
            start,
            end,
            repo: None,
        }
    }

    pub fn with_repo(self, repo: RepoBucket) -> Self {
        Self {
            repo: Some(repo),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRollup {
    pub day: RollupDay,
    pub repo: RepoBucket,
    pub model: ModelName,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostRollup {
    pub day: RollupDay,
    pub repo: RepoBucket,
    pub model: ModelName,
    pub cost_usd: CostUsd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcRequest {
    TokenRollup { query: RollupQuery },
    CostRollup { query: CostRollupQuery },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcResponse {
    TokenRollup { rollups: Vec<TokenRollup> },
    CostRollup { rollups: Vec<CostRollup> },
    Error { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    InvalidRequest,
    Internal,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::usage::{CostUsd, RepoBucket};

    #[test]
    fn cost_rollup_rpc_contract_serializes_typed_query_and_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = RpcRequest::CostRollup {
            query: CostRollupQuery::new(
                TimestampMillis::new_for_test(1_781_956_000_000),
                TimestampMillis::new_for_test(1_781_970_000_000),
            )
            .with_repo(RepoBucket::no_repo()),
        };

        assert_eq!(
            serde_json::to_value(request)?,
            json!({
                "type": "cost_rollup",
                "payload": {
                    "query": {
                        "start": 1781956000000i64,
                        "end": 1781970000000i64,
                        "repo": { "kind": "no_repo" }
                    }
                }
            })
        );

        let response = RpcResponse::CostRollup {
            rollups: vec![CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: RepoBucket::no_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: CostUsd::from_decimal_str("0.1").unwrap(),
            }],
        };

        assert_eq!(
            serde_json::to_value(response)?,
            json!({
                "type": "cost_rollup",
                "payload": {
                    "rollups": [{
                        "day": "2026-06-20",
                        "repo": { "kind": "no_repo" },
                        "model": "claude-opus-4-20250514",
                        "cost_usd": { "nanos": 100000000u64 }
                    }]
                }
            })
        );

        Ok(())
    }
}
