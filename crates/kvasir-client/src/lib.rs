mod client;
mod conversions;
mod error;
mod transport;
mod types;

pub use client::{KvasirClient, KvasirTokenRollupSubscription};
pub use error::KvasirClientError;
pub use types::{
    KvasirCostRollup, KvasirCostUsd, KvasirHarnessName, KvasirModelName, KvasirPromptId,
    KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath, KvasirRollupDay,
    KvasirRollupQuery, KvasirSessionId, KvasirSocketPath, KvasirSpanId, KvasirSpanName,
    KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate, KvasirToolCallRollup,
    KvasirToolName, KvasirTrace, KvasirTraceDurationMeasures, KvasirTraceId, KvasirTraceQuery,
    KvasirTraceSpan, KvasirTraceSpanKind,
};

uniffi::setup_scaffolding!();
