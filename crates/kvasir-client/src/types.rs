use crate::error::KvasirClientError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirSocketPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirRepoName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirRepoPath(String);

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

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirRepoBucketKind {
    NoRepo,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTimestampMillis {
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRepoBucket {
    pub kind: KvasirRepoBucketKind,
    pub name: Option<KvasirRepoName>,
    pub path: Option<KvasirRepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTraceQuery {
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
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

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirCostRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub model: KvasirModelName,
    pub cost_usd: KvasirCostUsd,
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
pub struct KvasirTrace {
    pub session_id: KvasirSessionId,
    pub prompt_id: KvasirPromptId,
    pub trace_id: KvasirTraceId,
    pub spans: Vec<KvasirTraceSpan>,
    pub durations: KvasirTraceDurationMeasures,
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
}

impl KvasirHarnessName {
    pub(crate) fn from_core(value: kvasir_core::rpc::HarnessName) -> Self {
        Self(value.as_str().to_owned())
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
        nonempty_text(value).map(Self)
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

fn nonempty_text(value: String) -> Result<String, KvasirClientError> {
    if value.trim().is_empty() {
        Err(KvasirClientError::InvalidQuery)
    } else {
        Ok(value)
    }
}
