use chrono::Datelike;
use kvasir_core::explorer::{
    ExplorerCatalog as CoreExplorerCatalog, ExplorerDataset as CoreExplorerDataset,
    ExplorerDatasetCatalog as CoreExplorerDatasetCatalog,
    ExplorerDimension as CoreExplorerDimension, ExplorerFilter as CoreExplorerFilter,
    ExplorerGroupValue as CoreExplorerGroupValue, ExplorerMeasure as CoreExplorerMeasure,
    ExplorerQuery as CoreExplorerQuery, ExplorerQueryResult as CoreExplorerQueryResult,
    ExplorerResultRow as CoreExplorerResultRow, ExplorerSavedPanel as CoreExplorerSavedPanel,
    ExplorerSavedPanelDefinition as CoreExplorerSavedPanelDefinition,
    ExplorerSavedPanelRun as CoreExplorerSavedPanelRun, ExplorerTimeRange as CoreExplorerTimeRange,
    ExplorerValidationError as CoreExplorerValidationError,
    ExplorerVisualization as CoreExplorerVisualization,
    UsageRollupExplorerMeasures as CoreUsageRollupExplorerMeasures,
};
use kvasir_core::rpc::{
    ContentAvailability as CoreContentAvailability,
    ContentKindAvailability as CoreContentKindAvailability, ContentQuery,
    ContentReplay as CoreContentReplay, ContentReplayItem as CoreContentReplayItem,
    ContentUnavailableReason as CoreContentUnavailableReason, CostRollup as CoreCostRollup,
    CostRollupQuery, HarnessSummary as CoreHarnessSummary, OverviewRollup as CoreOverviewRollup,
    PromptSummary as CorePromptSummary, RollupQuery, RpcError,
    SessionSummary as CoreSessionSummary, SummaryTotals as CoreSummaryTotals, TimestampMillis,
    TokenRollup as CoreTokenRollup, ToolCallRollup as CoreToolCallRollup, ToolCallRollupQuery,
    Trace as CoreTrace, TraceDurationMeasures as CoreTraceDurationMeasures, TraceQuery,
    TraceSpan as CoreTraceSpan, TraceSpanKind as CoreTraceSpanKind,
};
use kvasir_core::{ContentKind as CoreContentKind, RepoBucket, RepoIdentity};

