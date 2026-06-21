mod client;
mod conversions;
mod error;
mod transport;
mod types;

pub use client::{KvasirClient, KvasirTokenRollupSubscription};
pub use error::KvasirClientError;
pub use types::{
    KvasirCostRollup, KvasirCostUsd, KvasirModelName, KvasirRepoBucket, KvasirRepoBucketKind,
    KvasirRepoName, KvasirRepoPath, KvasirRollupDay, KvasirRollupQuery, KvasirSocketPath,
    KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate,
};

uniffi::setup_scaffolding!();
