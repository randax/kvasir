mod client;
mod conversions;
mod error;
mod setup;
mod transport;
mod types;

pub use client::{KvasirClient, KvasirTokenRollupSubscription};
pub use error::KvasirClientError;
pub use setup::{
    KvasirClaudeSettingsPath, KvasirCodexConfigPath, KvasirHarnessTelemetrySetup,
    KvasirOpenCodeConfigPath, KvasirOpenCodeEnvPath, KvasirOtlpEndpoint, KvasirRawBodyDirectory,
    KvasirRepoHookPath, KvasirShellProfilePath, configure_kvasir_harness_telemetry,
    uninstall_kvasir_harness_telemetry,
};
pub use types::{
    KvasirBearerToken, KvasirContentAvailability, KvasirContentKind, KvasirContentKindAvailability,
    KvasirContentQuery, KvasirContentReplay, KvasirContentReplayItem, KvasirContentText,
    KvasirContentUnavailableReason, KvasirCostRollup, KvasirCostUsd, KvasirHarnessName,
    KvasirModelName, KvasirOverviewRollup, KvasirPromptId, KvasirRepoBucket, KvasirRepoBucketKind,
    KvasirRepoName, KvasirRepoPath, KvasirRollupDay, KvasirRollupQuery, KvasirSessionId,
    KvasirSocketPath, KvasirSpanId, KvasirSpanName, KvasirTimestampMillis, KvasirTokenRollup,
    KvasirTokenRollupUpdate, KvasirToolCallRollup, KvasirToolName, KvasirTrace,
    KvasirTraceDurationMeasures, KvasirTraceId, KvasirTraceQuery, KvasirTraceSpan,
    KvasirTraceSpanKind,
};

uniffi::setup_scaffolding!();