use crate::error::KvasirClientError;
use crate::types::{
    KvasirAttributionStatus, KvasirContentAvailability, KvasirContentKind,
    KvasirContentKindAvailability, KvasirContentQuery, KvasirContentReplay,
    KvasirContentReplayItem, KvasirContentUnavailableReason, KvasirCostRollup, KvasirCostSource,
    KvasirCostUsd, KvasirExplorerCatalog, KvasirExplorerDataset, KvasirExplorerDatasetCatalog,
    KvasirExplorerDimension, KvasirExplorerFilter, KvasirExplorerGroupValue, KvasirExplorerMeasure,
    KvasirExplorerQuery, KvasirExplorerResult, KvasirExplorerResultRow, KvasirExplorerSavedPanel,
    KvasirExplorerSavedPanelDefinition, KvasirExplorerSavedPanelRun, KvasirExplorerTimeRange,
    KvasirExplorerValidationError, KvasirExplorerVisualization, KvasirHarnessName, KvasirModelName,
    KvasirOverviewHarnessSummary, KvasirOverviewPromptRoute, KvasirOverviewPromptSummary,
    KvasirOverviewRollup, KvasirOverviewSessionRoute, KvasirOverviewSessionSummary,
    KvasirOverviewTotals, KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath,
    KvasirRollupDay, KvasirRollupQuery, KvasirTimestampMillis, KvasirTokenRollup,
    KvasirToolCallRollup, KvasirTrace, KvasirTraceDurationMeasures, KvasirTraceQuery,
    KvasirTraceSpan, KvasirTraceSpanKind, KvasirUsageRollupExplorerMeasures,
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
        if let Some(harness) = query.harness {
            core_query = core_query.with_harness(harness.into_core());
        }
        if let Some(prompt) = query.prompt {
            core_query = core_query
                .with_harness(prompt.session.harness.into_core())
                .with_session(prompt.session.session_id.into_core())
                .with_prompt(prompt.prompt_id.into_core());
        } else if let Some(session) = query.session {
            core_query = core_query
                .with_harness(session.harness.into_core())
                .with_session(session.session_id.into_core());
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
        if let Some(harness) = query.harness {
            core_query = core_query.with_harness(harness.into_core());
        }
        if let Some(prompt) = query.prompt {
            core_query = core_query
                .with_harness(prompt.session.harness.into_core())
                .with_session(prompt.session.session_id.into_core())
                .with_prompt(prompt.prompt_id.into_core());
        } else if let Some(session) = query.session {
            core_query = core_query
                .with_harness(session.harness.into_core())
                .with_session(session.session_id.into_core());
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
        if let Some(harness) = query.harness {
            core_query = core_query.with_harness(harness.into_core());
        }
        if let Some(prompt) = query.prompt {
            core_query = core_query
                .with_harness(prompt.session.harness.into_core())
                .with_session(prompt.session.session_id.into_core())
                .with_prompt(prompt.prompt_id.into_core());
        } else if let Some(session) = query.session {
            core_query = core_query
                .with_harness(session.harness.into_core())
                .with_session(session.session_id.into_core());
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirExplorerQuery> for CoreExplorerQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirExplorerQuery) -> Result<Self, Self::Error> {
        Ok(Self {
            dataset: query.dataset.into(),
            time_range: query.time_range.into(),
            measures: query
                .measures
                .into_iter()
                .map(CoreExplorerMeasure::from)
                .collect(),
            group_by: query
                .group_by
                .into_iter()
                .map(CoreExplorerDimension::from)
                .collect(),
            filters: query
                .filters
                .into_iter()
                .map(CoreExplorerFilter::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            visualization: query.visualization.into(),
            limit: u32::try_from(query.limit).map_err(|_| KvasirClientError::InvalidQuery)?,
        })
    }
}

impl TryFrom<KvasirExplorerSavedPanelRun> for CoreExplorerSavedPanelRun {
    type Error = KvasirClientError;

    fn try_from(run: KvasirExplorerSavedPanelRun) -> Result<Self, Self::Error> {
        Ok(Self {
            panel: run.panel.into(),
            time_range: run.time_range.into(),
            filters: run
                .filters
                .into_iter()
                .map(CoreExplorerFilter::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<KvasirExplorerTimeRange> for CoreExplorerTimeRange {
    fn from(range: KvasirExplorerTimeRange) -> Self {
        Self {
            start: TimestampMillis::from_millis(range.start.value),
            end: TimestampMillis::from_millis(range.end.value),
        }
    }
}

impl From<KvasirExplorerDataset> for CoreExplorerDataset {
    fn from(dataset: KvasirExplorerDataset) -> Self {
        match dataset {
            KvasirExplorerDataset::UsageRollups => Self::UsageRollups,
        }
    }
}

impl From<CoreExplorerDataset> for KvasirExplorerDataset {
    fn from(dataset: CoreExplorerDataset) -> Self {
        match dataset {
            CoreExplorerDataset::UsageRollups => Self::UsageRollups,
        }
    }
}

impl From<KvasirExplorerSavedPanel> for CoreExplorerSavedPanel {
    fn from(panel: KvasirExplorerSavedPanel) -> Self {
        match panel {
            KvasirExplorerSavedPanel::UsageRollupsOverview => Self::UsageRollupsOverview,
        }
    }
}

impl From<CoreExplorerSavedPanel> for KvasirExplorerSavedPanel {
    fn from(panel: CoreExplorerSavedPanel) -> Self {
        match panel {
            CoreExplorerSavedPanel::UsageRollupsOverview => Self::UsageRollupsOverview,
        }
    }
}

impl From<KvasirExplorerMeasure> for CoreExplorerMeasure {
    fn from(measure: KvasirExplorerMeasure) -> Self {
        match measure {
            KvasirExplorerMeasure::TotalTokens => Self::TotalTokens,
            KvasirExplorerMeasure::CostUsd => Self::CostUsd,
        }
    }
}

impl From<CoreExplorerMeasure> for KvasirExplorerMeasure {
    fn from(measure: CoreExplorerMeasure) -> Self {
        match measure {
            CoreExplorerMeasure::TotalTokens => Self::TotalTokens,
            CoreExplorerMeasure::CostUsd => Self::CostUsd,
        }
    }
}

impl From<KvasirExplorerDimension> for CoreExplorerDimension {
    fn from(dimension: KvasirExplorerDimension) -> Self {
        match dimension {
            KvasirExplorerDimension::Day => Self::Day,
            KvasirExplorerDimension::Repo => Self::Repo,
            KvasirExplorerDimension::Model => Self::Model,
            KvasirExplorerDimension::Harness => Self::Harness,
        }
    }
}

impl From<CoreExplorerDimension> for KvasirExplorerDimension {
    fn from(dimension: CoreExplorerDimension) -> Self {
        match dimension {
            CoreExplorerDimension::Day => Self::Day,
            CoreExplorerDimension::Repo => Self::Repo,
            CoreExplorerDimension::Model => Self::Model,
            CoreExplorerDimension::Harness => Self::Harness,
        }
    }
}

impl From<KvasirExplorerVisualization> for CoreExplorerVisualization {
    fn from(visualization: KvasirExplorerVisualization) -> Self {
        match visualization {
            KvasirExplorerVisualization::Table => Self::Table,
            KvasirExplorerVisualization::LineChart => Self::LineChart,
        }
    }
}

impl From<CoreExplorerVisualization> for KvasirExplorerVisualization {
    fn from(visualization: CoreExplorerVisualization) -> Self {
        match visualization {
            CoreExplorerVisualization::Table => Self::Table,
            CoreExplorerVisualization::LineChart => Self::LineChart,
        }
    }
}

impl From<CoreExplorerValidationError> for KvasirExplorerValidationError {
    fn from(error: CoreExplorerValidationError) -> Self {
        match error {
            CoreExplorerValidationError::EmptyMeasureSelection => Self::EmptyMeasureSelection,
            CoreExplorerValidationError::UnsupportedDimension { dimension } => {
                Self::UnsupportedDimension {
                    dimension: dimension.into(),
                }
            }
            CoreExplorerValidationError::UnsupportedFilter { dimension } => {
                Self::UnsupportedFilter {
                    dimension: dimension.into(),
                }
            }
            CoreExplorerValidationError::UnsupportedVisualization { visualization } => {
                Self::UnsupportedVisualization {
                    visualization: visualization.into(),
                }
            }
            CoreExplorerValidationError::InvalidLimit { requested, max } => Self::InvalidLimit {
                requested: u64::from(requested),
                max: u64::from(max),
            },
            CoreExplorerValidationError::InvalidTimeRange => Self::InvalidTimeRange,
            CoreExplorerValidationError::TooManyGroups { requested, max } => {
                Self::TooManyGroups { requested, max }
            }
        }
    }
}

impl TryFrom<KvasirExplorerFilter> for CoreExplorerFilter {
    type Error = KvasirClientError;

    fn try_from(filter: KvasirExplorerFilter) -> Result<Self, Self::Error> {
        match filter {
            KvasirExplorerFilter::Repo { value } => Ok(Self::Repo(value.try_into()?)),
            KvasirExplorerFilter::Model { value } => Ok(Self::Model(value.into_core())),
            KvasirExplorerFilter::Harness { value } => Ok(Self::Harness(value.into_core())),
        }
    }
}

impl From<CoreExplorerFilter> for KvasirExplorerFilter {
    fn from(filter: CoreExplorerFilter) -> Self {
        match filter {
            CoreExplorerFilter::Repo(value) => Self::Repo {
                value: value.into(),
            },
            CoreExplorerFilter::Model(value) => Self::Model {
                value: KvasirModelName::from_core(value),
            },
            CoreExplorerFilter::Harness(value) => Self::Harness {
                value: KvasirHarnessName::from_core(value),
            },
        }
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

impl From<CoreExplorerCatalog> for KvasirExplorerCatalog {
    fn from(catalog: CoreExplorerCatalog) -> Self {
        Self {
            datasets: catalog
                .datasets
                .into_iter()
                .map(KvasirExplorerDatasetCatalog::from)
                .collect(),
            saved_panels: catalog
                .saved_panels
                .into_iter()
                .map(KvasirExplorerSavedPanelDefinition::from)
                .collect(),
        }
    }
}

impl From<CoreExplorerSavedPanelDefinition> for KvasirExplorerSavedPanelDefinition {
    fn from(panel: CoreExplorerSavedPanelDefinition) -> Self {
        Self {
            panel: panel.panel.into(),
            dataset: panel.dataset.into(),
            measures: panel.measures.into_iter().map(Into::into).collect(),
            group_by: panel.group_by.into_iter().map(Into::into).collect(),
            filters: panel.filters.into_iter().map(Into::into).collect(),
            visualization: panel.visualization.into(),
            limit: u64::from(panel.limit),
        }
    }
}

impl From<CoreExplorerDatasetCatalog> for KvasirExplorerDatasetCatalog {
    fn from(dataset: CoreExplorerDatasetCatalog) -> Self {
        Self {
            dataset: dataset.dataset.into(),
            measures: dataset.measures.into_iter().map(Into::into).collect(),
            dimensions: dataset.dimensions.into_iter().map(Into::into).collect(),
            filters: dataset.filters.into_iter().map(Into::into).collect(),
            visualizations: dataset.visualizations.into_iter().map(Into::into).collect(),
            default_measures: dataset
                .default_measures
                .into_iter()
                .map(Into::into)
                .collect(),
            default_group_by: dataset
                .default_group_by
                .into_iter()
                .map(Into::into)
                .collect(),
            default_visualization: dataset.default_visualization.into(),
            default_limit: u64::from(dataset.default_limit),
            max_limit: u64::from(dataset.max_limit),
            max_grouping_depth: dataset.max_grouping_depth,
        }
    }
}

impl TryFrom<CoreExplorerQueryResult> for KvasirExplorerResult {
    type Error = KvasirClientError;

    fn try_from(result: CoreExplorerQueryResult) -> Result<Self, Self::Error> {
        Ok(Self {
            dataset: result.dataset.into(),
            visualization: result.visualization.into(),
            rows: result
                .rows
                .into_iter()
                .map(KvasirExplorerResultRow::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryFrom<CoreExplorerResultRow> for KvasirExplorerResultRow {
    type Error = KvasirClientError;

    fn try_from(row: CoreExplorerResultRow) -> Result<Self, Self::Error> {
        Ok(Self {
            group: row
                .group
                .into_iter()
                .map(KvasirExplorerGroupValue::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            measures: row.measures.into(),
        })
    }
}

impl TryFrom<CoreExplorerGroupValue> for KvasirExplorerGroupValue {
    type Error = KvasirClientError;

    fn try_from(value: CoreExplorerGroupValue) -> Result<Self, Self::Error> {
        match value {
            CoreExplorerGroupValue::Day(day) => Ok(Self::Day {
                value: rollup_day_from_core(day)?,
            }),
            CoreExplorerGroupValue::Repo(repo) => Ok(Self::Repo { value: repo.into() }),
            CoreExplorerGroupValue::Model(model) => Ok(Self::Model {
                value: crate::types::KvasirModelName::from_core(model),
            }),
            CoreExplorerGroupValue::Harness(harness) => Ok(Self::Harness {
                value: crate::types::KvasirHarnessName::from_core(harness),
            }),
        }
    }
}

impl From<CoreUsageRollupExplorerMeasures> for KvasirUsageRollupExplorerMeasures {
    fn from(measures: CoreUsageRollupExplorerMeasures) -> Self {
        Self {
            total_tokens: measures.total_tokens,
            cost_usd: measures.cost_usd.map(|cost| KvasirCostUsd {
                nanos: cost.as_nanos(),
            }),
            cost_source: measures.cost_source.map(Into::into),
        }
    }
}

impl From<kvasir_core::rpc::AttributionStatus> for KvasirAttributionStatus {
    fn from(status: kvasir_core::rpc::AttributionStatus) -> Self {
        match status {
            kvasir_core::rpc::AttributionStatus::Direct => Self::Direct,
            kvasir_core::rpc::AttributionStatus::TraceDerived => Self::TraceDerived,
            kvasir_core::rpc::AttributionStatus::Partial => Self::Partial,
            kvasir_core::rpc::AttributionStatus::Unavailable => Self::Unavailable,
        }
    }
}

impl From<CoreSummaryTotals> for KvasirOverviewTotals {
    fn from(totals: CoreSummaryTotals) -> Self {
        Self {
            total_tokens: totals.total_tokens,
            cost_usd_nanos: totals.cost_usd.as_nanos(),
            cost_source: totals.cost_source.map(KvasirCostSource::from),
            tool_calls: totals.tool_calls,
        }
    }
}

impl From<kvasir_core::rpc::SessionRoute> for KvasirOverviewSessionRoute {
    fn from(route: kvasir_core::rpc::SessionRoute) -> Self {
        Self {
            harness: crate::types::KvasirHarnessName::from_core(route.harness),
            session_id: crate::types::KvasirSessionId::from_core(route.session_id),
        }
    }
}

impl From<kvasir_core::rpc::PromptRoute> for KvasirOverviewPromptRoute {
    fn from(route: kvasir_core::rpc::PromptRoute) -> Self {
        Self {
            session: route.session.into(),
            prompt_id: crate::types::KvasirPromptId::from_core(route.prompt_id),
        }
    }
}

impl From<CoreSessionSummary> for KvasirOverviewSessionSummary {
    fn from(summary: CoreSessionSummary) -> Self {
        Self {
            route: summary.route.into(),
            totals: summary.totals.into(),
            attribution_status: summary.attribution_status.into(),
            last_activity: KvasirTimestampMillis {
                value: summary.last_activity.value(),
            },
        }
    }
}

impl From<CorePromptSummary> for KvasirOverviewPromptSummary {
    fn from(summary: CorePromptSummary) -> Self {
        Self {
            route: summary.route.into(),
            totals: summary.totals.into(),
            attribution_status: summary.attribution_status.into(),
            last_activity: KvasirTimestampMillis {
                value: summary.last_activity.value(),
            },
        }
    }
}

impl From<CoreHarnessSummary> for KvasirOverviewHarnessSummary {
    fn from(summary: CoreHarnessSummary) -> Self {
        Self {
            harness: crate::types::KvasirHarnessName::from_core(summary.harness),
            totals: summary.totals.into(),
            last_activity: KvasirTimestampMillis {
                value: summary.last_activity.value(),
            },
        }
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
            harness_summaries: rollup
                .harness_summaries
                .into_iter()
                .map(KvasirOverviewHarnessSummary::from)
                .collect(),
            session_summaries: rollup
                .session_summaries
                .into_iter()
                .map(KvasirOverviewSessionSummary::from)
                .collect(),
            session_summaries_more_available: rollup.session_summaries_more_available,
            prompt_summaries: rollup
                .prompt_summaries
                .into_iter()
                .map(KvasirOverviewPromptSummary::from)
                .collect(),
            prompt_summaries_more_available: rollup.prompt_summaries_more_available,
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
            RpcError::ExplorerValidation { errors } => Self::ExplorerValidation {
                errors: errors.into_iter().map(Into::into).collect(),
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        KvasirHarnessName, KvasirOverviewPromptRoute, KvasirOverviewSessionRoute, KvasirPromptId,
        KvasirSessionId,
    };

    #[test]
    fn overview_deep_scope_conversions_preserve_harness_identity() {
        let session = KvasirOverviewSessionRoute {
            harness: KvasirHarnessName::try_from("GitHub-Copilot".to_owned()).unwrap(),
            session_id: KvasirSessionId::try_from("session-12".to_owned()).unwrap(),
        };
        let prompt = KvasirOverviewPromptRoute {
            session: session.clone(),
            prompt_id: KvasirPromptId::try_from("prompt-7".to_owned()).unwrap(),
        };
        let query = KvasirRollupQuery {
            start: KvasirTimestampMillis { value: 10 },
            end: KvasirTimestampMillis { value: 20 },
            repo: None,
            harness: None,
            model: None,
            session: Some(session),
            prompt: Some(prompt),
        };

        let token_query = RollupQuery::try_from(query.clone()).unwrap();
        let cost_query = CostRollupQuery::try_from(query.clone()).unwrap();
        let tool_query = ToolCallRollupQuery::try_from(query).unwrap();

        assert_eq!(token_query.harness.unwrap().as_str(), "github_copilot");
        assert_eq!(token_query.session_id.unwrap().as_str(), "session-12");
        assert_eq!(token_query.prompt_id.unwrap().as_str(), "prompt-7");
        assert_eq!(cost_query.harness.unwrap().as_str(), "github_copilot");
        assert_eq!(cost_query.session_id.unwrap().as_str(), "session-12");
        assert_eq!(cost_query.prompt_id.unwrap().as_str(), "prompt-7");
        assert_eq!(tool_query.harness.unwrap().as_str(), "github_copilot");
        assert_eq!(tool_query.session_id.unwrap().as_str(), "session-12");
        assert_eq!(tool_query.prompt_id.unwrap().as_str(), "prompt-7");
    }
}
