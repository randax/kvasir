use std::collections::HashMap;

use crate::error::KvasirClientError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirSocketPath(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KvasirModelName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirHarnessName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirToolName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirSessionId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirPromptId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirTraceId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirSpanId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirSpanName(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KvasirRepoName(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KvasirRepoPath(String);

#[derive(Clone, PartialEq, Eq)]
pub struct KvasirContentText(String);

#[derive(Clone, PartialEq, Eq)]
pub struct KvasirBearerToken(String);

uniffi::custom_type!(KvasirSocketPath, String);
uniffi::custom_type!(KvasirModelName, String);
uniffi::custom_type!(KvasirHarnessName, String);
uniffi::custom_type!(KvasirToolName, String);
uniffi::custom_type!(KvasirSessionId, String);
uniffi::custom_type!(KvasirPromptId, String);
uniffi::custom_type!(KvasirTraceId, String);
uniffi::custom_type!(KvasirSpanId, String);
uniffi::custom_type!(KvasirSpanName, String);
uniffi::custom_type!(KvasirRepoName, String);
uniffi::custom_type!(KvasirRepoPath, String);
uniffi::custom_type!(KvasirContentText, String);
uniffi::custom_type!(KvasirBearerToken, String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum KvasirRepoBucketKind {
    NoRepo,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTimestampMillis {
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct KvasirRepoBucket {
    pub kind: KvasirRepoBucketKind,
    pub name: Option<KvasirRepoName>,
    pub path: Option<KvasirRepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, uniffi::Record)]
pub struct KvasirRollupDay {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRollupQuery {
    pub start: KvasirTimestampMillis,
    pub end: KvasirTimestampMillis,
    pub repo: Option<KvasirRepoBucket>,
    pub model: Option<KvasirModelName>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTraceQuery {
    pub harness: KvasirHarnessName,
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirContentQuery {
    pub harness: KvasirHarnessName,
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
    pub bearer_token: KvasirBearerToken,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTokenRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub model: KvasirModelName,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTokenRollupUpdate {
    pub rollups: Vec<KvasirTokenRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirCostUsd {
    pub nanos: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirCostSource {
    Native,
    Estimated,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirCostRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub model: KvasirModelName,
    pub cost_usd: KvasirCostUsd,
    pub source: KvasirCostSource,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirToolCallRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub harness: KvasirHarnessName,
    pub tool_name: KvasirToolName,
    pub call_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewRollup {
    pub token_rollups: Vec<KvasirTokenRollup>,
    pub cost_rollups: Vec<KvasirCostRollup>,
    pub tool_call_rollups: Vec<KvasirToolCallRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewTotals {
    pub total_tokens: u64,
    pub cost_usd_nanos: u64,
    pub cost_source: Option<KvasirCostSource>,
    pub tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewSeriesPoint {
    pub day: KvasirRollupDay,
    pub total_tokens: u64,
    pub cost_usd_nanos: u64,
    pub cost_source: Option<KvasirCostSource>,
    pub tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewRepoSummary {
    pub repo: KvasirRepoBucket,
    pub totals: KvasirOverviewTotals,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewModelSummary {
    pub model: KvasirModelName,
    pub totals: KvasirOverviewTotals,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirOverviewSnapshot {
    pub totals: KvasirOverviewTotals,
    pub series: Vec<KvasirOverviewSeriesPoint>,
    pub repo_breakdown: Vec<KvasirOverviewRepoSummary>,
    pub model_breakdown: Vec<KvasirOverviewModelSummary>,
    pub selected_repo: Option<KvasirRepoBucket>,
    pub selected_model: Option<KvasirModelName>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTrace {
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
    pub trace_id: KvasirTraceId,
    pub spans: Vec<KvasirTraceSpan>,
    pub durations: KvasirTraceDurationMeasures,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirContentReplay {
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
    pub items: Vec<KvasirContentReplayItem>,
    pub availability: KvasirContentAvailability,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirContentReplayItem {
    pub occurred_at: KvasirTimestampMillis,
    pub harness: KvasirHarnessName,
    pub kind: KvasirContentKind,
    pub content: KvasirContentText,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirContentAvailability {
    Captured {
        harness: KvasirHarnessName,
        kinds: Vec<KvasirContentKindAvailability>,
    },
    Unavailable {
        reason: KvasirContentUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirContentKindAvailability {
    Captured {
        kind: KvasirContentKind,
    },
    Unavailable {
        kind: KvasirContentKind,
        reason: KvasirContentUnavailableReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirContentUnavailableReason {
    NotProvidedByHarness,
    NotCapturedForPrompt,
    PromptNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirContentKind {
    UserPrompt,
    AssistantMessage,
    ToolInput,
    ToolOutput,
    RawApiRequest,
    RawApiResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTraceSpan {
    pub span_id: KvasirSpanId,
    pub parent_span_id: Option<KvasirSpanId>,
    pub kind: KvasirTraceSpanKind,
    pub name: KvasirSpanName,
    pub started_at: KvasirTimestampMillis,
    pub ended_at: KvasirTimestampMillis,
    pub duration_ms: u64,
    pub tool_name: Option<KvasirToolName>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTraceDurationMeasures {
    pub ttft_ms: Option<u64>,
    pub request_ms: Option<u64>,
    pub tool_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirTraceSpanKind {
    Interaction,
    LlmRequest,
    ToolCall,
}

impl KvasirSocketPath {
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl KvasirModelName {
    pub(crate) fn from_core(value: kvasir_core::rpc::ModelName) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::rpc::ModelName {
        kvasir_core::rpc::ModelName::new(self.0)
    }
}

impl KvasirHarnessName {
    pub(crate) fn from_core(value: kvasir_core::rpc::HarnessName) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::rpc::HarnessName {
        kvasir_core::rpc::HarnessName::new(self.0)
    }
}

impl KvasirToolName {
    pub(crate) fn from_core(value: kvasir_core::rpc::ToolName) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl KvasirSessionId {
    pub(crate) fn from_core(value: kvasir_core::rpc::SessionId) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::rpc::SessionId {
        kvasir_core::rpc::SessionId::new(self.0)
    }
}

impl KvasirPromptId {
    pub(crate) fn from_core(value: kvasir_core::rpc::PromptId) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::rpc::PromptId {
        kvasir_core::rpc::PromptId::new(self.0)
    }
}

impl KvasirTraceId {
    pub(crate) fn from_core(value: kvasir_core::rpc::TraceId) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl KvasirSpanId {
    pub(crate) fn from_core(value: kvasir_core::rpc::SpanId) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl KvasirSpanName {
    pub(crate) fn from_core(value: kvasir_core::rpc::SpanName) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl KvasirRepoName {
    pub(crate) fn from_core(value: kvasir_core::RepoName) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::RepoName {
        kvasir_core::RepoName::new(self.0)
    }
}

impl KvasirRepoPath {
    pub(crate) fn from_core(value: kvasir_core::RepoPath) -> Self {
        Self(value.as_str().to_owned())
    }

    pub(crate) fn into_core(self) -> kvasir_core::RepoPath {
        kvasir_core::RepoPath::new(self.0)
    }
}

impl KvasirContentText {
    pub(crate) fn from_core(value: kvasir_core::ContentText) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl KvasirBearerToken {
    pub(crate) fn into_core(self) -> kvasir_core::rpc::BearerToken {
        kvasir_core::rpc::BearerToken::new(self.0)
    }
}

impl KvasirOverviewSnapshot {
    pub(crate) fn from_rollup(
        rollup: KvasirOverviewRollup,
        selected_repo: Option<KvasirRepoBucket>,
        selected_model: Option<KvasirModelName>,
    ) -> Self {
        let mut totals = KvasirOverviewTotals::zero();
        let mut points_by_day: HashMap<KvasirRollupDay, KvasirOverviewSeriesPoint> = HashMap::new();
        let mut totals_by_repo: HashMap<KvasirRepoBucket, KvasirOverviewTotals> = HashMap::new();
        let mut totals_by_model: HashMap<KvasirModelName, KvasirOverviewTotals> = HashMap::new();

        for token_rollup in rollup.token_rollups {
            let KvasirTokenRollup {
                day,
                repo,
                model,
                input_tokens,
                output_tokens,
                cache_tokens,
            } = token_rollup;
            let tokens = input_tokens
                .saturating_add(output_tokens)
                .saturating_add(cache_tokens);
            totals.total_tokens = totals.total_tokens.saturating_add(tokens);
            let point = points_by_day
                .entry(day.clone())
                .or_insert_with(|| KvasirOverviewSeriesPoint::empty(day));
            point.total_tokens = point.total_tokens.saturating_add(tokens);
            let repo_totals = totals_by_repo
                .entry(repo)
                .or_insert_with(KvasirOverviewTotals::zero);
            repo_totals.total_tokens = repo_totals.total_tokens.saturating_add(tokens);
            let model_totals = totals_by_model
                .entry(model)
                .or_insert_with(KvasirOverviewTotals::zero);
            model_totals.total_tokens = model_totals.total_tokens.saturating_add(tokens);
        }

        for cost_rollup in rollup.cost_rollups {
            let KvasirCostRollup {
                day,
                repo,
                model,
                cost_usd,
                source,
            } = cost_rollup;
            totals.cost_usd_nanos = totals.cost_usd_nanos.saturating_add(cost_usd.nanos);
            totals.cost_source = combined_cost_source(totals.cost_source, source);
            let point = points_by_day
                .entry(day.clone())
                .or_insert_with(|| KvasirOverviewSeriesPoint::empty(day));
            point.cost_usd_nanos = point.cost_usd_nanos.saturating_add(cost_usd.nanos);
            point.cost_source = combined_cost_source(point.cost_source, source);
            let repo_totals = totals_by_repo
                .entry(repo)
                .or_insert_with(KvasirOverviewTotals::zero);
            repo_totals.cost_usd_nanos = repo_totals.cost_usd_nanos.saturating_add(cost_usd.nanos);
            repo_totals.cost_source = combined_cost_source(repo_totals.cost_source, source);
            let model_totals = totals_by_model
                .entry(model)
                .or_insert_with(KvasirOverviewTotals::zero);
            model_totals.cost_usd_nanos =
                model_totals.cost_usd_nanos.saturating_add(cost_usd.nanos);
            model_totals.cost_source = combined_cost_source(model_totals.cost_source, source);
        }

        for tool_call_rollup in rollup.tool_call_rollups {
            totals.tool_calls = totals
                .tool_calls
                .saturating_add(tool_call_rollup.call_count);
            let point = points_by_day
                .entry(tool_call_rollup.day.clone())
                .or_insert_with(|| KvasirOverviewSeriesPoint::empty(tool_call_rollup.day));
            point.tool_calls = point.tool_calls.saturating_add(tool_call_rollup.call_count);
            let repo_totals = totals_by_repo
                .entry(tool_call_rollup.repo)
                .or_insert_with(KvasirOverviewTotals::zero);
            repo_totals.tool_calls = repo_totals
                .tool_calls
                .saturating_add(tool_call_rollup.call_count);
        }

        let mut series = points_by_day.into_values().collect::<Vec<_>>();
        series.sort_by(|lhs, rhs| lhs.day.cmp(&rhs.day));

        let mut repo_breakdown = totals_by_repo
            .into_iter()
            .map(|(repo, totals)| KvasirOverviewRepoSummary { repo, totals })
            .collect::<Vec<_>>();
        repo_breakdown.sort_by(repo_summary_order);

        let mut model_breakdown = totals_by_model
            .into_iter()
            .map(|(model, totals)| KvasirOverviewModelSummary { model, totals })
            .collect::<Vec<_>>();
        model_breakdown.sort_by(model_summary_order);

        Self {
            totals,
            series,
            repo_breakdown,
            model_breakdown,
            selected_repo,
            selected_model,
        }
    }
}

impl KvasirOverviewTotals {
    fn zero() -> Self {
        Self {
            total_tokens: 0,
            cost_usd_nanos: 0,
            cost_source: None,
            tool_calls: 0,
        }
    }
}

impl KvasirOverviewSeriesPoint {
    fn empty(day: KvasirRollupDay) -> Self {
        Self {
            day,
            total_tokens: 0,
            cost_usd_nanos: 0,
            cost_source: None,
            tool_calls: 0,
        }
    }
}

fn combined_cost_source(
    current: Option<KvasirCostSource>,
    next: KvasirCostSource,
) -> Option<KvasirCostSource> {
    Some(match (current, next) {
        (None, source) => source,
        (Some(KvasirCostSource::Mixed), _) | (_, KvasirCostSource::Mixed) => {
            KvasirCostSource::Mixed
        }
        (Some(source), next) if source == next => source,
        (Some(_), _) => KvasirCostSource::Mixed,
    })
}

fn repo_summary_order(
    lhs: &KvasirOverviewRepoSummary,
    rhs: &KvasirOverviewRepoSummary,
) -> std::cmp::Ordering {
    rhs.totals
        .total_tokens
        .cmp(&lhs.totals.total_tokens)
        .then_with(|| rhs.totals.cost_usd_nanos.cmp(&lhs.totals.cost_usd_nanos))
        .then_with(|| rhs.totals.tool_calls.cmp(&lhs.totals.tool_calls))
        .then_with(|| repo_sort_key(&lhs.repo).cmp(&repo_sort_key(&rhs.repo)))
}

fn model_summary_order(
    lhs: &KvasirOverviewModelSummary,
    rhs: &KvasirOverviewModelSummary,
) -> std::cmp::Ordering {
    rhs.totals
        .total_tokens
        .cmp(&lhs.totals.total_tokens)
        .then_with(|| rhs.totals.cost_usd_nanos.cmp(&lhs.totals.cost_usd_nanos))
        .then_with(|| lhs.model.cmp(&rhs.model))
}

fn repo_sort_key(repo: &KvasirRepoBucket) -> (u8, String, String) {
    match repo.kind {
        KvasirRepoBucketKind::NoRepo => (1, String::new(), String::new()),
        KvasirRepoBucketKind::Repo => (
            0,
            repo.name
                .as_ref()
                .map(|name| name.0.clone())
                .unwrap_or_default(),
            repo.path
                .as_ref()
                .map(|path| path.0.clone())
                .unwrap_or_default(),
        ),
    }
}

impl std::fmt::Debug for KvasirContentText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KvasirContentText(<redacted>)")
    }
}

impl std::fmt::Debug for KvasirBearerToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KvasirBearerToken(<redacted>)")
    }
}

impl std::fmt::Debug for KvasirContentQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KvasirContentQuery")
            .field("harness", &self.harness)
            .field("session_id", &self.session_id)
            .field("prompt_id", &self.prompt_id)
            .field("bearer_token", &self.bearer_token)
            .finish()
    }
}

impl std::fmt::Debug for KvasirContentReplay {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KvasirContentReplay")
            .field("session_id", &self.session_id)
            .field("prompt_id", &self.prompt_id)
            .field("items", &self.items)
            .field("availability", &self.availability)
            .finish()
    }
}

impl std::fmt::Debug for KvasirContentReplayItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KvasirContentReplayItem")
            .field("occurred_at", &self.occurred_at)
            .field("harness", &self.harness)
            .field("kind", &self.kind)
            .field("content", &self.content)
            .finish()
    }
}

impl From<KvasirSocketPath> for String {
    fn from(value: KvasirSocketPath) -> Self {
        value.0
    }
}

impl From<KvasirModelName> for String {
    fn from(value: KvasirModelName) -> Self {
        value.0
    }
}

impl From<KvasirHarnessName> for String {
    fn from(value: KvasirHarnessName) -> Self {
        value.0
    }
}

impl From<KvasirToolName> for String {
    fn from(value: KvasirToolName) -> Self {
        value.0
    }
}

impl From<KvasirSessionId> for String {
    fn from(value: KvasirSessionId) -> Self {
        value.0
    }
}

impl From<KvasirPromptId> for String {
    fn from(value: KvasirPromptId) -> Self {
        value.0
    }
}

impl From<KvasirTraceId> for String {
    fn from(value: KvasirTraceId) -> Self {
        value.0
    }
}

impl From<KvasirSpanId> for String {
    fn from(value: KvasirSpanId) -> Self {
        value.0
    }
}

impl From<KvasirSpanName> for String {
    fn from(value: KvasirSpanName) -> Self {
        value.0
    }
}

impl From<KvasirRepoName> for String {
    fn from(value: KvasirRepoName) -> Self {
        value.0
    }
}

impl From<KvasirRepoPath> for String {
    fn from(value: KvasirRepoPath) -> Self {
        value.0
    }
}

impl From<KvasirContentText> for String {
    fn from(value: KvasirContentText) -> Self {
        value.0
    }
}

impl From<KvasirBearerToken> for String {
    fn from(value: KvasirBearerToken) -> Self {
        value.0
    }
}

impl TryFrom<String> for KvasirSocketPath {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirModelName {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirHarnessName {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value)
            .map(|value| kvasir_core::rpc::canonical_harness_name(&value))
            .map(Self)
    }
}

impl TryFrom<String> for KvasirToolName {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if kvasir_core::rpc::ToolName::try_new(&value).is_some() {
            Ok(Self(value))
        } else {
            Err(KvasirClientError::InvalidQuery)
        }
    }
}

impl TryFrom<String> for KvasirSessionId {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirPromptId {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirTraceId {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirSpanId {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirSpanName {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirRepoName {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirRepoPath {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirContentText {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

impl TryFrom<String> for KvasirBearerToken {
    type Error = KvasirClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        nonempty_text(value).map(Self)
    }
}

fn nonempty_text(value: String) -> Result<String, KvasirClientError> {
    if value.trim().is_empty() {
        Err(KvasirClientError::InvalidQuery)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_debug_output_redacts_sensitive_values() -> Result<(), KvasirClientError> {
        let query = KvasirContentQuery {
            harness: KvasirHarnessName::try_from("opencode".to_owned())?,
            session_id: KvasirSessionId::try_from("session-12".to_owned())?,
            prompt_id: KvasirPromptId::try_from("prompt-7".to_owned())?,
            bearer_token: KvasirBearerToken::try_from("secret-token".to_owned())?,
        };
        let replay = KvasirContentReplay {
            session_id: KvasirSessionId::try_from("session-12".to_owned())?,
            prompt_id: KvasirPromptId::try_from("prompt-7".to_owned())?,
            items: vec![KvasirContentReplayItem {
                occurred_at: KvasirTimestampMillis { value: 1 },
                harness: KvasirHarnessName::try_from("opencode".to_owned())?,
                kind: KvasirContentKind::AssistantMessage,
                content: KvasirContentText::try_from("private prompt text".to_owned())?,
            }],
            availability: KvasirContentAvailability::Unavailable {
                reason: KvasirContentUnavailableReason::PromptNotFound,
            },
        };

        let debug_output = format!("{query:?}\n{replay:?}");

        assert!(debug_output.contains("<redacted>"));
        assert!(!debug_output.contains("secret-token"));
        assert!(!debug_output.contains("private prompt text"));

        Ok(())
    }

    #[test]
    fn harness_names_are_canonicalized() -> Result<(), KvasirClientError> {
        let harness = KvasirHarnessName::try_from(" GitHub-Copilot ".to_owned())?;

        assert_eq!(String::from(harness), "github_copilot");

        Ok(())
    }

    #[test]
    fn overview_snapshot_aggregates_rollups_by_day_and_repo() -> Result<(), KvasirClientError> {
        let kvasir_repo = repo("kvasir", "/repos/kvasir")?;
        let other_repo = repo("other", "/repos/other")?;
        let selected_repo = kvasir_repo.clone();
        let snapshot = KvasirOverviewSnapshot::from_rollup(
            KvasirOverviewRollup {
                token_rollups: vec![
                    KvasirTokenRollup {
                        day: day(2026, 6, 20),
                        repo: kvasir_repo.clone(),
                        model: KvasirModelName::try_from("claude-opus-4".to_owned())?,
                        input_tokens: 1_000,
                        output_tokens: 500,
                        cache_tokens: 250,
                    },
                    KvasirTokenRollup {
                        day: day(2026, 6, 20),
                        repo: other_repo.clone(),
                        model: KvasirModelName::try_from("claude-sonnet-4".to_owned())?,
                        input_tokens: 300,
                        output_tokens: 100,
                        cache_tokens: 0,
                    },
                    KvasirTokenRollup {
                        day: day(2026, 6, 21),
                        repo: kvasir_repo.clone(),
                        model: KvasirModelName::try_from("claude-sonnet-4".to_owned())?,
                        input_tokens: 2_000,
                        output_tokens: 800,
                        cache_tokens: 100,
                    },
                ],
                cost_rollups: vec![
                    KvasirCostRollup {
                        day: day(2026, 6, 20),
                        repo: kvasir_repo.clone(),
                        model: KvasirModelName::try_from("claude-opus-4".to_owned())?,
                        cost_usd: KvasirCostUsd {
                            nanos: 1_250_000_000,
                        },
                        source: KvasirCostSource::Native,
                    },
                    KvasirCostRollup {
                        day: day(2026, 6, 20),
                        repo: other_repo.clone(),
                        model: KvasirModelName::try_from("claude-sonnet-4".to_owned())?,
                        cost_usd: KvasirCostUsd { nanos: 75_000_000 },
                        source: KvasirCostSource::Estimated,
                    },
                    KvasirCostRollup {
                        day: day(2026, 6, 21),
                        repo: kvasir_repo.clone(),
                        model: KvasirModelName::try_from("claude-sonnet-4".to_owned())?,
                        cost_usd: KvasirCostUsd {
                            nanos: 2_000_000_000,
                        },
                        source: KvasirCostSource::Native,
                    },
                ],
                tool_call_rollups: vec![
                    KvasirToolCallRollup {
                        day: day(2026, 6, 20),
                        repo: kvasir_repo.clone(),
                        harness: KvasirHarnessName::try_from("claude-code".to_owned())?,
                        tool_name: KvasirToolName::try_from("Read".to_owned())?,
                        call_count: 4,
                    },
                    KvasirToolCallRollup {
                        day: day(2026, 6, 20),
                        repo: other_repo.clone(),
                        harness: KvasirHarnessName::try_from("claude-code".to_owned())?,
                        tool_name: KvasirToolName::try_from("Bash".to_owned())?,
                        call_count: 2,
                    },
                    KvasirToolCallRollup {
                        day: day(2026, 6, 21),
                        repo: kvasir_repo.clone(),
                        harness: KvasirHarnessName::try_from("claude-code".to_owned())?,
                        tool_name: KvasirToolName::try_from("Edit".to_owned())?,
                        call_count: 6,
                    },
                ],
            },
            Some(selected_repo.clone()),
            None,
        );

        assert_eq!(
            snapshot.totals,
            KvasirOverviewTotals {
                total_tokens: 5_050,
                cost_usd_nanos: 3_325_000_000,
                cost_source: Some(KvasirCostSource::Mixed),
                tool_calls: 12,
            }
        );
        assert_eq!(
            snapshot.series,
            vec![
                KvasirOverviewSeriesPoint {
                    day: day(2026, 6, 20),
                    total_tokens: 2_150,
                    cost_usd_nanos: 1_325_000_000,
                    cost_source: Some(KvasirCostSource::Mixed),
                    tool_calls: 6,
                },
                KvasirOverviewSeriesPoint {
                    day: day(2026, 6, 21),
                    total_tokens: 2_900,
                    cost_usd_nanos: 2_000_000_000,
                    cost_source: Some(KvasirCostSource::Native),
                    tool_calls: 6,
                },
            ]
        );
        assert_eq!(
            snapshot.repo_breakdown,
            vec![
                KvasirOverviewRepoSummary {
                    repo: kvasir_repo,
                    totals: KvasirOverviewTotals {
                        total_tokens: 4_650,
                        cost_usd_nanos: 3_250_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 10,
                    },
                },
                KvasirOverviewRepoSummary {
                    repo: other_repo,
                    totals: KvasirOverviewTotals {
                        total_tokens: 400,
                        cost_usd_nanos: 75_000_000,
                        cost_source: Some(KvasirCostSource::Estimated),
                        tool_calls: 2,
                    },
                },
            ]
        );
        assert_eq!(
            snapshot.model_breakdown,
            vec![
                KvasirOverviewModelSummary {
                    model: KvasirModelName::try_from("claude-sonnet-4".to_owned())?,
                    totals: KvasirOverviewTotals {
                        total_tokens: 3_300,
                        cost_usd_nanos: 2_075_000_000,
                        cost_source: Some(KvasirCostSource::Mixed),
                        tool_calls: 0,
                    },
                },
                KvasirOverviewModelSummary {
                    model: KvasirModelName::try_from("claude-opus-4".to_owned())?,
                    totals: KvasirOverviewTotals {
                        total_tokens: 1_750,
                        cost_usd_nanos: 1_250_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 0,
                    },
                },
            ]
        );
        assert_eq!(snapshot.selected_repo, Some(selected_repo));
        assert_eq!(snapshot.selected_model, None);

        Ok(())
    }

    #[test]
    fn overview_snapshot_preserves_cost_source_for_aggregated_costs()
    -> Result<(), KvasirClientError> {
        let kvasir_repo = repo("kvasir", "/repos/kvasir")?;
        let other_repo = repo("other", "/repos/other")?;
        let sonnet = KvasirModelName::try_from("claude-sonnet-4".to_owned())?;
        let opus = KvasirModelName::try_from("claude-opus-4".to_owned())?;

        let snapshot = KvasirOverviewSnapshot::from_rollup(
            KvasirOverviewRollup {
                token_rollups: Vec::new(),
                cost_rollups: vec![
                    KvasirCostRollup {
                        day: day(2026, 6, 20),
                        repo: kvasir_repo.clone(),
                        model: sonnet.clone(),
                        cost_usd: KvasirCostUsd { nanos: 1_000 },
                        source: KvasirCostSource::Estimated,
                    },
                    KvasirCostRollup {
                        day: day(2026, 6, 20),
                        repo: other_repo.clone(),
                        model: opus.clone(),
                        cost_usd: KvasirCostUsd { nanos: 2_000 },
                        source: KvasirCostSource::Native,
                    },
                    KvasirCostRollup {
                        day: day(2026, 6, 21),
                        repo: kvasir_repo.clone(),
                        model: sonnet.clone(),
                        cost_usd: KvasirCostUsd { nanos: 3_000 },
                        source: KvasirCostSource::Estimated,
                    },
                ],
                tool_call_rollups: Vec::new(),
            },
            None,
            None,
        );

        assert_eq!(snapshot.totals.cost_source, Some(KvasirCostSource::Mixed));
        assert_eq!(
            snapshot
                .series
                .iter()
                .map(|point| point.cost_source)
                .collect::<Vec<_>>(),
            vec![
                Some(KvasirCostSource::Mixed),
                Some(KvasirCostSource::Estimated),
            ]
        );
        assert_eq!(
            snapshot
                .repo_breakdown
                .iter()
                .map(|summary| (&summary.repo, summary.totals.cost_source))
                .collect::<Vec<_>>(),
            vec![
                (&kvasir_repo, Some(KvasirCostSource::Estimated)),
                (&other_repo, Some(KvasirCostSource::Native)),
            ]
        );
        assert_eq!(
            snapshot
                .model_breakdown
                .iter()
                .map(|summary| (&summary.model, summary.totals.cost_source))
                .collect::<Vec<_>>(),
            vec![
                (&sonnet, Some(KvasirCostSource::Estimated)),
                (&opus, Some(KvasirCostSource::Native)),
            ]
        );

        Ok(())
    }

    fn repo(name: &str, path: &str) -> Result<KvasirRepoBucket, KvasirClientError> {
        Ok(KvasirRepoBucket {
            kind: KvasirRepoBucketKind::Repo,
            name: Some(KvasirRepoName::try_from(name.to_owned())?),
            path: Some(KvasirRepoPath::try_from(path.to_owned())?),
        })
    }

    fn day(year: i32, month: u8, day: u8) -> KvasirRollupDay {
        KvasirRollupDay { year, month, day }
    }
}
