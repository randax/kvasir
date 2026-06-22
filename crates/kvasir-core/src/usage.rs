use serde::{Deserialize, Serialize};

use crate::rpc::{
    HarnessName, ModelName, PromptId, SessionId, SpanId, SpanName, TimestampMillis, ToolName,
    TraceId, TraceSpanKind,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIdentity {
    pub name: Option<RepoName>,
    pub path: Option<RepoPath>,
}

impl RepoIdentity {
    pub fn new(name: RepoName, path: RepoPath) -> Self {
        Self {
            name: Some(name),
            path: Some(path),
        }
    }

    pub fn from_parts(name: Option<RepoName>, path: Option<RepoPath>) -> Option<Self> {
        if name.is_none() && path.is_none() {
            None
        } else {
            Some(Self { name, path })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "identity", rename_all = "snake_case")]
pub enum RepoBucket {
    NoRepo,
    Repo(RepoIdentity),
}

impl RepoBucket {
    pub fn repo(identity: RepoIdentity) -> Self {
        Self::Repo(identity)
    }

    pub fn no_repo() -> Self {
        Self::NoRepo
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenMeasure {
    Input,
    Output,
    Cache,
}

impl TokenMeasure {
    pub fn from_attribute(value: &str) -> Option<Self> {
        match value {
            "input" | "input_tokens" => Some(Self::Input),
            "output" | "output_tokens" => Some(Self::Output),
            "cache" | "cache_tokens" | "cache_read" | "cache_creation" => Some(Self::Cache),
            _ => None,
        }
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Cache => "cache",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenUsageKind {
    Cumulative,
    Delta { event_key: TokenUsageEventKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageEventKey(String);

impl TokenUsageEventKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount(u64);

impl TokenCount {
    pub fn new(value: u64) -> Self {
        Self::try_new(value).expect("token count must fit SQLite integer storage")
    }

    pub fn try_new(value: u64) -> Option<Self> {
        i64::try_from(value).ok().map(|_| Self(value))
    }

    pub fn value(self) -> u64 {
        self.0
    }

    pub fn storage_value(self) -> i64 {
        i64::try_from(self.0).expect("token count is validated before storage")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub occurred_at: TimestampMillis,
    pub counter_start: TimestampMillis,
    pub signal: TokenUsageSignal,
    pub repo: RepoBucket,
    pub model: ModelName,
    pub measure: TokenMeasure,
    pub token_count: TokenCount,
    pub kind: TokenUsageKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenUsageSignal {
    Metrics,
    Logs,
}

impl TokenUsageSignal {
    pub const fn authoritative_for(measure: TokenMeasure) -> Self {
        match measure {
            TokenMeasure::Input | TokenMeasure::Output | TokenMeasure::Cache => Self::Metrics,
        }
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Logs => "logs",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "metrics" => Some(Self::Metrics),
            "logs" => Some(Self::Logs),
            _ => None,
        }
    }

    pub fn is_authoritative_for(self, measure: TokenMeasure) -> bool {
        self == Self::authoritative_for(measure)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub event_key: ToolCallEventKey,
    pub occurred_at: TimestampMillis,
    pub repo: RepoBucket,
    pub harness: HarnessName,
    pub tool_name: ToolName,
}

impl ToolCallRecord {
    pub fn new(
        event_key: ToolCallEventKey,
        occurred_at: TimestampMillis,
        repo: RepoBucket,
        harness: HarnessName,
        tool_name: ToolName,
    ) -> Self {
        Self {
            event_key,
            occurred_at,
            repo,
            harness,
            tool_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallEventKey(String);

impl ToolCallEventKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSpanRecord {
    pub session_id: SessionId,
    pub prompt_id: PromptId,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub kind: TraceSpanKind,
    pub name: SpanName,
    pub started_at: TimestampMillis,
    pub ended_at: TimestampMillis,
    pub duration_ms: u64,
    pub tool_name: Option<ToolName>,
}

const COST_NANOS_PER_USD: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsd {
    nanos: u64,
}

impl CostUsd {
    pub fn from_nanos(nanos: u64) -> Option<Self> {
        i64::try_from(nanos).ok().map(|_| Self { nanos })
    }

    pub fn from_whole_usd(value: u64) -> Option<Self> {
        value
            .checked_mul(COST_NANOS_PER_USD)
            .and_then(Self::from_nanos)
    }

    pub fn from_decimal_str(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.starts_with('-') {
            return None;
        }
        let (mantissa, exponent) = split_decimal_exponent(value)?;
        let mut digits = String::with_capacity(mantissa.len());
        let mut fractional_digits = 0_i32;
        let mut saw_decimal = false;
        for character in mantissa.chars() {
            match character {
                '.' if saw_decimal => return None,
                '.' => saw_decimal = true,
                digit if digit.is_ascii_digit() => {
                    digits.push(digit);
                    if saw_decimal {
                        fractional_digits += 1;
                    }
                }
                _ => return None,
            }
        }
        if digits.is_empty() {
            return None;
        }
        let nanos_scale = 9_i32 + exponent - fractional_digits;
        if nanos_scale < 0 {
            return None;
        }
        let mut nanos = digits.parse::<u64>().ok()?;
        for _ in 0..nanos_scale {
            nanos = nanos.checked_mul(10)?;
        }
        Self::from_nanos(nanos)
    }

    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        let nanos = (value * COST_NANOS_PER_USD as f64).round();
        if nanos < 0.0 || nanos > i64::MAX as f64 {
            return None;
        }
        Self::from_nanos(nanos as u64)
    }

    pub fn from_storage_value(value: i64) -> Option<Self> {
        u64::try_from(value).ok().and_then(Self::from_nanos)
    }

    pub fn as_nanos(self) -> u64 {
        self.nanos
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.nanos
            .checked_add(other.nanos)
            .and_then(Self::from_nanos)
    }

    pub fn checked_mul(self, multiplier: u64) -> Option<Self> {
        self.nanos
            .checked_mul(multiplier)
            .and_then(Self::from_nanos)
    }

    pub fn storage_value(self) -> i64 {
        i64::try_from(self.nanos).expect("cost is validated before storage")
    }
}

fn split_decimal_exponent(value: &str) -> Option<(&str, i32)> {
    let Some((mantissa, exponent)) = value.split_once(['e', 'E']) else {
        return Some((value, 0));
    };
    if mantissa.is_empty() || exponent.is_empty() {
        return None;
    }
    Some((mantissa, exponent.parse::<i32>().ok()?))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostUsageRecord {
    pub occurred_at: TimestampMillis,
    pub counter_start: TimestampMillis,
    pub repo: RepoBucket,
    pub model: ModelName,
    pub cost_usd: CostUsd,
}

impl CostUsageRecord {
    pub fn new(
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        cost_usd: CostUsd,
    ) -> Self {
        Self {
            occurred_at,
            counter_start,
            repo,
            model,
            cost_usd,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageRecords {
    pub token_usage: Vec<TokenUsageRecord>,
    pub cost_usage: Vec<CostUsageRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub trace_spans: Vec<TraceSpanRecord>,
}

impl UsageRecords {
    pub fn from_token_usage(token_usage: Vec<TokenUsageRecord>) -> Self {
        Self {
            token_usage,
            cost_usage: Vec::new(),
            tool_calls: Vec::new(),
            trace_spans: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.token_usage.is_empty()
            && self.cost_usage.is_empty()
            && self.tool_calls.is_empty()
            && self.trace_spans.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.token_usage.extend(other.token_usage);
        self.cost_usage.extend(other.cost_usage);
        self.tool_calls.extend(other.tool_calls);
        self.trace_spans.extend(other.trace_spans);
    }
}

impl TokenUsageRecord {
    pub fn new(
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        measure: TokenMeasure,
        token_count: TokenCount,
    ) -> Self {
        Self::new_from_signal(
            TokenUsageSignal::Metrics,
            occurred_at,
            counter_start,
            repo,
            model,
            measure,
            token_count,
        )
    }

    pub fn new_from_signal(
        signal: TokenUsageSignal,
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        measure: TokenMeasure,
        token_count: TokenCount,
    ) -> Self {
        Self {
            occurred_at,
            counter_start,
            signal,
            repo,
            model,
            measure,
            token_count,
            kind: TokenUsageKind::Cumulative,
        }
    }

    pub fn new_delta(
        event_key: TokenUsageEventKey,
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        measure: TokenMeasure,
        token_count: TokenCount,
    ) -> Self {
        Self {
            occurred_at,
            counter_start,
            signal: TokenUsageSignal::Metrics,
            repo,
            model,
            measure,
            token_count,
            kind: TokenUsageKind::Delta { event_key },
        }
    }

    pub fn new_delta_from_signal(
        signal: TokenUsageSignal,
        event_key: TokenUsageEventKey,
        occurred_at: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        measure: TokenMeasure,
        token_count: TokenCount,
    ) -> Self {
        Self {
            occurred_at,
            counter_start: occurred_at,
            signal,
            repo,
            model,
            measure,
            token_count,
            kind: TokenUsageKind::Delta { event_key },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenMeasure, TokenUsageSignal};

    #[test]
    fn token_authority_is_defined_per_measure() {
        assert_eq!(
            TokenUsageSignal::authoritative_for(TokenMeasure::Input),
            TokenUsageSignal::Metrics
        );
        assert_eq!(
            TokenUsageSignal::authoritative_for(TokenMeasure::Output),
            TokenUsageSignal::Metrics
        );
        assert_eq!(
            TokenUsageSignal::authoritative_for(TokenMeasure::Cache),
            TokenUsageSignal::Metrics
        );
    }
}
