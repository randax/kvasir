use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::usage::{ContentKind, ContentText, CostUsd, RepoBucket};

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct HarnessName(String);

impl HarnessName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(canonical_harness_name(&value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn canonical_harness_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

impl<'de> Deserialize<'de> for HarnessName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(value: impl Into<String>) -> Self {
        Self::try_new(value).expect("tool name must be a valid tool identifier")
    }

    pub fn unknown() -> Self {
        Self("Unknown".to_owned())
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<PromptId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionRoute {
    pub harness: HarnessName,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PromptRoute {
    pub session: SessionRoute,
    pub prompt_id: PromptId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionStatus {
    Direct,
    TraceDerived,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryTotals {
    pub total_tokens: u64,
    pub cost_usd: CostUsd,
    pub cost_source: Option<CostSource>,
    pub tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSummary {
    pub harness: HarnessName,
    pub totals: SummaryTotals,
    pub last_activity: TimestampMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub route: SessionRoute,
    pub totals: SummaryTotals,
    pub attribution_status: AttributionStatus,
    pub last_activity: TimestampMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryPage {
    pub summaries: Vec<SessionSummary>,
    pub more_available: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSummary {
    pub route: PromptRoute,
    pub totals: SummaryTotals,
    pub attribution_status: AttributionStatus,
    pub last_activity: TimestampMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSummaryPage {
    pub summaries: Vec<PromptSummary>,
    pub more_available: u64,
}

impl RollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self {
            start,
            end,
            repo: None,
            harness: None,
            model: None,
            session_id: None,
            prompt_id: None,
        }
    }

    pub fn with_repo(self, repo: RepoBucket) -> Self {
        Self {
            repo: Some(repo),
            ..self
        }
    }

    pub fn with_model(self, model: ModelName) -> Self {
        Self {
            model: Some(model),
            ..self
        }
    }

    pub fn with_harness(self, harness: HarnessName) -> Self {
        Self {
            harness: Some(harness),
            ..self
        }
    }

    pub fn with_session(self, session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..self
        }
    }

    pub fn with_prompt(self, prompt_id: PromptId) -> Self {
        Self {
            prompt_id: Some(prompt_id),
            ..self
        }
    }

    pub fn with_session_route(self, route: SessionRoute) -> Self {
        Self {
            harness: Some(route.harness),
            session_id: Some(route.session_id),
            prompt_id: None,
            ..self
        }
    }

    pub fn has_deep_scope(&self) -> bool {
        self.session_id.is_some() || self.prompt_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostRollupQuery {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
    pub repo: Option<RepoBucket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<PromptId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRollupQuery {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
    pub repo: Option<RepoBucket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<PromptId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceQuery {
    pub harness: HarnessName,
    pub session_id: SessionId,
    pub prompt_id: PromptId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentQuery {
    pub harness: HarnessName,
    pub session_id: SessionId,
    pub prompt_id: PromptId,
}

impl ToolCallRollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self {
            start,
            end,
            repo: None,
            harness: None,
            model: None,
            session_id: None,
            prompt_id: None,
        }
    }

    pub fn with_repo(self, repo: RepoBucket) -> Self {
        Self {
            repo: Some(repo),
            ..self
        }
    }

    pub fn with_model(self, model: ModelName) -> Self {
        Self {
            model: Some(model),
            ..self
        }
    }

    pub fn with_harness(self, harness: HarnessName) -> Self {
        Self {
            harness: Some(harness),
            ..self
        }
    }

    pub fn with_session(self, session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..self
        }
    }

    pub fn with_prompt(self, prompt_id: PromptId) -> Self {
        Self {
            prompt_id: Some(prompt_id),
            ..self
        }
    }

    pub fn has_deep_scope(&self) -> bool {
        self.session_id.is_some() || self.prompt_id.is_some()
    }
}

impl CostRollupQuery {
    pub fn new(start: TimestampMillis, end: TimestampMillis) -> Self {
        Self {
            start,
            end,
            repo: None,
            harness: None,
            model: None,
            session_id: None,
            prompt_id: None,
        }
    }

    pub fn with_repo(self, repo: RepoBucket) -> Self {
        Self {
            repo: Some(repo),
            ..self
        }
    }

    pub fn with_model(self, model: ModelName) -> Self {
        Self {
            model: Some(model),
            ..self
        }
    }

    pub fn with_harness(self, harness: HarnessName) -> Self {
        Self {
            harness: Some(harness),
            ..self
        }
    }

    pub fn with_session(self, session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..self
        }
    }

    pub fn with_prompt(self, prompt_id: PromptId) -> Self {
        Self {
            prompt_id: Some(prompt_id),
            ..self
        }
    }

    pub fn has_deep_scope(&self) -> bool {
        self.session_id.is_some() || self.prompt_id.is_some()
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
    pub harness_summaries: Vec<HarnessSummary>,
    pub session_summaries: Vec<SessionSummary>,
    pub session_summaries_more_available: u64,
    pub prompt_summaries: Vec<PromptSummary>,
    pub prompt_summaries_more_available: u64,
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
pub struct ContentReplay {
    pub session_id: SessionId,
    pub prompt_id: PromptId,
    pub items: Vec<ContentReplayItem>,
    pub availability: ContentAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentReplayItem {
    pub occurred_at: TimestampMillis,
    pub harness: HarnessName,
    pub kind: ContentKind,
    pub content: ContentText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContentAvailability {
    Captured {
        harness: HarnessName,
        kinds: Vec<ContentKindAvailability>,
    },
    Unavailable {
        reason: ContentUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContentKindAvailability {
    Captured {
        kind: ContentKind,
    },
    Unavailable {
        kind: ContentKind,
        reason: ContentUnavailableReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentUnavailableReason {
    NotProvidedByHarness,
    NotCapturedForPrompt,
    PromptNotFound,
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
    TokenRollup {
        query: RollupQuery,
    },
    OverviewRollup {
        query: RollupQuery,
    },
    CostRollup {
        query: CostRollupQuery,
    },
    ToolCallRollup {
        query: ToolCallRollupQuery,
    },
    Trace {
        query: TraceQuery,
    },
    Content {
        query: ContentQuery,
        bearer_token: BearerToken,
    },
    SubscribeTokenRollup {
        query: RollupQuery,
    },
    SubscribeUsageUpdates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcResponse {
    TokenRollup { rollups: Vec<TokenRollup> },
    OverviewRollup { rollup: OverviewRollup },
    CostRollup { rollups: Vec<CostRollup> },
    ToolCallRollup { rollups: Vec<ToolCallRollup> },
    Trace { traces: Vec<Trace> },
    Content { replay: ContentReplay },
    Error { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RpcStreamEvent {
    TokenRollup { rollups: Vec<TokenRollup> },
    UsageUpdate { kind: UsageUpdateKind },
    Error { error: RpcError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageUpdateKind {
    Initial,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcError {
    InvalidRequest,
    Internal,
    ResponseTooLarge,
    Unauthorized,
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

    #[test]
    fn usage_update_subscription_rpc_contract_is_typed() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            serde_json::to_value(RpcRequest::SubscribeUsageUpdates)?,
            json!({
                "type": "subscribe_usage_updates"
            })
        );

        assert_eq!(
            serde_json::to_value(RpcStreamEvent::UsageUpdate {
                kind: UsageUpdateKind::Initial
            })?,
            json!({
                "type": "usage_update",
                "payload": {
                    "kind": "initial"
                }
            })
        );

        assert_eq!(
            serde_json::to_value(RpcStreamEvent::UsageUpdate {
                kind: UsageUpdateKind::Changed
            })?,
            json!({
                "type": "usage_update",
                "payload": {
                    "kind": "changed"
                }
            })
        );

        Ok(())
    }

    #[test]
    fn rpc_request_deserialization_canonicalizes_harness_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let request: RpcRequest = serde_json::from_value(json!({
            "type": "trace",
            "payload": {
                "query": {
                    "harness": " GitHub-Copilot ",
                    "session_id": "session-12",
                    "prompt_id": "prompt-7"
                }
            }
        }))?;

        let RpcRequest::Trace { query } = request else {
            panic!("expected trace request");
        };
        assert_eq!(query.harness, HarnessName::new("github_copilot"));

        Ok(())
    }
}
