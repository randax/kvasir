use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::rpc::{
    CostRollup, CostRollupQuery, CostSource, HarnessName, ModelName, RollupDay, RollupQuery,
    TimestampMillis, TokenRollup,
};
use crate::usage::{CostUsd, RepoBucket};

pub const USAGE_ROLLUP_DEFAULT_LIMIT: u32 = 50;
pub const USAGE_ROLLUP_MAX_LIMIT: u32 = 500;
const USAGE_ROLLUP_MAX_GROUPING_DEPTH: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerCatalog {
    pub datasets: Vec<ExplorerDatasetCatalog>,
    pub saved_panels: Vec<ExplorerSavedPanelDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerDatasetCatalog {
    pub dataset: ExplorerDataset,
    pub measures: Vec<ExplorerMeasure>,
    pub dimensions: Vec<ExplorerDimension>,
    pub filters: Vec<ExplorerDimension>,
    pub visualizations: Vec<ExplorerVisualization>,
    pub default_measures: Vec<ExplorerMeasure>,
    pub default_group_by: Vec<ExplorerDimension>,
    pub default_visualization: ExplorerVisualization,
    pub default_limit: u32,
    pub max_limit: u32,
    pub max_grouping_depth: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerDataset {
    UsageRollups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerMeasure {
    TotalTokens,
    CostUsd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerDimension {
    Day,
    Repo,
    Model,
    Harness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerVisualization {
    Table,
    LineChart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerTimeRange {
    pub start: TimestampMillis,
    pub end: TimestampMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerQuery {
    pub dataset: ExplorerDataset,
    pub time_range: ExplorerTimeRange,
    pub measures: Vec<ExplorerMeasure>,
    pub group_by: Vec<ExplorerDimension>,
    pub filters: Vec<ExplorerFilter>,
    pub visualization: ExplorerVisualization,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerSavedPanel {
    UsageRollupsOverview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerSavedPanelDefinition {
    pub panel: ExplorerSavedPanel,
    pub dataset: ExplorerDataset,
    pub measures: Vec<ExplorerMeasure>,
    pub group_by: Vec<ExplorerDimension>,
    pub filters: Vec<ExplorerFilter>,
    pub visualization: ExplorerVisualization,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerSavedPanelRun {
    pub panel: ExplorerSavedPanel,
    pub time_range: ExplorerTimeRange,
    pub filters: Vec<ExplorerFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "dimension", content = "value", rename_all = "snake_case")]
pub enum ExplorerFilter {
    Repo(RepoBucket),
    Model(ModelName),
    Harness(HarnessName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerQueryResult {
    pub dataset: ExplorerDataset,
    pub visualization: ExplorerVisualization,
    pub rows: Vec<ExplorerResultRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerResultRow {
    pub group: Vec<ExplorerGroupValue>,
    pub measures: UsageRollupExplorerMeasures,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "dimension", content = "value", rename_all = "snake_case")]
pub enum ExplorerGroupValue {
    Day(RollupDay),
    Repo(RepoBucket),
    Model(ModelName),
    Harness(HarnessName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageRollupExplorerMeasures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<CostUsd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_source: Option<CostSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerValidationErrors {
    pub errors: Vec<ExplorerValidationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplorerValidationError {
    EmptyMeasureSelection,
    UnsupportedDimension {
        dimension: ExplorerDimension,
    },
    UnsupportedFilter {
        dimension: ExplorerDimension,
    },
    UnsupportedVisualization {
        visualization: ExplorerVisualization,
    },
    InvalidLimit {
        requested: u32,
        max: u32,
    },
    InvalidTimeRange,
    TooManyGroups {
        requested: u8,
        max: u8,
    },
}

pub fn explorer_catalog() -> ExplorerCatalog {
    ExplorerCatalog {
        datasets: vec![ExplorerDatasetCatalog {
            dataset: ExplorerDataset::UsageRollups,
            measures: vec![ExplorerMeasure::TotalTokens, ExplorerMeasure::CostUsd],
            dimensions: vec![
                ExplorerDimension::Day,
                ExplorerDimension::Repo,
                ExplorerDimension::Model,
            ],
            filters: vec![
                ExplorerDimension::Repo,
                ExplorerDimension::Model,
                ExplorerDimension::Harness,
            ],
            visualizations: vec![ExplorerVisualization::Table],
            default_measures: vec![ExplorerMeasure::TotalTokens, ExplorerMeasure::CostUsd],
            default_group_by: vec![
                ExplorerDimension::Day,
                ExplorerDimension::Repo,
                ExplorerDimension::Model,
            ],
            default_visualization: ExplorerVisualization::Table,
            default_limit: USAGE_ROLLUP_DEFAULT_LIMIT,
            max_limit: USAGE_ROLLUP_MAX_LIMIT,
            max_grouping_depth: USAGE_ROLLUP_MAX_GROUPING_DEPTH,
        }],
        saved_panels: vec![explorer_saved_panel(
            ExplorerSavedPanel::UsageRollupsOverview,
        )],
    }
}

pub fn explorer_saved_panel(panel: ExplorerSavedPanel) -> ExplorerSavedPanelDefinition {
    match panel {
        ExplorerSavedPanel::UsageRollupsOverview => ExplorerSavedPanelDefinition {
            panel,
            dataset: ExplorerDataset::UsageRollups,
            measures: vec![ExplorerMeasure::TotalTokens, ExplorerMeasure::CostUsd],
            group_by: vec![
                ExplorerDimension::Day,
                ExplorerDimension::Repo,
                ExplorerDimension::Model,
            ],
            filters: Vec::new(),
            visualization: ExplorerVisualization::Table,
            limit: USAGE_ROLLUP_DEFAULT_LIMIT,
        },
    }
}

pub fn explorer_query_for_saved_panel(run: ExplorerSavedPanelRun) -> ExplorerQuery {
    let panel = explorer_saved_panel(run.panel);
    ExplorerQuery {
        dataset: panel.dataset,
        time_range: run.time_range,
        measures: panel.measures,
        group_by: panel.group_by,
        filters: run.filters,
        visualization: panel.visualization,
        limit: panel.limit,
    }
}

pub fn validate_explorer_query(query: &ExplorerQuery) -> Result<(), ExplorerValidationErrors> {
    let mut errors = Vec::new();

    if query.time_range.start >= query.time_range.end {
        errors.push(ExplorerValidationError::InvalidTimeRange);
    }
    if query.measures.is_empty() {
        errors.push(ExplorerValidationError::EmptyMeasureSelection);
    }
    if query.limit == 0 || query.limit > USAGE_ROLLUP_MAX_LIMIT {
        errors.push(ExplorerValidationError::InvalidLimit {
            requested: query.limit,
            max: USAGE_ROLLUP_MAX_LIMIT,
        });
    }
    if query.group_by.len() > usize::from(USAGE_ROLLUP_MAX_GROUPING_DEPTH) {
        errors.push(ExplorerValidationError::TooManyGroups {
            requested: query.group_by.len() as u8,
            max: USAGE_ROLLUP_MAX_GROUPING_DEPTH,
        });
    }
    for dimension in &query.group_by {
        if !matches!(
            dimension,
            ExplorerDimension::Day | ExplorerDimension::Repo | ExplorerDimension::Model
        ) {
            errors.push(ExplorerValidationError::UnsupportedDimension {
                dimension: *dimension,
            });
        }
    }
    for filter in &query.filters {
        let dimension = filter.dimension();
        if !matches!(
            dimension,
            ExplorerDimension::Repo | ExplorerDimension::Model | ExplorerDimension::Harness
        ) {
            errors.push(ExplorerValidationError::UnsupportedFilter { dimension });
        }
    }
    if query.visualization != ExplorerVisualization::Table {
        errors.push(ExplorerValidationError::UnsupportedVisualization {
            visualization: query.visualization,
        });
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ExplorerValidationErrors { errors })
    }
}

pub fn usage_rollup_token_query(query: &ExplorerQuery) -> RollupQuery {
    let mut rollup_query = RollupQuery::new(query.time_range.start, query.time_range.end);
    for filter in &query.filters {
        match filter {
            ExplorerFilter::Repo(repo) => rollup_query = rollup_query.with_repo(repo.clone()),
            ExplorerFilter::Model(model) => rollup_query = rollup_query.with_model(model.clone()),
            ExplorerFilter::Harness(harness) => {
                rollup_query = rollup_query.with_harness(harness.clone());
            }
        }
    }
    rollup_query
}

pub fn usage_rollup_cost_query(query: &ExplorerQuery) -> CostRollupQuery {
    let mut rollup_query = CostRollupQuery::new(query.time_range.start, query.time_range.end);
    for filter in &query.filters {
        match filter {
            ExplorerFilter::Repo(repo) => rollup_query = rollup_query.with_repo(repo.clone()),
            ExplorerFilter::Model(model) => rollup_query = rollup_query.with_model(model.clone()),
            ExplorerFilter::Harness(harness) => {
                rollup_query = rollup_query.with_harness(harness.clone());
            }
        }
    }
    rollup_query
}

pub fn usage_rollup_explorer_result(
    query: ExplorerQuery,
    token_rollups: Vec<TokenRollup>,
    cost_rollups: Vec<CostRollup>,
) -> ExplorerQueryResult {
    let mut rows = Vec::new();
    let mut indexes = HashMap::new();
    let limit = query.limit as usize;
    let includes_total_tokens = query.measures.contains(&ExplorerMeasure::TotalTokens);
    let includes_cost = query.measures.contains(&ExplorerMeasure::CostUsd);

    if includes_total_tokens {
        for rollup in token_rollups {
            let group = group_for_token_rollup(&query.group_by, &rollup);
            if let Some(row) = row_for_group(&mut rows, &mut indexes, group, limit) {
                let tokens = rollup
                    .input_tokens
                    .saturating_add(rollup.output_tokens)
                    .saturating_add(rollup.cache_tokens);
                row.measures.total_tokens = Some(
                    row.measures
                        .total_tokens
                        .unwrap_or(0)
                        .saturating_add(tokens),
                );
            }
        }
    }

    if includes_cost {
        for rollup in cost_rollups {
            let group = group_for_cost_rollup(&query.group_by, &rollup);
            if let Some(row) = row_for_group(&mut rows, &mut indexes, group, limit) {
                let existing_cost = row
                    .measures
                    .cost_usd
                    .map(|cost| cost.as_nanos())
                    .unwrap_or(0);
                let total_cost = existing_cost.saturating_add(rollup.cost_usd.as_nanos());
                row.measures.cost_usd =
                    Some(CostUsd::from_nanos(total_cost).unwrap_or_else(|| {
                        CostUsd::from_nanos(u64::MAX).expect("u64::MAX fits cost storage")
                    }));
                row.measures.cost_source = Some(combined_cost_source(
                    row.measures.cost_source,
                    rollup.source,
                ));
            }
        }
    }

    ExplorerQueryResult {
        dataset: query.dataset,
        visualization: query.visualization,
        rows,
    }
}

impl ExplorerFilter {
    fn dimension(&self) -> ExplorerDimension {
        match self {
            Self::Repo(_) => ExplorerDimension::Repo,
            Self::Model(_) => ExplorerDimension::Model,
            Self::Harness(_) => ExplorerDimension::Harness,
        }
    }
}

fn row_for_group<'rows>(
    rows: &'rows mut Vec<ExplorerResultRow>,
    indexes: &mut HashMap<Vec<ExplorerGroupValue>, usize>,
    group: Vec<ExplorerGroupValue>,
    limit: usize,
) -> Option<&'rows mut ExplorerResultRow> {
    if let Some(index) = indexes.get(&group).copied() {
        return rows.get_mut(index);
    }
    if rows.len() >= limit {
        return None;
    }
    let index = rows.len();
    indexes.insert(group.clone(), index);
    rows.push(ExplorerResultRow {
        group,
        measures: UsageRollupExplorerMeasures::default(),
    });
    rows.get_mut(index)
}

fn group_for_token_rollup(
    dimensions: &[ExplorerDimension],
    rollup: &TokenRollup,
) -> Vec<ExplorerGroupValue> {
    dimensions
        .iter()
        .filter_map(|dimension| match dimension {
            ExplorerDimension::Day => Some(ExplorerGroupValue::Day(rollup.day)),
            ExplorerDimension::Repo => Some(ExplorerGroupValue::Repo(rollup.repo.clone())),
            ExplorerDimension::Model => Some(ExplorerGroupValue::Model(rollup.model.clone())),
            ExplorerDimension::Harness => None,
        })
        .collect()
}

fn group_for_cost_rollup(
    dimensions: &[ExplorerDimension],
    rollup: &CostRollup,
) -> Vec<ExplorerGroupValue> {
    dimensions
        .iter()
        .filter_map(|dimension| match dimension {
            ExplorerDimension::Day => Some(ExplorerGroupValue::Day(rollup.day)),
            ExplorerDimension::Repo => Some(ExplorerGroupValue::Repo(rollup.repo.clone())),
            ExplorerDimension::Model => Some(ExplorerGroupValue::Model(rollup.model.clone())),
            ExplorerDimension::Harness => None,
        })
        .collect()
}

fn combined_cost_source(current: Option<CostSource>, incoming: CostSource) -> CostSource {
    match current {
        None => incoming,
        Some(existing) if existing == incoming => existing,
        Some(_) => CostSource::Mixed,
    }
}
