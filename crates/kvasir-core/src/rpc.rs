use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

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
}

impl RollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRollup {
    pub day: RollupDay,
    pub model: ModelName,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcRequest {
    TokenRollup { query: RollupQuery },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcResponse {
    TokenRollup { rollups: Vec<TokenRollup> },
    Error { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    InvalidRequest,
    Internal,
}
