use chrono::Datelike;
use kvasir_core::rpc::{
    ContentAvailability as CoreContentAvailability,
    ContentKindAvailability as CoreContentKindAvailability, ContentQuery,
    ContentReplay as CoreContentReplay, ContentReplayItem as CoreContentReplayItem,
    ContentUnavailableReason as CoreContentUnavailableReason, CostRollup as CoreCostRollup,
    CostRollupQuery, OverviewRollup as CoreOverviewRollup, RollupQuery, RpcError, TimestampMillis,
    TokenRollup as CoreTokenRollup, ToolCallRollup as CoreToolCallRollup, ToolCallRollupQuery,
    Trace as CoreTrace, TraceDurationMeasures as CoreTraceDurationMeasures, TraceQuery,
    TraceSpan as CoreTraceSpan, TraceSpanKind as CoreTraceSpanKind,
};
use kvasir_core::{ContentKind as CoreContentKind, RepoBucket, RepoIdentity};

use crate::error::KvasirClientError;
use crate::types::{
    KvasirContentAvailability, KvasirContentKind, KvasirContentKindAvailability,
    KvasirContentQuery, KvasirContentReplay, KvasirContentReplayItem,
    KvasirContentUnavailableReason, KvasirCostRollup, KvasirCostSource, KvasirCostUsd,
    KvasirOverviewRollup, KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath,
    KvasirRollupDay, KvasirRollupQuery, KvasirTimestampMillis, KvasirTokenRollup,
    KvasirToolCallRollup, KvasirTrace, KvasirTraceDurationMeasures, KvasirTraceQuery,
    KvasirTraceSpan, KvasirTraceSpanKind,
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
        if let Some(model) = query.model {
            core_query = core_query.with_model(model.into_core());
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
        if let Some(model) = query.model {
            core_query = core_query.with_model(model.into_core());
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
        if let Some(model) = query.model {
            core_query = core_query.with_model(model.into_core());
        }
        Ok(core_query)
    }
}

impl From<KvasirTraceQuery> for TraceQuery {
    fn from(query: KvasirTraceQuery) -> Self {
        Self {
            harness: query.harness.into_core(),
            session_id: query.session_id.into_core(),
            prompt_id: query.prompt_id.into_core(),
        }
    }
}

impl From<KvasirContentQuery> for (ContentQuery, kvasir_core::rpc::BearerToken) {
    fn from(query: KvasirContentQuery) -> Self {
        (
            ContentQuery {
                harness: query.harness.into_core(),
                session_id: query.session_id.into_core(),
                prompt_id: query.prompt_id.into_core(),
            },
            query.bearer_token.into_core(),
        )
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
            source: rollup.source.into(),
        })
    }
}

impl From<kvasir_core::rpc::CostSource> for KvasirCostSource {
    fn from(source: kvasir_core::rpc::CostSource) -> Self {
        match source {
            kvasir_core::rpc::CostSource::Native => Self::Native,
            kvasir_core::rpc::CostSource::Estimated => Self::Estimated,
            kvasir_core::rpc::CostSource::Mixed => Self::Mixed,
        }
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

impl TryFrom<CoreOverviewRollup> for KvasirOverviewRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreOverviewRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            token_rollups: rollup
                .token_rollups
                .into_iter()
                .map(KvasirTokenRollup::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            cost_rollups: rollup
                .cost_rollups
                .into_iter()
                .map(KvasirCostRollup::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            tool_call_rollups: rollup
                .tool_call_rollups
                .into_iter()
                .map(KvasirToolCallRollup::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryFrom<CoreTrace> for KvasirTrace {
    type Error = KvasirClientError;

    fn try_from(trace: CoreTrace) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: crate::types::KvasirSessionId::from_core(trace.session_id),
            prompt_id: crate::types::KvasirPromptId::from_core(trace.prompt_id),
            trace_id: crate::types::KvasirTraceId::from_core(trace.trace_id),
            spans: trace
                .spans
                .into_iter()
                .map(KvasirTraceSpan::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            durations: trace.durations.into(),
        })
    }
}

impl TryFrom<CoreContentReplay> for KvasirContentReplay {
    type Error = KvasirClientError;

    fn try_from(replay: CoreContentReplay) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: crate::types::KvasirSessionId::from_core(replay.session_id),
            prompt_id: crate::types::KvasirPromptId::from_core(replay.prompt_id),
            items: replay
                .items
                .into_iter()
                .map(KvasirContentReplayItem::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            availability: replay.availability.into(),
        })
    }
}

impl TryFrom<CoreContentReplayItem> for KvasirContentReplayItem {
    type Error = KvasirClientError;

