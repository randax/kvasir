use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::usage::{CostUsd, RepoBucket};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearerToken(String);

impl std::fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BearerToken(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BearerTokenError {
    #[error("bearer token generation failed")]
    Random,
}

impl BearerToken {
    pub fn generate() -> Result<Self, BearerTokenError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| BearerTokenError::Random)?;

        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(hex_nibble(byte >> 4));
            encoded.push(hex_nibble(byte & 0x0f));
        }
        Ok(Self(encoded))
    }

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessName(String);

impl HarnessName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(value: impl Into<String>) -> Self {
        Self::try_new(value).expect("tool name must be a valid tool identifier")
    }

    pub fn try_new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if is_known_claude_tool_name(&value) || is_valid_mcp_tool_name(&value) {
            Some(Self(value))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptId(String);

impl PromptId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceId(String);

impl TraceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanId(String);

impl SpanId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanName(String);

impl SpanName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_known_claude_tool_name(value: &str) -> bool {
    matches!(
        value,
        "Bash"
            | "BashOutput"
            | "Edit"
            | "ExitPlanMode"
            | "Glob"
            | "Grep"
            | "KillBash"
            | "LS"
            | "MultiEdit"
            | "NotebookEdit"
            | "NotebookRead"
            | "Read"
            | "Task"
            | "TodoWrite"
            | "WebFetch"
            | "WebSearch"
            | "Write"
    )
}

fn hex_nibble(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
}

fn is_valid_mcp_tool_name(value: &str) -> bool {
    value.starts_with("mcp__")
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimestampMillis(i64);

impl TimestampMillis {
    pub fn from_millis(value: i64) -> Self {
        Self(value)
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRollupQuery {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
    pub repo: Option<RepoBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceQuery {
    pub session_id: SessionId,
    pub prompt_id: PromptId,
}

impl ToolCallRollupQuery {
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
    pub source: CostSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRollup {
    pub day: RollupDay,
    pub repo: RepoBucket,
    pub harness: HarnessName,
    pub tool_name: ToolName,
    pub call_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewRollup {
    pub token_rollups: Vec<TokenRollup>,
    pub cost_rollups: Vec<CostRollup>,
    pub tool_call_rollups: Vec<ToolCallRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub session_id: SessionId,
    pub prompt_id: PromptId,
    pub trace_id: TraceId,
    pub spans: Vec<TraceSpan>,
    pub durations: TraceDurationMeasures,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSpan {
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub kind: TraceSpanKind,
    pub name: SpanName,
    pub started_at: TimestampMillis,
    pub ended_at: TimestampMillis,
    pub duration_ms: u64,
    pub tool_name: Option<ToolName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceDurationMeasures {
    pub ttft_ms: Option<u64>,
    pub request_ms: Option<u64>,
    pub tool_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSpanKind {
    Interaction,
    LlmRequest,
    ToolCall,
}

impl TraceSpanKind {
    pub fn from_attribute(value: &str) -> Option<Self> {
        match value {
            "interaction" => Some(Self::Interaction),
            "llm_request" | "llm-request" => Some(Self::LlmRequest),
            "tool" | "tool_call" | "tool-call" => Some(Self::ToolCall),
            _ => None,
        }
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Interaction => "interaction",
            Self::LlmRequest => "llm_request",
            Self::ToolCall => "tool_call",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "interaction" => Some(Self::Interaction),
            "llm_request" => Some(Self::LlmRequest),
            "tool_call" => Some(Self::ToolCall),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Native,
    Estimated,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcRequest {
    TokenRollup { query: RollupQuery },
    OverviewRollup { query: RollupQuery },
    CostRollup { query: CostRollupQuery },
    ToolCallRollup { query: ToolCallRollupQuery },
    Trace { query: TraceQuery },
    SubscribeTokenRollup { query: RollupQuery },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcResponse {
    TokenRollup { rollups: Vec<TokenRollup> },
    OverviewRollup { rollup: OverviewRollup },
    CostRollup { rollups: Vec<CostRollup> },
    ToolCallRollup { rollups: Vec<ToolCallRollup> },
    Trace { traces: Vec<Trace> },
    Error { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcStreamEvent {
    TokenRollup { rollups: Vec<TokenRollup> },
    Error { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    InvalidRequest,
    Internal,
    ResponseTooLarge,
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
                source: CostSource::Native,
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
                        "cost_usd": { "nanos": 100000000u64 },
                        "source": "native"
                    }]
                }
            })
        );

        Ok(())
    }

    #[test]
    fn subscription_rpc_contract_serializes_typed_query_and_stream_event()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = RollupQuery::new(
            TimestampMillis::new_for_test(1_781_956_000_000),
            TimestampMillis::new_for_test(1_781_970_000_000),
        )
        .with_repo(RepoBucket::no_repo());

        assert_eq!(
            serde_json::to_value(RpcRequest::SubscribeTokenRollup { query })?,
            json!({
                "type": "subscribe_token_rollup",
                "payload": {
                    "query": {
                        "start": 1781956000000i64,
                        "end": 1781970000000i64,
                        "repo": { "kind": "no_repo" }
                    }
                }
            })
        );

        assert_eq!(
            serde_json::to_value(RpcStreamEvent::TokenRollup {
                rollups: vec![TokenRollup {
                    day: RollupDay::parse("2026-06-20")?,
                    repo: RepoBucket::no_repo(),
                    model: ModelName::new("claude-opus-4-20250514"),
                    input_tokens: 1100,
                    output_tokens: 500,
                    cache_tokens: 100,
                }],
            })?,
            json!({
                "type": "token_rollup",
                "payload": {
                    "rollups": [{
                        "day": "2026-06-20",
                        "repo": { "kind": "no_repo" },
                        "model": "claude-opus-4-20250514",
                        "input_tokens": 1100u64,
                        "output_tokens": 500u64,
                        "cache_tokens": 100u64
                    }]
                }
            })
        );

        Ok(())
    }
}
