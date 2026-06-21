mod client;
mod conversions;
mod error;
mod transport;
mod types;

pub use client::{KvasirClient, KvasirTokenRollupSubscription};
pub use error::KvasirClientError;
pub use types::{
    KvasirCostRollup, KvasirCostUsd, KvasirHarnessName, KvasirModelName, KvasirRepoBucket,
    KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath, KvasirRollupDay, KvasirRollupQuery,
    KvasirSocketPath, KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate,
    KvasirToolCallRollup, KvasirToolName,
};

uniffi::setup_scaffolding!();