    fn try_from(item: CoreContentReplayItem) -> Result<Self, Self::Error> {
        Ok(Self {
            occurred_at: KvasirTimestampMillis {
                value: item.occurred_at.value(),
            },
            harness: crate::types::KvasirHarnessName::from_core(item.harness),
            kind: item.kind.into(),
            content: crate::types::KvasirContentText::from_core(item.content),
        })
    }
}

impl From<CoreContentAvailability> for KvasirContentAvailability {
    fn from(availability: CoreContentAvailability) -> Self {
        match availability {
            CoreContentAvailability::Captured { harness, kinds } => Self::Captured {
                harness: crate::types::KvasirHarnessName::from_core(harness),
                kinds: kinds
                    .into_iter()
                    .map(KvasirContentKindAvailability::from)
                    .collect(),
            },
            CoreContentAvailability::Unavailable { reason } => Self::Unavailable {
                reason: reason.into(),
            },
        }
    }
}

impl From<CoreContentKindAvailability> for KvasirContentKindAvailability {
    fn from(availability: CoreContentKindAvailability) -> Self {
        match availability {
            CoreContentKindAvailability::Captured { kind } => Self::Captured { kind: kind.into() },
            CoreContentKindAvailability::Unavailable { kind, reason } => Self::Unavailable {
                kind: kind.into(),
                reason: reason.into(),
            },
        }
    }
}

impl From<CoreContentUnavailableReason> for KvasirContentUnavailableReason {
    fn from(reason: CoreContentUnavailableReason) -> Self {
        match reason {
            CoreContentUnavailableReason::NotProvidedByHarness => Self::NotProvidedByHarness,
            CoreContentUnavailableReason::NotCapturedForPrompt => Self::NotCapturedForPrompt,
            CoreContentUnavailableReason::PromptNotFound => Self::PromptNotFound,
        }
    }
}

impl From<CoreContentKind> for KvasirContentKind {
    fn from(kind: CoreContentKind) -> Self {
        match kind {
            CoreContentKind::UserPrompt => Self::UserPrompt,
            CoreContentKind::AssistantMessage => Self::AssistantMessage,
            CoreContentKind::ToolInput => Self::ToolInput,
            CoreContentKind::ToolOutput => Self::ToolOutput,
            CoreContentKind::RawApiRequest => Self::RawApiRequest,
            CoreContentKind::RawApiResponse => Self::RawApiResponse,
        }
    }
}

impl TryFrom<CoreTraceSpan> for KvasirTraceSpan {
    type Error = KvasirClientError;

    fn try_from(span: CoreTraceSpan) -> Result<Self, Self::Error> {
        Ok(Self {
            span_id: crate::types::KvasirSpanId::from_core(span.span_id),
            parent_span_id: span
                .parent_span_id
                .map(crate::types::KvasirSpanId::from_core),
            kind: span.kind.into(),
            name: crate::types::KvasirSpanName::from_core(span.name),
            started_at: KvasirTimestampMillis {
                value: span.started_at.value(),
            },
            ended_at: KvasirTimestampMillis {
                value: span.ended_at.value(),
            },
            duration_ms: span.duration_ms,
            tool_name: span.tool_name.map(crate::types::KvasirToolName::from_core),
        })
    }
}

impl From<CoreTraceDurationMeasures> for KvasirTraceDurationMeasures {
    fn from(durations: CoreTraceDurationMeasures) -> Self {
        Self {
            ttft_ms: durations.ttft_ms,
            request_ms: durations.request_ms,
            tool_ms: durations.tool_ms,
        }
    }
}

impl From<CoreTraceSpanKind> for KvasirTraceSpanKind {
    fn from(kind: CoreTraceSpanKind) -> Self {
        match kind {
            CoreTraceSpanKind::Interaction => Self::Interaction,
            CoreTraceSpanKind::LlmRequest => Self::LlmRequest,
            CoreTraceSpanKind::ToolCall => Self::ToolCall,
        }
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
            RpcError::InvalidRequest | RpcError::Internal | RpcError::Unauthorized => {
                Self::DaemonError
            }
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
