use chrono::Datelike;
use kvasir_core::rpc::{
    CostRollup as CoreCostRollup, CostRollupQuery, RollupQuery, RpcError, TimestampMillis,
    TokenRollup as CoreTokenRollup, ToolCallRollup as CoreToolCallRollup, ToolCallRollupQuery,
};
use kvasir_core::{RepoBucket, RepoIdentity};

use crate::error::KvasirClientError;
use crate::types::{
    KvasirCostRollup, KvasirCostUsd, KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName,
    KvasirRepoPath, KvasirRollupDay, KvasirRollupQuery, KvasirTokenRollup, KvasirToolCallRollup,
};

impl TryFrom<KvasirRollupQuery> for RollupQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirRollupQuery) -> Result<Self, Self::Error> {
        let mut core_query = Self::new(
            TimestampMillis::from_millis(query.start.value),
            TimestampMillis::from_millis(query.end.value),
        );
        if let Some(repo) = query.repo {
            core_query = core_query.with_repo(repo.try_into()?);
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirRollupQuery> for CostRollupQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirRollupQuery) -> Result<Self, Self::Error> {
        let mut core_query = Self::new(
            TimestampMillis::from_millis(query.start.value),
            TimestampMillis::from_millis(query.end.value),
        );
        if let Some(repo) = query.repo {
            core_query = core_query.with_repo(repo.try_into()?);
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirRollupQuery> for ToolCallRollupQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirRollupQuery) -> Result<Self, Self::Error> {
        let mut core_query = Self::new(
            TimestampMillis::from_millis(query.start.value),
            TimestampMillis::from_millis(query.end.value),
        );
        if let Some(repo) = query.repo {
            core_query = core_query.with_repo(repo.try_into()?);
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirRepoBucket> for RepoBucket {
    type Error = KvasirClientError;

    fn try_from(repo: KvasirRepoBucket) -> Result<Self, Self::Error> {
        match repo.kind {
            KvasirRepoBucketKind::NoRepo => Ok(Self::no_repo()),
            KvasirRepoBucketKind::Repo => {
                let name = repo.name.map(KvasirRepoName::into_core);
                let path = repo.path.map(KvasirRepoPath::into_core);
                RepoIdentity::from_parts(name, path)
                    .map(Self::repo)
                    .ok_or(KvasirClientError::InvalidQuery)
            }
        }
    }
}

impl TryFrom<CoreTokenRollup> for KvasirTokenRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreTokenRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            day: rollup_day_from_core(rollup.day)?,
            repo: rollup.repo.into(),
            model: crate::types::KvasirModelName::from_core(rollup.model),
            input_tokens: rollup.input_tokens,
            output_tokens: rollup.output_tokens,
            cache_tokens: rollup.cache_tokens,
        })
    }
}

impl TryFrom<CoreCostRollup> for KvasirCostRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreCostRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            day: rollup_day_from_core(rollup.day)?,
            repo: rollup.repo.into(),
            model: crate::types::KvasirModelName::from_core(rollup.model),
            cost_usd: KvasirCostUsd {
                nanos: rollup.cost_usd.as_nanos(),
            },
        })
    }
}

impl TryFrom<CoreToolCallRollup> for KvasirToolCallRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreToolCallRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            day: rollup_day_from_core(rollup.day)?,
            repo: rollup.repo.into(),
            harness: crate::types::KvasirHarnessName::from_core(rollup.harness),
            tool_name: crate::types::KvasirToolName::from_core(rollup.tool_name),
            call_count: rollup.call_count,
        })
    }
}

impl From<RepoBucket> for KvasirRepoBucket {
    fn from(repo: RepoBucket) -> Self {
        match repo {
            RepoBucket::NoRepo => Self {
                kind: KvasirRepoBucketKind::NoRepo,
                name: None,
                path: None,
            },
            RepoBucket::Repo(identity) => Self {
                kind: KvasirRepoBucketKind::Repo,
                name: identity.name.map(KvasirRepoName::from_core),
                path: identity.path.map(KvasirRepoPath::from_core),
            },
        }
    }
}

impl From<RpcError> for KvasirClientError {
    fn from(error: RpcError) -> Self {
        match error {
            RpcError::ResponseTooLarge => Self::RpcResponseTooLarge,
            RpcError::InvalidRequest | RpcError::Internal => Self::DaemonError,
        }
    }
}

fn rollup_day_from_core(
    day: kvasir_core::rpc::RollupDay,
) -> Result<KvasirRollupDay, KvasirClientError> {
    let date = day.as_date();
    Ok(KvasirRollupDay {
        year: date.year(),
        month: u8::try_from(date.month()).map_err(|_| KvasirClientError::InvalidQuery)?,
        day: u8::try_from(date.day()).map_err(|_| KvasirClientError::InvalidQuery)?,
    })
}
