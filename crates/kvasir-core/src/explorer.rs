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
pub struct UsageRollupExplorerPanelSnapshot {
    pub panel: ExplorerSavedPanelDefinition,
    pub query: ExplorerQuery,
    pub result: ExplorerQueryResult,
    pub table: ExplorerTablePresentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerTablePresentation {
    pub columns: Vec<ExplorerTableColumn>,
    pub rows: Vec<ExplorerTableRowPresentation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerTableRowPresentation {
    pub cells: Vec<ExplorerTableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplorerTableColumn {
    Dimension { dimension: ExplorerDimension },
    TotalTokens,
    CostUsd,
    CostSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplorerTableCell {
    Day { value: RollupDay },
    Repo { value: RepoBucket },
    Model { value: ModelName },
    Harness { value: HarnessName },
    TotalTokens { value: u64 },
    EmptyTotalTokens,
    CostUsd { value: CostUsd },
    EmptyCostUsd,
    CostSource { value: CostSource },
    EmptyCostSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerValidationErrors {
    pub errors: Vec<ExplorerValidationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExplorerValidationError {
    EmptyMeasureSelection,
    UnsupportedDataset {
        dataset: ExplorerDataset,
    },
    UnsupportedSavedPanel {
        panel: ExplorerSavedPanel,
    },
    UnsupportedMeasure {
        measure: ExplorerMeasure,
    },
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

pub fn usage_rollup_explorer_query_for_panel(
    catalog: &ExplorerCatalog,
    mut panel: ExplorerSavedPanelDefinition,
    time_range: ExplorerTimeRange,
    filters: Vec<ExplorerFilter>,
) -> Result<(ExplorerSavedPanelDefinition, ExplorerQuery), ExplorerValidationErrors> {
    panel.filters = filters;

    let mut errors = Vec::new();
    if !catalog
        .saved_panels
        .iter()
        .any(|saved_panel| saved_panel.panel == panel.panel)
    {
        errors.push(ExplorerValidationError::UnsupportedSavedPanel { panel: panel.panel });
    }

    if let Some(dataset) = catalog
        .datasets
        .iter()
        .find(|dataset| dataset.dataset == panel.dataset)
    {
        errors.extend(validate_panel_against_dataset(&panel, dataset));
    } else {
        errors.push(ExplorerValidationError::UnsupportedDataset {
            dataset: panel.dataset,
        });
    }

    if time_range.start >= time_range.end {
        errors.push(ExplorerValidationError::InvalidTimeRange);
    }

    if !errors.is_empty() {
        return Err(ExplorerValidationErrors { errors });
    }

    let query = ExplorerQuery {
        dataset: panel.dataset,
        time_range,
        measures: panel.measures.clone(),
        group_by: panel.group_by.clone(),
        filters: panel.filters.clone(),
        visualization: panel.visualization,
        limit: panel.limit,
    };
    Ok((panel, query))
}

pub fn usage_rollup_explorer_panel_snapshot(
    panel: ExplorerSavedPanelDefinition,
    query: ExplorerQuery,
    result: ExplorerQueryResult,
) -> UsageRollupExplorerPanelSnapshot {
    let table = explorer_table_presentation(&query, &result);
    UsageRollupExplorerPanelSnapshot {
        panel,
        query,
        result,
        table,
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

fn validate_panel_against_dataset(
    panel: &ExplorerSavedPanelDefinition,
    dataset: &ExplorerDatasetCatalog,
) -> Vec<ExplorerValidationError> {
    let mut errors = Vec::new();

    if panel.measures.is_empty() {
        errors.push(ExplorerValidationError::EmptyMeasureSelection);
    }
    errors.extend(
        panel
            .measures
            .iter()
            .filter(|measure| !dataset.measures.contains(measure))
            .map(|measure| ExplorerValidationError::UnsupportedMeasure { measure: *measure }),
    );
    errors.extend(
        panel
            .group_by
            .iter()
            .filter(|dimension| !dataset.dimensions.contains(dimension))
            .map(|dimension| ExplorerValidationError::UnsupportedDimension {
                dimension: *dimension,
            }),
    );
    if panel.group_by.len() > usize::from(dataset.max_grouping_depth) {
        errors.push(ExplorerValidationError::TooManyGroups {
            requested: panel.group_by.len() as u8,
            max: dataset.max_grouping_depth,
        });
    }
    errors.extend(
        panel
            .filters
            .iter()
            .map(ExplorerFilter::dimension)
            .filter(|dimension| !dataset.filters.contains(dimension))
            .map(|dimension| ExplorerValidationError::UnsupportedFilter { dimension }),
    );
    if !dataset.visualizations.contains(&panel.visualization)
        || panel.visualization != ExplorerVisualization::Table
    {
        errors.push(ExplorerValidationError::UnsupportedVisualization {
            visualization: panel.visualization,
        });
    }
    if panel.limit == 0 || panel.limit > dataset.max_limit {
        errors.push(ExplorerValidationError::InvalidLimit {
            requested: panel.limit,
            max: dataset.max_limit,
        });
    }

    errors
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
                let clamped_cost = total_cost.min(i64::MAX as u64);
                row.measures.cost_usd =
                    Some(CostUsd::from_nanos(clamped_cost).expect("clamped cost fits storage"));
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

fn explorer_table_presentation(
    query: &ExplorerQuery,
    result: &ExplorerQueryResult,
) -> ExplorerTablePresentation {
    ExplorerTablePresentation {
        columns: query
            .group_by
            .iter()
            .map(|dimension| ExplorerTableColumn::Dimension {
                dimension: *dimension,
            })
            .chain(query.measures.iter().flat_map(measure_columns))
            .collect(),
        rows: result
            .rows
            .iter()
            .map(|row| ExplorerTableRowPresentation {
                cells: row_cells(row, &query.measures),
            })
            .collect(),
    }
}

fn row_cells(
    row: &ExplorerResultRow,
    selected_measures: &[ExplorerMeasure],
) -> Vec<ExplorerTableCell> {
    row.group
        .iter()
        .map(group_cell)
        .chain(
            selected_measures
                .iter()
                .flat_map(|measure| measure_cells(*measure, &row.measures)),
        )
        .collect()
}

fn measure_columns(measure: &ExplorerMeasure) -> Vec<ExplorerTableColumn> {
    match measure {
        ExplorerMeasure::TotalTokens => vec![ExplorerTableColumn::TotalTokens],
        ExplorerMeasure::CostUsd => vec![
            ExplorerTableColumn::CostUsd,
            ExplorerTableColumn::CostSource,
        ],
    }
}

fn measure_cells(
    measure: ExplorerMeasure,
    measures: &UsageRollupExplorerMeasures,
) -> Vec<ExplorerTableCell> {
    match measure {
        ExplorerMeasure::TotalTokens => vec![
            measures
                .total_tokens
                .map_or(ExplorerTableCell::EmptyTotalTokens, |value| {
                    ExplorerTableCell::TotalTokens { value }
                }),
        ],
        ExplorerMeasure::CostUsd => vec![
            measures
                .cost_usd
                .map_or(ExplorerTableCell::EmptyCostUsd, |value| {
                    ExplorerTableCell::CostUsd { value }
                }),
            measures
                .cost_source
                .map_or(ExplorerTableCell::EmptyCostSource, |value| {
                    ExplorerTableCell::CostSource { value }
                }),
        ],
    }
}

fn group_cell(value: &ExplorerGroupValue) -> ExplorerTableCell {
    match value {
        ExplorerGroupValue::Day(value) => ExplorerTableCell::Day { value: *value },
        ExplorerGroupValue::Repo(value) => ExplorerTableCell::Repo {
            value: value.clone(),
        },
        ExplorerGroupValue::Model(value) => ExplorerTableCell::Model {
            value: value.clone(),
        },
        ExplorerGroupValue::Harness(value) => ExplorerTableCell::Harness {
            value: value.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::rpc::{CostRollup, CostSource, ModelName, RollupDay, TimestampMillis};
    use crate::usage::{CostUsd, RepoBucket, RepoIdentity, RepoName, RepoPath};

    use super::{
        ExplorerCatalog, ExplorerDataset, ExplorerDatasetCatalog, ExplorerDimension,
        ExplorerFilter, ExplorerMeasure, ExplorerQuery, ExplorerQueryResult, ExplorerResultRow,
        ExplorerSavedPanelDefinition, ExplorerTableCell, ExplorerTableColumn, ExplorerTimeRange,
        ExplorerValidationError, ExplorerVisualization, UsageRollupExplorerMeasures,
        explorer_catalog, explorer_saved_panel, usage_rollup_explorer_panel_snapshot,
        usage_rollup_explorer_query_for_panel, usage_rollup_explorer_result,
    };

    #[test]
    fn usage_rollup_explorer_result_clamps_aggregated_cost_to_i64_max() {
        let query = ExplorerQuery {
            dataset: ExplorerDataset::UsageRollups,
            time_range: ExplorerTimeRange {
                start: TimestampMillis::from_millis(1),
                end: TimestampMillis::from_millis(2),
            },
            measures: vec![ExplorerMeasure::CostUsd],
            group_by: Vec::new(),
            filters: Vec::new(),
            visualization: ExplorerVisualization::Table,
            limit: 10,
        };
        let day = RollupDay::parse("2026-06-20").expect("valid rollup day");
        let rollups = vec![
            CostRollup {
                day,
                repo: RepoBucket::no_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: CostUsd::from_nanos(i64::MAX as u64).expect("i64::MAX is representable"),
                source: CostSource::Native,
            },
            CostRollup {
                day,
                repo: RepoBucket::no_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: CostUsd::from_nanos(1).expect("small cost is representable"),
                source: CostSource::Native,
            },
        ];

        let result = usage_rollup_explorer_result(query, Vec::new(), rollups);

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0]
                .measures
                .cost_usd
                .expect("cost measure should be populated")
                .as_nanos(),
            i64::MAX as u64
        );
    }

    #[test]
    fn usage_rollup_explorer_query_for_panel_replaces_filters_with_runtime_filters() {
        let catalog = explorer_catalog();
        let mut panel = explorer_saved_panel(super::ExplorerSavedPanel::UsageRollupsOverview);
        panel.filters = vec![ExplorerFilter::Model(ModelName::new("stale-model"))];
        let repo = kvasir_repo();
        let filters = vec![ExplorerFilter::Repo(repo.clone())];
        let range = ExplorerTimeRange {
            start: TimestampMillis::from_millis(1),
            end: TimestampMillis::from_millis(2),
        };

        let (resolved_panel, query) = usage_rollup_explorer_query_for_panel(
            &catalog,
            panel.clone(),
            range.clone(),
            filters.clone(),
        )
        .expect("valid panel should produce query");

        assert_eq!(resolved_panel.filters, filters);
        assert_eq!(
            query,
            ExplorerQuery {
                dataset: panel.dataset,
                time_range: range,
                measures: panel.measures,
                group_by: panel.group_by,
                filters,
                visualization: panel.visualization,
                limit: panel.limit,
            }
        );
    }

    #[test]
    fn usage_rollup_explorer_panel_snapshot_builds_typed_table_in_query_measure_order() {
        let repo = kvasir_repo();
        let day = RollupDay::parse("2026-06-20").expect("valid rollup day");
        let panel = ExplorerSavedPanelDefinition {
            panel: super::ExplorerSavedPanel::UsageRollupsOverview,
            dataset: ExplorerDataset::UsageRollups,
            measures: vec![ExplorerMeasure::CostUsd, ExplorerMeasure::TotalTokens],
            group_by: vec![ExplorerDimension::Day, ExplorerDimension::Repo],
            filters: vec![ExplorerFilter::Repo(repo.clone())],
            visualization: ExplorerVisualization::Table,
            limit: 25,
        };
        let query = ExplorerQuery {
            dataset: panel.dataset,
            time_range: ExplorerTimeRange {
                start: TimestampMillis::from_millis(1),
                end: TimestampMillis::from_millis(2),
            },
            measures: panel.measures.clone(),
            group_by: panel.group_by.clone(),
            filters: panel.filters.clone(),
            visualization: panel.visualization,
            limit: panel.limit,
        };
        let result = ExplorerQueryResult {
            dataset: ExplorerDataset::UsageRollups,
            visualization: ExplorerVisualization::Table,
            rows: vec![ExplorerResultRow {
                group: vec![
                    super::ExplorerGroupValue::Day(day),
                    super::ExplorerGroupValue::Repo(repo.clone()),
                ],
                measures: UsageRollupExplorerMeasures {
                    total_tokens: Some(1_700),
                    cost_usd: Some(
                        CostUsd::from_nanos(54_150_000).expect("small cost is representable"),
                    ),
                    cost_source: Some(CostSource::Estimated),
                },
            }],
        };

        let snapshot = usage_rollup_explorer_panel_snapshot(panel, query, result);

        assert_eq!(
            snapshot.table.columns,
            vec![
                ExplorerTableColumn::Dimension {
                    dimension: ExplorerDimension::Day,
                },
                ExplorerTableColumn::Dimension {
                    dimension: ExplorerDimension::Repo,
                },
                ExplorerTableColumn::CostUsd,
                ExplorerTableColumn::CostSource,
                ExplorerTableColumn::TotalTokens,
            ]
        );
        assert_eq!(
            snapshot.table.rows[0].cells,
            vec![
                ExplorerTableCell::Day { value: day },
                ExplorerTableCell::Repo { value: repo },
                ExplorerTableCell::CostUsd {
                    value: CostUsd::from_nanos(54_150_000).expect("small cost is representable"),
                },
                ExplorerTableCell::CostSource {
                    value: CostSource::Estimated,
                },
                ExplorerTableCell::TotalTokens { value: 1_700 },
            ]
        );
    }

    #[test]
    fn usage_rollup_explorer_query_for_panel_reports_catalog_validation_errors() {
        let panel = explorer_saved_panel(super::ExplorerSavedPanel::UsageRollupsOverview);
        let range = ExplorerTimeRange {
            start: TimestampMillis::from_millis(1),
            end: TimestampMillis::from_millis(2),
        };
        let missing_catalog = ExplorerCatalog {
            datasets: Vec::new(),
            saved_panels: Vec::new(),
        };
        let errors = usage_rollup_explorer_query_for_panel(
            &missing_catalog,
            panel.clone(),
            range.clone(),
            Vec::new(),
        )
        .expect_err("missing catalog entries should fail");
        assert_eq!(
            errors.errors,
            vec![
                ExplorerValidationError::UnsupportedSavedPanel {
                    panel: super::ExplorerSavedPanel::UsageRollupsOverview,
                },
                ExplorerValidationError::UnsupportedDataset {
                    dataset: ExplorerDataset::UsageRollups,
                },
            ]
        );

        let no_measure_catalog = ExplorerCatalog {
            datasets: vec![ExplorerDatasetCatalog {
                dataset: ExplorerDataset::UsageRollups,
                measures: Vec::new(),
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
                default_measures: Vec::new(),
                default_group_by: Vec::new(),
                default_visualization: ExplorerVisualization::Table,
                default_limit: 50,
                max_limit: 500,
                max_grouping_depth: 3,
            }],
            saved_panels: vec![panel.clone()],
        };
        let errors =
            usage_rollup_explorer_query_for_panel(&no_measure_catalog, panel, range, Vec::new())
                .expect_err("unsupported measures should fail");
        assert_eq!(
            errors.errors,
            vec![
                ExplorerValidationError::UnsupportedMeasure {
                    measure: ExplorerMeasure::TotalTokens,
                },
                ExplorerValidationError::UnsupportedMeasure {
                    measure: ExplorerMeasure::CostUsd,
                },
            ]
        );
    }

    fn kvasir_repo() -> RepoBucket {
        RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ))
    }
}
