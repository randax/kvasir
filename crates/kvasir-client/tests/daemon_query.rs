use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use kvasir_client::{
    KvasirBearerToken, KvasirClient, KvasirClientError, KvasirContentAvailability,
    KvasirContentKind, KvasirContentKindAvailability, KvasirContentReplay, KvasirContentReplayItem,
    KvasirContentText, KvasirContentUnavailableReason, KvasirCostRollup, KvasirCostSource,
    KvasirCostUsd, KvasirExplorerDataset, KvasirExplorerDatasetCatalog, KvasirExplorerDimension,
    KvasirExplorerFilter, KvasirExplorerGroupValue, KvasirExplorerMeasure, KvasirExplorerQuery,
    KvasirExplorerSavedPanel, KvasirExplorerSavedPanelDefinition, KvasirExplorerSavedPanelRun,
    KvasirExplorerTableCell, KvasirExplorerTableColumn, KvasirExplorerTimeRange,
    KvasirExplorerValidationError, KvasirExplorerVisualization, KvasirHarnessName, KvasirModelName,
    KvasirOverviewHarnessSummary, KvasirOverviewModelSummary, KvasirOverviewRefreshSubscription,
    KvasirOverviewRepoSummary, KvasirOverviewRollup, KvasirOverviewSeriesPoint,
    KvasirOverviewSessionRoute, KvasirOverviewSnapshot, KvasirOverviewTotals, KvasirPromptId,
    KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath, KvasirRollupDay,
    KvasirRollupQuery, KvasirSessionId, KvasirSocketPath, KvasirSpanId, KvasirSpanName,
    KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate, KvasirToolCallRollup,
    KvasirToolName, KvasirTraceDurationMeasures, KvasirTraceId, KvasirTraceQuery, KvasirTraceSpan,
    KvasirTraceSpanKind, KvasirUsageRollupExplorerPanelRequest, KvasirUsageUpdateKind,
};
use kvasir_core::explorer::{
    ExplorerCatalog as CoreExplorerCatalog, ExplorerDataset, ExplorerDatasetCatalog,
    ExplorerDimension, ExplorerMeasure, ExplorerQuery, ExplorerQueryResult, ExplorerResultRow,
    ExplorerSavedPanel as CoreExplorerSavedPanel,
    ExplorerSavedPanelDefinition as CoreExplorerSavedPanelDefinition, ExplorerTimeRange,
    ExplorerValidationError, ExplorerVisualization,
    UsageRollupExplorerMeasures as CoreUsageRollupExplorerMeasures,
};
use kvasir_core::rpc::{
    BearerToken, ContentQuery, HarnessName as CoreHarnessName, PromptId as CorePromptId,
    RpcRequest, RpcResponse, RpcStreamEvent, SessionId as CoreSessionId, UsageUpdateKind,
};
use kvasir_core::{
    ContentRetentionPolicy, PriceTable, RepoBucket, RepoIdentity, RepoName as CoreRepoName,
    RepoPath as CoreRepoPath,
};
use kvasird::{
    ContentRetentionSchedule, DaemonConfig, StoreKeySource, start_with_store_key_source,
};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use tempfile::tempdir;

#[tokio::test]
async fn client_queries_token_rollups_through_daemon_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: None,
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let rollups = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.token_rollups(query)
    })
    .await??;

    assert_eq!(
        rollups,
        vec![
            KvasirTokenRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-opus-4-20250514"),
                input_tokens: 1100,
                output_tokens: 500,
                cache_tokens: 100,
            },
            KvasirTokenRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-sonnet-4-20250514"),
                input_tokens: 300,
                output_tokens: 120,
                cache_tokens: 30,
            },
            KvasirTokenRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 21,
                },
                repo: kvasir_repo(),
                model: model("claude-sonnet-4-20250514"),
                input_tokens: 2000,
                output_tokens: 800,
                cache_tokens: 50,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn client_runs_usage_rollup_explorer_query_through_daemon_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirExplorerQuery {
        dataset: KvasirExplorerDataset::UsageRollups,
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        measures: vec![
            KvasirExplorerMeasure::TotalTokens,
            KvasirExplorerMeasure::CostUsd,
        ],
        group_by: vec![
            KvasirExplorerDimension::Day,
            KvasirExplorerDimension::Repo,
            KvasirExplorerDimension::Model,
        ],
        filters: Vec::new(),
        visualization: KvasirExplorerVisualization::Table,
        limit: 10,
    };
    let saved_panel_run = KvasirExplorerSavedPanelRun {
        panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        filters: Vec::new(),
    };
    let panel_request = KvasirUsageRollupExplorerPanelRequest {
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        filters: Vec::new(),
        saved_panel: None,
    };
    let (catalog, saved_panel, result, saved_panel_result, panel_snapshot) =
        tokio::task::spawn_blocking(move || {
            let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
            Ok::<_, KvasirClientError>((
                client.explorer_catalog()?,
                client.explorer_saved_panel(KvasirExplorerSavedPanel::UsageRollupsOverview)?,
                client.run_explorer_query(query)?,
                client.run_explorer_saved_panel(saved_panel_run)?,
                client.usage_rollup_explorer_panel(panel_request)?,
            ))
        })
        .await??;

    assert_eq!(
        catalog.datasets,
        vec![KvasirExplorerDatasetCatalog {
            dataset: KvasirExplorerDataset::UsageRollups,
            measures: vec![
                KvasirExplorerMeasure::TotalTokens,
                KvasirExplorerMeasure::CostUsd,
            ],
            dimensions: vec![
                KvasirExplorerDimension::Day,
                KvasirExplorerDimension::Repo,
                KvasirExplorerDimension::Model,
            ],
            filters: vec![
                KvasirExplorerDimension::Repo,
                KvasirExplorerDimension::Model,
                KvasirExplorerDimension::Harness,
            ],
            visualizations: vec![KvasirExplorerVisualization::Table],
            default_measures: vec![
                KvasirExplorerMeasure::TotalTokens,
                KvasirExplorerMeasure::CostUsd,
            ],
            default_group_by: vec![
                KvasirExplorerDimension::Day,
                KvasirExplorerDimension::Repo,
                KvasirExplorerDimension::Model,
            ],
            default_visualization: KvasirExplorerVisualization::Table,
            default_limit: 50,
            max_limit: 500,
            max_grouping_depth: 3,
        }]
    );
    assert_eq!(
        catalog.saved_panels,
        vec![KvasirExplorerSavedPanelDefinition {
            panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
            dataset: KvasirExplorerDataset::UsageRollups,
            measures: vec![
                KvasirExplorerMeasure::TotalTokens,
                KvasirExplorerMeasure::CostUsd,
            ],
            group_by: vec![
                KvasirExplorerDimension::Day,
                KvasirExplorerDimension::Repo,
                KvasirExplorerDimension::Model,
            ],
            filters: Vec::new(),
            visualization: KvasirExplorerVisualization::Table,
            limit: 50,
        }]
    );
    assert_eq!(saved_panel, catalog.saved_panels[0]);

    assert_eq!(result.dataset, KvasirExplorerDataset::UsageRollups);
    assert_eq!(result.visualization, KvasirExplorerVisualization::Table);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows[0].group,
        vec![
            KvasirExplorerGroupValue::Day {
                value: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
            },
            KvasirExplorerGroupValue::Repo {
                value: kvasir_repo(),
            },
            KvasirExplorerGroupValue::Model {
                value: model("claude-opus-4-20250514"),
            },
        ]
    );
    assert_eq!(result.rows[0].measures.total_tokens, Some(1_700));
    assert_eq!(
        result.rows[0].measures.cost_usd,
        Some(KvasirCostUsd { nanos: 54_150_000 })
    );
    assert_eq!(
        result.rows[0].measures.cost_source,
        Some(KvasirCostSource::Estimated)
    );

    assert_eq!(result.rows[1].measures.total_tokens, Some(450));
    assert_eq!(
        result.rows[1].measures.cost_usd,
        Some(KvasirCostUsd { nanos: 2_709_000 })
    );
    assert_eq!(result.rows[2].measures.total_tokens, Some(2_850));
    assert_eq!(
        result.rows[2].measures.cost_usd,
        Some(KvasirCostUsd { nanos: 18_015_000 })
    );
    assert_eq!(saved_panel_result, result);
    assert_eq!(panel_snapshot.panel, saved_panel);
    assert_eq!(
        panel_snapshot.query,
        KvasirExplorerQuery {
            dataset: KvasirExplorerDataset::UsageRollups,
            time_range: KvasirExplorerTimeRange {
                start: timestamp(2026, 6, 19),
                end: timestamp(2026, 6, 22),
            },
            measures: vec![
                KvasirExplorerMeasure::TotalTokens,
                KvasirExplorerMeasure::CostUsd,
            ],
            group_by: vec![
                KvasirExplorerDimension::Day,
                KvasirExplorerDimension::Repo,
                KvasirExplorerDimension::Model,
            ],
            filters: Vec::new(),
            visualization: KvasirExplorerVisualization::Table,
            limit: 50,
        }
    );
    assert_eq!(panel_snapshot.result, result);
    assert_eq!(
        panel_snapshot.table.columns,
        vec![
            KvasirExplorerTableColumn::Dimension {
                dimension: KvasirExplorerDimension::Day,
            },
            KvasirExplorerTableColumn::Dimension {
                dimension: KvasirExplorerDimension::Repo,
            },
            KvasirExplorerTableColumn::Dimension {
                dimension: KvasirExplorerDimension::Model,
            },
            KvasirExplorerTableColumn::TotalTokens,
            KvasirExplorerTableColumn::CostUsd,
            KvasirExplorerTableColumn::CostSource,
        ]
    );
    assert_eq!(
        panel_snapshot.table.rows[0].cells,
        vec![
            KvasirExplorerTableCell::Day {
                value: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
            },
            KvasirExplorerTableCell::Repo {
                value: kvasir_repo(),
            },
            KvasirExplorerTableCell::Model {
                value: model("claude-opus-4-20250514"),
            },
            KvasirExplorerTableCell::TotalTokens { value: 1_700 },
            KvasirExplorerTableCell::CostUsd {
                value: KvasirCostUsd { nanos: 54_150_000 },
            },
            KvasirExplorerTableCell::CostSource {
                value: KvasirCostSource::Estimated,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn usage_rollup_explorer_panel_uses_supplied_saved_panel_and_runtime_filters()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let saved_panel = KvasirExplorerSavedPanelDefinition {
        panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
        dataset: KvasirExplorerDataset::UsageRollups,
        measures: vec![
            KvasirExplorerMeasure::CostUsd,
            KvasirExplorerMeasure::TotalTokens,
        ],
        group_by: vec![KvasirExplorerDimension::Day, KvasirExplorerDimension::Repo],
        filters: vec![KvasirExplorerFilter::Model {
            value: model("stale-model"),
        }],
        visualization: KvasirExplorerVisualization::Table,
        limit: 25,
    };
    let filters = vec![KvasirExplorerFilter::Repo {
        value: kvasir_repo(),
    }];
    let request = KvasirUsageRollupExplorerPanelRequest {
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        filters: filters.clone(),
        saved_panel: Some(saved_panel),
    };
    let snapshot = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.usage_rollup_explorer_panel(request)
    })
    .await??;

    assert_eq!(snapshot.panel.filters, filters);
    assert_eq!(snapshot.query.filters, filters);
    assert_eq!(
        snapshot.query.measures,
        vec![
            KvasirExplorerMeasure::CostUsd,
            KvasirExplorerMeasure::TotalTokens,
        ]
    );
    assert_eq!(
        snapshot.table.columns,
        vec![
            KvasirExplorerTableColumn::Dimension {
                dimension: KvasirExplorerDimension::Day,
            },
            KvasirExplorerTableColumn::Dimension {
                dimension: KvasirExplorerDimension::Repo,
            },
            KvasirExplorerTableColumn::CostUsd,
            KvasirExplorerTableColumn::CostSource,
            KvasirExplorerTableColumn::TotalTokens,
        ]
    );
    assert_eq!(
        snapshot.table.rows[0].cells,
        vec![
            KvasirExplorerTableCell::Day {
                value: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
            },
            KvasirExplorerTableCell::Repo {
                value: kvasir_repo(),
            },
            KvasirExplorerTableCell::CostUsd {
                value: KvasirCostUsd { nanos: 56_859_000 },
            },
            KvasirExplorerTableCell::CostSource {
                value: KvasirCostSource::Estimated,
            },
            KvasirExplorerTableCell::TotalTokens { value: 2_150 },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn usage_rollup_explorer_panel_preserves_typed_panel_validation_errors() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let request = KvasirUsageRollupExplorerPanelRequest {
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        filters: Vec::new(),
        saved_panel: Some(KvasirExplorerSavedPanelDefinition {
            panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
            dataset: KvasirExplorerDataset::UsageRollups,
            measures: vec![KvasirExplorerMeasure::TotalTokens],
            group_by: vec![KvasirExplorerDimension::Day],
            filters: Vec::new(),
            visualization: KvasirExplorerVisualization::LineChart,
            limit: 0,
        }),
    };
    let error = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        Ok::<_, KvasirClientError>(
            client
                .usage_rollup_explorer_panel(request)
                .expect_err("invalid saved panel should fail validation"),
        )
    })
    .await??;

    assert_eq!(
        error,
        KvasirClientError::ExplorerValidation {
            errors: vec![
                KvasirExplorerValidationError::UnsupportedVisualization {
                    visualization: KvasirExplorerVisualization::LineChart,
                },
                KvasirExplorerValidationError::InvalidLimit {
                    requested: 0,
                    max: 500,
                },
            ],
        }
    );

    Ok(())
}

#[tokio::test]
async fn usage_rollup_explorer_panel_preserves_catalog_validation_errors_from_rpc_catalog()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let missing_socket_path = temp.path().join("missing-catalog.sock");
    let missing_server = start_scripted_rpc_server(
        missing_socket_path.clone(),
        vec![RpcResponse::ExplorerCatalog {
            catalog: CoreExplorerCatalog {
                datasets: Vec::new(),
                saved_panels: Vec::new(),
            },
        }],
    )?;
    let missing_error = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(missing_socket_path))?;
        Ok::<_, KvasirClientError>(
            client
                .usage_rollup_explorer_panel(valid_panel_request())
                .expect_err("missing catalog entries should fail validation"),
        )
    })
    .await??;
    missing_server.join().expect("scripted server panicked")?;
    assert_eq!(
        missing_error,
        KvasirClientError::ExplorerValidation {
            errors: vec![
                KvasirExplorerValidationError::UnsupportedSavedPanel {
                    panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
                },
                KvasirExplorerValidationError::UnsupportedDataset {
                    dataset: KvasirExplorerDataset::UsageRollups,
                },
            ],
        }
    );

    let no_measure_socket_path = temp.path().join("no-measure-catalog.sock");
    let no_measure_server = start_scripted_rpc_server(
        no_measure_socket_path.clone(),
        vec![RpcResponse::ExplorerCatalog {
            catalog: CoreExplorerCatalog {
                datasets: vec![core_usage_rollup_catalog_with_measures(Vec::new())],
                saved_panels: vec![core_usage_rollup_saved_panel()],
            },
        }],
    )?;
    let no_measure_error = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(no_measure_socket_path))?;
        Ok::<_, KvasirClientError>(
            client
                .usage_rollup_explorer_panel(valid_panel_request())
                .expect_err("unsupported catalog measures should fail validation"),
        )
    })
    .await??;
    no_measure_server
        .join()
        .expect("scripted server panicked")?;
    assert_eq!(
        no_measure_error,
        KvasirClientError::ExplorerValidation {
            errors: vec![
                KvasirExplorerValidationError::UnsupportedMeasure {
                    measure: KvasirExplorerMeasure::TotalTokens,
                },
                KvasirExplorerValidationError::UnsupportedMeasure {
                    measure: KvasirExplorerMeasure::CostUsd,
                },
            ],
        }
    );

    Ok(())
}

#[tokio::test]
async fn usage_rollup_explorer_panel_preserves_empty_typed_table_cells_from_rpc_result()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("empty-cells.sock");
    let day = kvasir_core::rpc::RollupDay::parse("2026-06-20")?;
    let repo = core_repo();
    let server = start_scripted_rpc_server(
        rpc_socket_path.clone(),
        vec![
            RpcResponse::ExplorerCatalog {
                catalog: CoreExplorerCatalog {
                    datasets: vec![core_usage_rollup_catalog_with_measures(vec![
                        ExplorerMeasure::TotalTokens,
                        ExplorerMeasure::CostUsd,
                    ])],
                    saved_panels: vec![core_usage_rollup_saved_panel()],
                },
            },
            RpcResponse::ExplorerQuery {
                result: ExplorerQueryResult {
                    dataset: ExplorerDataset::UsageRollups,
                    visualization: ExplorerVisualization::Table,
                    rows: vec![ExplorerResultRow {
                        group: vec![
                            kvasir_core::explorer::ExplorerGroupValue::Day(day),
                            kvasir_core::explorer::ExplorerGroupValue::Repo(repo),
                        ],
                        measures: CoreUsageRollupExplorerMeasures {
                            total_tokens: None,
                            cost_usd: None,
                            cost_source: None,
                        },
                    }],
                },
            },
        ],
    )?;

    let snapshot = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.usage_rollup_explorer_panel(valid_panel_request())
    })
    .await??;
    server.join().expect("scripted server panicked")?;

    assert_eq!(
        snapshot.table.rows[0].cells,
        vec![
            KvasirExplorerTableCell::Day {
                value: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
            },
            KvasirExplorerTableCell::Repo {
                value: kvasir_repo(),
            },
            KvasirExplorerTableCell::EmptyTotalTokens,
            KvasirExplorerTableCell::EmptyCostUsd,
            KvasirExplorerTableCell::EmptyCostSource,
        ]
    );

    Ok(())
}

#[tokio::test]
async fn daemon_returns_typed_validation_errors_for_unsupported_explorer_query()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let request = serde_json::to_string(&RpcRequest::ExplorerQuery {
        query: ExplorerQuery {
            dataset: ExplorerDataset::UsageRollups,
            time_range: ExplorerTimeRange {
                start: kvasir_core::rpc::TimestampMillis::from_millis(timestamp(2026, 6, 19).value),
                end: kvasir_core::rpc::TimestampMillis::from_millis(timestamp(2026, 6, 22).value),
            },
            measures: vec![ExplorerMeasure::TotalTokens],
            group_by: vec![ExplorerDimension::Harness],
            filters: Vec::new(),
            visualization: ExplorerVisualization::Table,
            limit: 10,
        },
    })?;

    let response =
        tokio::task::spawn_blocking(move || raw_rpc_request(&rpc_socket_path, &request)).await??;

    assert_eq!(
        response,
        RpcResponse::Error {
            error: kvasir_core::rpc::RpcError::ExplorerValidation {
                errors: vec![ExplorerValidationError::UnsupportedDimension {
                    dimension: ExplorerDimension::Harness,
                }],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_preserves_typed_explorer_validation_errors() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let query = KvasirExplorerQuery {
        dataset: KvasirExplorerDataset::UsageRollups,
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        measures: vec![KvasirExplorerMeasure::TotalTokens],
        group_by: vec![KvasirExplorerDimension::Harness],
        filters: Vec::new(),
        visualization: KvasirExplorerVisualization::Table,
        limit: 10,
    };
    let error = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        Ok::<_, KvasirClientError>(
            client
                .run_explorer_query(query)
                .expect_err("unsupported grouping should fail validation"),
        )
    })
    .await??;

    assert_eq!(
        error,
        KvasirClientError::ExplorerValidation {
            errors: vec![KvasirExplorerValidationError::UnsupportedDimension {
                dimension: KvasirExplorerDimension::Harness,
            }],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_clears_all_data_through_daemon_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: None,
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let clear_socket_path = rpc_socket_path.clone();
    let rollups_after_clear = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(clear_socket_path))?;
        assert!(!client.token_rollups(query.clone())?.is_empty());
        client.clear_all_data(KvasirBearerToken::try_from("test-token".to_owned())?)?;
        client.token_rollups(query)
    })
    .await??;

    assert!(rollups_after_clear.is_empty());

    Ok(())
}

#[tokio::test]
async fn client_clear_all_data_requires_bearer_token() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: None,
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let rollups_after_failed_clear = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        let clear_result =
            client.clear_all_data(KvasirBearerToken::try_from("wrong-token".to_owned())?);
        assert!(matches!(clear_result, Err(KvasirClientError::DaemonError)));
        client.token_rollups(query)
    })
    .await??;

    assert!(!rollups_after_failed_clear.is_empty());

    Ok(())
}

#[tokio::test]
async fn client_queries_claude_trace_by_session_and_prompt() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("claude"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
        })
    })
    .await??;

    assert_eq!(traces.len(), 1);
    let trace = &traces[0];
    assert_eq!(trace.session_id, session("session-12"));
    assert_eq!(trace.prompt_id, prompt("prompt-7"));
    assert_eq!(trace.trace_id, trace_id("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert_eq!(
        trace.durations,
        KvasirTraceDurationMeasures {
            ttft_ms: Some(250),
            request_ms: Some(3_000),
            tool_ms: Some(750),
        }
    );
    assert_eq!(
        trace.spans,
        vec![
            KvasirTraceSpan {
                span_id: span("1111111111111111"),
                parent_span_id: None,
                kind: KvasirTraceSpanKind::Interaction,
                name: span_name("claude.interaction"),
                started_at: KvasirTimestampMillis {
                    value: 1_781_956_800_000
                },
                ended_at: KvasirTimestampMillis {
                    value: 1_781_956_802_750
                },
                duration_ms: 2_750,
                tool_name: None,
            },
            KvasirTraceSpan {
                span_id: span("2222222222222222"),
                parent_span_id: Some(span("1111111111111111")),
                kind: KvasirTraceSpanKind::LlmRequest,
                name: span_name("claude.llm_request"),
                started_at: KvasirTimestampMillis {
                    value: 1_781_956_800_250
                },
                ended_at: KvasirTimestampMillis {
                    value: 1_781_956_802_250
                },
                duration_ms: 2_000,
                tool_name: None,
            },
            KvasirTraceSpan {
                span_id: span("3333333333333333"),
                parent_span_id: Some(span("1111111111111111")),
                kind: KvasirTraceSpanKind::ToolCall,
                name: span_name("claude.tool"),
                started_at: KvasirTimestampMillis {
                    value: 1_781_956_802_250
                },
                ended_at: KvasirTimestampMillis {
                    value: 1_781_956_802_750
                },
                duration_ms: 500,
                tool_name: Some(tool("Read")),
            },
            KvasirTraceSpan {
                span_id: span("4444444444444444"),
                parent_span_id: Some(span("1111111111111111")),
                kind: KvasirTraceSpanKind::LlmRequest,
                name: span_name("claude.llm_request"),
                started_at: KvasirTimestampMillis {
                    value: 1_781_956_803_000
                },
                ended_at: KvasirTimestampMillis {
                    value: 1_781_956_804_000
                },
                duration_ms: 1_000,
                tool_name: None,
            },
            KvasirTraceSpan {
                span_id: span("5555555555555555"),
                parent_span_id: Some(span("1111111111111111")),
                kind: KvasirTraceSpanKind::ToolCall,
                name: span_name("claude.tool"),
                started_at: KvasirTimestampMillis {
                    value: 1_781_956_804_000
                },
                ended_at: KvasirTimestampMillis {
                    value: 1_781_956_804_250
                },
                duration_ms: 250,
                tool_name: Some(tool("Bash")),
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn client_retrieves_trace_response_above_previous_rpc_response_cap() -> anyhow::Result<()> {
    const TRACE_SPAN_COUNT_ABOVE_OLD_RPC_CAP: usize = 400;

    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(large_claude_trace_fixture(
            TRACE_SPAN_COUNT_ABOVE_OLD_RPC_CAP,
        ))
        .send()
        .await?
        .error_for_status()?;

    let traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("claude"),
            session_id: session("session-large"),
            prompt_id: prompt("prompt-large"),
        })
    })
    .await??;

    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), TRACE_SPAN_COUNT_ABOVE_OLD_RPC_CAP);
    assert_eq!(
        traces[0].durations.request_ms,
        Some(TRACE_SPAN_COUNT_ABOVE_OLD_RPC_CAP as u64)
    );

    Ok(())
}

#[tokio::test]
async fn client_keeps_distinct_trace_ids_for_the_same_session_prompt() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(two_trace_ids_for_same_prompt_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("claude"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
        })
    })
    .await??;

    assert_eq!(traces.len(), 2);
    assert_eq!(
        traces
            .iter()
            .map(|trace| String::from(trace.trace_id.clone()))
            .collect::<Vec<_>>(),
        vec![
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        ]
    );
    assert_eq!(traces[0].durations.request_ms, Some(2_000));
    assert_eq!(traces[1].durations.request_ms, Some(1_000));

    Ok(())
}

#[tokio::test]
async fn client_scopes_trace_replay_by_harness() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_trace_fixture())
        .send()
        .await?
        .error_for_status()?;
    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(codex_trace_reusing_claude_identity_fixture())
        .send()
        .await?
        .error_for_status()?;

    let claude_socket_path = rpc_socket_path.clone();
    let claude_traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(claude_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("claude"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
        })
    })
    .await??;
    let codex_traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("codex"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
        })
    })
    .await??;

    assert_eq!(claude_traces.len(), 1);
    assert_eq!(claude_traces[0].spans.len(), 5);
    assert_eq!(
        claude_traces[0].spans[0].name,
        span_name("claude.interaction")
    );
    assert_eq!(codex_traces.len(), 1);
    assert_eq!(codex_traces[0].spans.len(), 1);
    assert_eq!(
        codex_traces[0].spans[0].name,
        span_name("codex.interaction")
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_protobuf_claude_trace_by_session_and_prompt() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(claude_trace_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.trace(KvasirTraceQuery {
            harness: harness("claude"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
        })
    })
    .await??;

    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].trace_id,
        trace_id("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(traces[0].spans[0].span_id, span("1111111111111111"));
    assert_eq!(traces[0].durations.request_ms, Some(2_000));

    Ok(())
}

#[tokio::test]
async fn client_queries_content_replay_by_session_and_prompt() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(opencode_trace_content_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("opencode"),
            session("opencode-session-1"),
            prompt("opencode-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("opencode-session-1"),
            prompt_id: prompt("opencode-turn-1"),
            items: vec![
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_801_920,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::UserPrompt,
                    content: content_text("summarize README.md"),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_801_920,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::AssistantMessage,
                    content: content_text("I need to read it first."),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_170,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::ToolInput,
                    content: content_text(r#"{"path":"README.md"}"#),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_170,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::ToolOutput,
                    content: content_text("kvasir is a local telemetry daemon"),
                },
            ],
            availability: KvasirContentAvailability::Captured {
                harness: harness("opencode"),
                kinds: vec![
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::UserPrompt,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::AssistantMessage,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::ToolInput,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::ToolOutput,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_protobuf_content_replay_by_session_and_prompt() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(opencode_trace_content_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("opencode"),
            session("opencode-session-1"),
            prompt("opencode-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("opencode-session-1"),
            prompt_id: prompt("opencode-turn-1"),
            items: vec![
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_801_920,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::UserPrompt,
                    content: content_text("summarize README.md"),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_801_920,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::AssistantMessage,
                    content: content_text("I need to read it first."),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_170,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::ToolInput,
                    content: content_text(r#"{"path":"README.md"}"#),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_170,
                    },
                    harness: harness("opencode"),
                    kind: KvasirContentKind::ToolOutput,
                    content: content_text("kvasir is a local telemetry daemon"),
                },
            ],
            availability: KvasirContentAvailability::Captured {
                harness: harness("opencode"),
                kinds: vec![
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::UserPrompt,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::AssistantMessage,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::ToolInput,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::ToolOutput,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_opencode_content_replay_from_opted_in_logs() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(opencode_content_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("opencode"),
            session("opencode-session-1"),
            prompt("opencode-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("opencode-session-1"),
            prompt_id: prompt("opencode-turn-1"),
            items: vec![KvasirContentReplayItem {
                occurred_at: KvasirTimestampMillis {
                    value: 1_781_956_802_180,
                },
                harness: harness("opencode"),
                kind: KvasirContentKind::AssistantMessage,
                content: content_text("stored assistant text"),
            }],
            availability: KvasirContentAvailability::Captured {
                harness: harness("opencode"),
                kinds: vec![
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::AssistantMessage,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::UserPrompt,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolInput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolOutput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn raw_rpc_queries_canonicalize_hyphenated_mixed_case_harnesses() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(
            r#"{
                "resourceSpans": [{
                    "resource": {
                        "attributes": [
                            { "key": "service.name", "value": { "stringValue": "GitHub-Copilot" } },
                            { "key": "session.id", "value": { "stringValue": "session-12" } },
                            { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                        ]
                    },
                    "scopeSpans": [{
                        "spans": [{
                            "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                            "spanId": "1111111111111111",
                            "name": "github.copilot.interaction",
                            "startTimeUnixNano": "1781956800000000000",
                            "endTimeUnixNano": "1781956801000000000",
                            "attributes": [
                                { "key": "span.kind", "value": { "stringValue": "interaction" } }
                            ]
                        }]
                    }]
                }]
            }"#,
        )
        .send()
        .await?
        .error_for_status()?;
    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(
            r#"{
                "resourceLogs": [{
                    "resource": {
                        "attributes": [
                            { "key": "service.name", "value": { "stringValue": "GitHub-Copilot" } },
                            { "key": "session.id", "value": { "stringValue": "session-12" } },
                            { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                        ]
                    },
                    "scopeLogs": [{
                        "logRecords": [{
                            "timeUnixNano": "1781956802000000000",
                            "eventName": "github.copilot.content",
                            "body": { "stringValue": "stored copilot text" },
                            "attributes": [
                                { "key": "content.opt_in", "value": { "boolValue": true } },
                                { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                            ]
                        }]
                    }]
                }]
            }"#,
        )
        .send()
        .await?
        .error_for_status()?;

    let trace_socket_path = rpc_socket_path.clone();
    let trace_response = tokio::task::spawn_blocking(move || {
        raw_rpc_request(
            &trace_socket_path,
            r#"{"type":"trace","payload":{"query":{"harness":" GitHub-Copilot ","session_id":"session-12","prompt_id":"prompt-7"}}}"#,
        )
    })
    .await??;
    let content_response = tokio::task::spawn_blocking(move || {
        raw_rpc_request(
            &rpc_socket_path,
            r#"{"type":"content","payload":{"query":{"harness":" GitHub-Copilot ","session_id":"session-12","prompt_id":"prompt-7"},"bearer_token":"test-token"}}"#,
        )
    })
    .await??;

    let RpcResponse::Trace { traces } = trace_response else {
        panic!("expected trace response");
    };
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].spans[0].name.as_str(),
        "github.copilot.interaction"
    );

    let RpcResponse::Content { replay } = content_response else {
        panic!("expected content response");
    };
    assert_eq!(replay.items.len(), 1);
    assert_eq!(replay.items[0].harness.as_str(), "github_copilot");
    assert_eq!(replay.items[0].content.as_str(), "stored copilot text");

    Ok(())
}

#[tokio::test]
async fn client_queries_claude_content_replay_from_opted_in_logs() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_content_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("claude"),
            session("claude-session-1"),
            prompt("claude-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("claude-session-1"),
            prompt_id: prompt("claude-turn-1"),
            items: vec![
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_180,
                    },
                    harness: harness("claude"),
                    kind: KvasirContentKind::UserPrompt,
                    content: content_text("explain this repository"),
                },
                KvasirContentReplayItem {
                    occurred_at: KvasirTimestampMillis {
                        value: 1_781_956_802_220,
                    },
                    harness: harness("claude"),
                    kind: KvasirContentKind::ToolOutput,
                    content: content_text("README.md contains the project overview"),
                },
            ],
            availability: KvasirContentAvailability::Captured {
                harness: harness("claude"),
                kinds: vec![
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::UserPrompt,
                    },
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::ToolOutput,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::AssistantMessage,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolInput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_codex_content_replay_from_opted_in_logs() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(codex_content_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("codex"),
            session("codex-session-1"),
            prompt("codex-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("codex-session-1"),
            prompt_id: prompt("codex-turn-1"),
            items: vec![KvasirContentReplayItem {
                occurred_at: KvasirTimestampMillis {
                    value: 1_781_956_802_180,
                },
                harness: harness("codex"),
                kind: KvasirContentKind::AssistantMessage,
                content: content_text("codex response text"),
            }],
            availability: KvasirContentAvailability::Captured {
                harness: harness("codex"),
                kinds: vec![
                    KvasirContentKindAvailability::Captured {
                        kind: KvasirContentKind::AssistantMessage,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::UserPrompt,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolInput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolOutput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_ignores_content_logs_from_unknown_harnesses() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(unknown_harness_content_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("random_service"),
            session("unknown-session-1"),
            prompt("unknown-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("unknown-session-1"),
            prompt_id: prompt("unknown-turn-1"),
            items: Vec::new(),
            availability: KvasirContentAvailability::Unavailable {
                reason: KvasirContentUnavailableReason::PromptNotFound,
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_reports_prompt_not_found_for_empty_content_replay() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("opencode"),
            session("missing-session"),
            prompt("missing-prompt"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("missing-session"),
            prompt_id: prompt("missing-prompt"),
            items: Vec::new(),
            availability: KvasirContentAvailability::Unavailable {
                reason: KvasirContentUnavailableReason::PromptNotFound,
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_reports_known_harness_content_kinds_when_prompt_has_no_content()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("claude"),
            session("session-12"),
            prompt("prompt-7"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
            items: Vec::new(),
            availability: KvasirContentAvailability::Captured {
                harness: harness("claude"),
                kinds: vec![
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::UserPrompt,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::AssistantMessage,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolInput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::ToolOutput,
                        reason: KvasirContentUnavailableReason::NotCapturedForPrompt,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiRequest,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                    KvasirContentKindAvailability::Unavailable {
                        kind: KvasirContentKind::RawApiResponse,
                        reason: KvasirContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_reports_not_provided_for_existing_unsupported_harness_prompt() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(unsupported_harness_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("unknown-harness"),
            session("unknown-session-1"),
            prompt("unknown-turn-1"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("unknown-session-1"),
            prompt_id: prompt("unknown-turn-1"),
            items: Vec::new(),
            availability: KvasirContentAvailability::Unavailable {
                reason: KvasirContentUnavailableReason::NotProvidedByHarness,
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_does_not_report_content_capability_for_another_harness_prompt() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let replay = tokio::task::spawn_blocking(move || {
        load_content_replay(
            rpc_socket_path,
            harness("codex"),
            session("session-12"),
            prompt("prompt-7"),
        )
    })
    .await??;

    assert_eq!(
        replay,
        KvasirContentReplay {
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
            items: Vec::new(),
            availability: KvasirContentAvailability::Unavailable {
                reason: KvasirContentUnavailableReason::PromptNotFound,
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_overview_rollups_through_one_daemon_socket_request() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;
    let http_client = reqwest::Client::new();

    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(native_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let (overview, snapshot) = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        let overview = client.overview_rollups(query.clone())?;
        let snapshot = client.overview_snapshot(query)?;
        Ok::<_, KvasirClientError>((overview, snapshot))
    })
    .await??;

    assert_eq!(
        overview,
        KvasirOverviewRollup {
            token_rollups: vec![
                KvasirTokenRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-opus-4-20250514"),
                    input_tokens: 1100,
                    output_tokens: 500,
                    cache_tokens: 100,
                },
                KvasirTokenRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-sonnet-4-20250514"),
                    input_tokens: 300,
                    output_tokens: 120,
                    cache_tokens: 30,
                },
                KvasirTokenRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-sonnet-4-20250514"),
                    input_tokens: 2000,
                    output_tokens: 800,
                    cache_tokens: 50,
                },
            ],
            cost_rollups: vec![
                KvasirCostRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-opus-4-20250514"),
                    cost_usd: KvasirCostUsd {
                        nanos: 1_250_000_000,
                    },
                    source: KvasirCostSource::Native,
                },
                KvasirCostRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-sonnet-4-20250514"),
                    cost_usd: KvasirCostUsd { nanos: 200_000_000 },
                    source: KvasirCostSource::Native,
                },
                KvasirCostRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-opus-4-20250514"),
                    cost_usd: KvasirCostUsd { nanos: 500_000_000 },
                    source: KvasirCostSource::Native,
                },
                KvasirCostRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    repo: kvasir_repo(),
                    model: model("claude-sonnet-4-20250514"),
                    cost_usd: KvasirCostUsd { nanos: 18_015_000 },
                    source: KvasirCostSource::Estimated,
                },
            ],
            tool_call_rollups: vec![
                KvasirToolCallRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    harness: harness("claude_code"),
                    tool_name: tool("Bash"),
                    call_count: 1,
                },
                KvasirToolCallRollup {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    repo: kvasir_repo(),
                    harness: harness("claude_code"),
                    tool_name: tool("Read"),
                    call_count: 2,
                },
            ],
            harness_summaries: vec![
                KvasirOverviewHarnessSummary {
                    harness: harness("unknown"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 0,
                        cost_usd_nanos: 1_950_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 0,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_782_043_200_000,
                    },
                },
                KvasirOverviewHarnessSummary {
                    harness: harness("claude_code"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 5_000,
                        cost_usd_nanos: 18_015_000,
                        cost_source: Some(KvasirCostSource::Estimated),
                        tool_calls: 3,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_782_043_200_000,
                    },
                },
            ],
            session_summaries: Vec::new(),
            session_summaries_more_available: 0,
            prompt_summaries: Vec::new(),
            prompt_summaries_more_available: 0,
        }
    );
    assert_eq!(
        snapshot,
        KvasirOverviewSnapshot {
            totals: KvasirOverviewTotals {
                total_tokens: 5_000,
                cost_usd_nanos: 1_968_015_000,
                cost_source: Some(KvasirCostSource::Mixed),
                tool_calls: 3,
            },
            series: vec![
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    total_tokens: 2_150,
                    cost_usd_nanos: 1_450_000_000,
                    cost_source: Some(KvasirCostSource::Native),
                    tool_calls: 3,
                },
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    total_tokens: 2_850,
                    cost_usd_nanos: 518_015_000,
                    cost_source: Some(KvasirCostSource::Mixed),
                    tool_calls: 0,
                },
            ],
            repo_breakdown: vec![KvasirOverviewRepoSummary {
                repo: kvasir_repo(),
                totals: KvasirOverviewTotals {
                    total_tokens: 5_000,
                    cost_usd_nanos: 1_968_015_000,
                    cost_source: Some(KvasirCostSource::Mixed),
                    tool_calls: 3,
                },
            }],
            model_breakdown: vec![
                KvasirOverviewModelSummary {
                    model: model("claude-sonnet-4-20250514"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 3_300,
                        cost_usd_nanos: 218_015_000,
                        cost_source: Some(KvasirCostSource::Mixed),
                        tool_calls: 0,
                    },
                },
                KvasirOverviewModelSummary {
                    model: model("claude-opus-4-20250514"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 1_700,
                        cost_usd_nanos: 1_750_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 0,
                    },
                },
            ],
            harness_breakdown: vec![
                KvasirOverviewHarnessSummary {
                    harness: harness("unknown"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 0,
                        cost_usd_nanos: 1_950_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 0,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_782_043_200_000,
                    },
                },
                KvasirOverviewHarnessSummary {
                    harness: harness("claude_code"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 5_000,
                        cost_usd_nanos: 18_015_000,
                        cost_source: Some(KvasirCostSource::Estimated),
                        tool_calls: 3,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_782_043_200_000,
                    },
                },
            ],
            session_breakdown: vec![],
            session_breakdown_more_available: 0,
            prompt_breakdown: vec![],
            prompt_breakdown_more_available: 0,
            selected_repo: Some(kvasir_repo()),
            selected_harness: None,
            selected_model: None,
            selected_session: None,
            selected_prompt: None,
            dimensions: vec![],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_scopes_overview_snapshot_by_selected_model() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;
    let http_client = reqwest::Client::new();

    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(native_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let selected_model = model("claude-sonnet-4-20250514");
    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: Some(selected_model.clone()),
        session: None,
        prompt: None,
    };
    let snapshot = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.overview_snapshot(query)
    })
    .await??;

    assert_eq!(
        snapshot,
        KvasirOverviewSnapshot {
            totals: KvasirOverviewTotals {
                total_tokens: 3_300,
                cost_usd_nanos: 218_015_000,
                cost_source: Some(KvasirCostSource::Mixed),
                tool_calls: 0,
            },
            series: vec![
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    total_tokens: 450,
                    cost_usd_nanos: 200_000_000,
                    cost_source: Some(KvasirCostSource::Native),
                    tool_calls: 0,
                },
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    total_tokens: 2_850,
                    cost_usd_nanos: 18_015_000,
                    cost_source: Some(KvasirCostSource::Estimated),
                    tool_calls: 0,
                },
            ],
            repo_breakdown: vec![KvasirOverviewRepoSummary {
                repo: kvasir_repo(),
                totals: KvasirOverviewTotals {
                    total_tokens: 3_300,
                    cost_usd_nanos: 218_015_000,
                    cost_source: Some(KvasirCostSource::Mixed),
                    tool_calls: 0,
                },
            }],
            model_breakdown: vec![KvasirOverviewModelSummary {
                model: selected_model.clone(),
                totals: KvasirOverviewTotals {
                    total_tokens: 3_300,
                    cost_usd_nanos: 218_015_000,
                    cost_source: Some(KvasirCostSource::Mixed),
                    tool_calls: 0,
                },
            }],
            harness_breakdown: vec![
                KvasirOverviewHarnessSummary {
                    harness: harness("unknown"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 0,
                        cost_usd_nanos: 200_000_000,
                        cost_source: Some(KvasirCostSource::Native),
                        tool_calls: 0,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_781_962_200_000,
                    },
                },
                KvasirOverviewHarnessSummary {
                    harness: harness("claude_code"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 3_300,
                        cost_usd_nanos: 18_015_000,
                        cost_source: Some(KvasirCostSource::Estimated),
                        tool_calls: 0,
                    },
                    last_activity: KvasirTimestampMillis {
                        value: 1_782_043_200_000,
                    },
                },
            ],
            session_breakdown: vec![],
            session_breakdown_more_available: 0,
            prompt_breakdown: vec![],
            prompt_breakdown_more_available: 0,
            selected_repo: Some(kvasir_repo()),
            selected_harness: None,
            selected_model: Some(selected_model),
            selected_session: None,
            selected_prompt: None,
            dimensions: vec![],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_scopes_overview_snapshot_by_selected_harness() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;
    let http_client = reqwest::Client::new();

    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(native_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;
    http_client
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let selected_harness = harness("claude_code");
    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: Some(selected_harness.clone()),
        model: None,
        session: None,
        prompt: None,
    };
    let snapshot = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.overview_snapshot(query)
    })
    .await??;

    // The native-cost fixture lands under the "unknown" harness, so scoping to
    // claude_code must drop its 1_950_000_000 nanos and keep only the
    // claude_code-attributed tokens, estimated cost, and tool calls. Before the
    // fix these were all zeroed because token/cost rollups bailed out whenever a
    // harness filter was present.
    assert_eq!(
        snapshot,
        KvasirOverviewSnapshot {
            totals: KvasirOverviewTotals {
                total_tokens: 5_000,
                cost_usd_nanos: 18_015_000,
                cost_source: Some(KvasirCostSource::Estimated),
                tool_calls: 3,
            },
            series: vec![
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 20,
                    },
                    total_tokens: 2_150,
                    cost_usd_nanos: 0,
                    cost_source: None,
                    tool_calls: 3,
                },
                KvasirOverviewSeriesPoint {
                    day: KvasirRollupDay {
                        year: 2026,
                        month: 6,
                        day: 21,
                    },
                    total_tokens: 2_850,
                    cost_usd_nanos: 18_015_000,
                    cost_source: Some(KvasirCostSource::Estimated),
                    tool_calls: 0,
                },
            ],
            repo_breakdown: vec![KvasirOverviewRepoSummary {
                repo: kvasir_repo(),
                totals: KvasirOverviewTotals {
                    total_tokens: 5_000,
                    cost_usd_nanos: 18_015_000,
                    cost_source: Some(KvasirCostSource::Estimated),
                    tool_calls: 3,
                },
            }],
            model_breakdown: vec![
                KvasirOverviewModelSummary {
                    model: model("claude-sonnet-4-20250514"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 3_300,
                        cost_usd_nanos: 18_015_000,
                        cost_source: Some(KvasirCostSource::Estimated),
                        tool_calls: 0,
                    },
                },
                KvasirOverviewModelSummary {
                    model: model("claude-opus-4-20250514"),
                    totals: KvasirOverviewTotals {
                        total_tokens: 1_700,
                        cost_usd_nanos: 0,
                        cost_source: None,
                        tool_calls: 0,
                    },
                },
            ],
            harness_breakdown: vec![KvasirOverviewHarnessSummary {
                harness: harness("claude_code"),
                totals: KvasirOverviewTotals {
                    total_tokens: 5_000,
                    cost_usd_nanos: 18_015_000,
                    cost_source: Some(KvasirCostSource::Estimated),
                    tool_calls: 3,
                },
                last_activity: KvasirTimestampMillis {
                    value: 1_782_043_200_000,
                },
            }],
            session_breakdown: vec![],
            session_breakdown_more_available: 0,
            prompt_breakdown: vec![],
            prompt_breakdown_more_available: 0,
            selected_repo: Some(kvasir_repo()),
            selected_harness: Some(selected_harness),
            selected_model: None,
            selected_session: None,
            selected_prompt: None,
            dimensions: vec![],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_deep_scoped_overview_snapshot_does_not_leak_aggregate_rollups() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;
    let http_client = reqwest::Client::new();

    http_client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!(
            "../../kvasird/tests/fixtures/claude_token_usage_otlp.json"
        ))
        .send()
        .await?
        .error_for_status()?;

    let selected_session = KvasirOverviewSessionRoute {
        harness: harness("claude_code"),
        session_id: session("session-12"),
    };
    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: None,
        session: Some(selected_session.clone()),
        prompt: None,
    };
    let snapshot = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.overview_snapshot(query)
    })
    .await??;

    assert_eq!(
        snapshot,
        KvasirOverviewSnapshot {
            totals: KvasirOverviewTotals {
                total_tokens: 0,
                cost_usd_nanos: 0,
                cost_source: None,
                tool_calls: 0,
            },
            series: vec![],
            repo_breakdown: vec![],
            model_breakdown: vec![],
            harness_breakdown: vec![],
            session_breakdown: vec![],
            session_breakdown_more_available: 0,
            prompt_breakdown: vec![],
            prompt_breakdown_more_available: 0,
            selected_repo: Some(kvasir_repo()),
            selected_harness: Some(harness("claude_code")),
            selected_model: None,
            selected_session: Some(selected_session),
            selected_prompt: None,
            dimensions: vec![],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_subscription_delivers_live_token_rollup_updates() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
    let (first_update_sender, first_update_receiver) = tokio::sync::oneshot::channel();
    let subscription_task = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        let subscription = client.subscribe_token_rollups(query)?;
        let initial = subscription.next()?;
        ready_sender.send(initial).expect("test receiver is alive");
        let first_update = subscription.next()?;
        first_update_sender
            .send(first_update)
            .expect("test receiver is alive");
        subscription.next()
    });

    let initial = match tokio::time::timeout(Duration::from_secs(2), ready_receiver).await {
        Ok(Ok(initial)) => initial,
        Ok(Err(err)) if subscription_task.is_finished() => {
            let result = subscription_task.await?;
            return Err(anyhow::anyhow!(
                "subscription task ended before readiness: {result:?}; receiver: {err}"
            ));
        }
        Ok(Err(err)) => return Err(err.into()),
        Err(err) if subscription_task.is_finished() => {
            let result = subscription_task.await?;
            return Err(anyhow::anyhow!(
                "subscription task ended before readiness: {result:?}; timeout: {err}"
            ));
        }
        Err(err) => return Err(err.into()),
    };
    assert_eq!(initial, KvasirTokenRollupUpdate { rollups: vec![] });

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(repo_and_other_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    let first_update =
        tokio::time::timeout(Duration::from_secs(2), first_update_receiver).await??;
    assert_eq!(
        first_update,
        KvasirTokenRollupUpdate {
            rollups: vec![KvasirTokenRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-opus-4-20250514"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            },],
        }
    );

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(other_repo_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(second_repo_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    let second_update = subscription_task.await??;
    assert_eq!(
        second_update,
        KvasirTokenRollupUpdate {
            rollups: vec![KvasirTokenRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-opus-4-20250514"),
                input_tokens: 125,
                output_tokens: 0,
                cache_tokens: 0,
            },],
        }
    );

    Ok(())
}

#[tokio::test]
async fn client_subscription_delivers_live_usage_update_notifications() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
    let (first_update_sender, first_update_receiver) = tokio::sync::oneshot::channel();
    let subscription_task = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        let subscription = client.subscribe_usage_updates()?;
        assert_eq!(subscription.next()?, KvasirUsageUpdateKind::Initial);
        ready_sender.send(()).expect("test receiver is alive");
        assert_eq!(subscription.next()?, KvasirUsageUpdateKind::Changed);
        first_update_sender
            .send(())
            .expect("test receiver is alive");
        assert_eq!(subscription.next()?, KvasirUsageUpdateKind::Changed);
        Ok::<(), KvasirClientError>(())
    });

    match tokio::time::timeout(Duration::from_secs(2), ready_receiver).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) if subscription_task.is_finished() => {
            let result = subscription_task.await?;
            return Err(anyhow::anyhow!(
                "subscription task ended before readiness: {result:?}; receiver: {err}"
            ));
        }
        Ok(Err(err)) => return Err(err.into()),
        Err(err) if subscription_task.is_finished() => {
            let result = subscription_task.await?;
            return Err(anyhow::anyhow!(
                "subscription task ended before readiness: {result:?}; timeout: {err}"
            ));
        }
        Err(err) => return Err(err.into()),
    }

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(repo_and_other_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    tokio::time::timeout(Duration::from_secs(2), first_update_receiver).await??;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(other_repo_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    tokio::time::timeout(Duration::from_secs(2), subscription_task).await???;

    Ok(())
}

#[tokio::test]
async fn usage_update_subscription_close_unblocks_waiting_reader() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let subscription = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.subscribe_usage_updates().map(Arc::new)
    })
    .await??;
    let reader_subscription = Arc::clone(&subscription);
    let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
    let reader_task = tokio::task::spawn_blocking(move || {
        assert_eq!(reader_subscription.next()?, KvasirUsageUpdateKind::Initial);
        ready_sender.send(()).expect("test receiver is alive");
        reader_subscription.next()
    });

    tokio::time::timeout(Duration::from_secs(2), ready_receiver).await??;
    subscription.close()?;

    let result = tokio::time::timeout(Duration::from_secs(2), reader_task).await??;
    assert!(matches!(result, Err(KvasirClientError::SocketIo)));

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_skips_initial_and_delivers_changes() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let server = ControlledUsageUpdateServer::start(rpc_socket_path.clone())?;
    let subscription = Arc::new(KvasirOverviewRefreshSubscription::connect(socket_path(
        rpc_socket_path,
    ))?);
    let waiting_subscription = Arc::clone(&subscription);
    let refresh_task = tokio::task::spawn_blocking(move || waiting_subscription.next());

    server.wait_until_initial_sent()?;
    assert!(
        !refresh_task.is_finished(),
        "initial subscription event must not request a dashboard refresh"
    );
    server.send_changed()?;

    tokio::time::timeout(Duration::from_secs(2), refresh_task).await???;

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_reconnects_and_delivers_later_initial() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: database_path.clone(),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let subscription = {
        let client = KvasirClient::connect(socket_path(rpc_socket_path.clone()))?;
        Arc::new(client.subscribe_overview_refreshes()?)
    };
    let waiting_subscription = Arc::clone(&subscription);
    let refresh_task = tokio::task::spawn_blocking(move || waiting_subscription.next());

    post_usage_fixture(daemon.otlp_addr(), repo_and_other_token_usage_fixture()).await?;
    tokio::time::timeout(Duration::from_secs(2), refresh_task).await???;

    let waiting_subscription = Arc::clone(&subscription);
    let refresh_task = tokio::task::spawn_blocking(move || waiting_subscription.next());
    drop(daemon);
    let _replacement_daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path,
            database_path,
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    tokio::time::timeout(Duration::from_secs(5), refresh_task).await???;

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_waits_for_initial_daemon_connection() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let subscription = Arc::new(KvasirOverviewRefreshSubscription::connect(socket_path(
        rpc_socket_path.clone(),
    ))?);
    let waiting_subscription = Arc::clone(&subscription);
    let refresh_task = tokio::task::spawn_blocking(move || waiting_subscription.next());

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !refresh_task.is_finished(),
        "missing daemon socket must not terminate the overview refresh subscription"
    );

    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path,
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;
    let mut use_other_repo = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !refresh_task.is_finished() && tokio::time::Instant::now() < deadline {
        let fixture = if use_other_repo {
            other_repo_token_usage_fixture()
        } else {
            repo_and_other_token_usage_fixture()
        };
        post_usage_fixture(daemon.otlp_addr(), fixture).await?;
        use_other_repo = !use_other_repo;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tokio::time::timeout(Duration::from_secs(3), refresh_task).await???;

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_close_unblocks_initial_reconnect_wait() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let subscription = Arc::new(KvasirOverviewRefreshSubscription::connect(socket_path(
        rpc_socket_path,
    ))?);
    let waiting_subscription = Arc::clone(&subscription);
    let reader_task = tokio::task::spawn_blocking(move || waiting_subscription.next());

    tokio::time::sleep(Duration::from_millis(100)).await;
    subscription.close()?;

    let result = tokio::time::timeout(Duration::from_millis(500), reader_task).await??;
    assert!(matches!(result, Err(KvasirClientError::SocketIo)));

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_close_unblocks_concurrent_waiters() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let subscription = Arc::new(KvasirOverviewRefreshSubscription::connect(socket_path(
        rpc_socket_path,
    ))?);
    let first_subscription = Arc::clone(&subscription);
    let second_subscription = Arc::clone(&subscription);
    let first_task = tokio::task::spawn_blocking(move || first_subscription.next());
    let second_task = tokio::task::spawn_blocking(move || second_subscription.next());

    tokio::time::sleep(Duration::from_millis(100)).await;
    subscription.close()?;

    let first_result = tokio::time::timeout(Duration::from_millis(500), first_task).await??;
    let second_result = tokio::time::timeout(Duration::from_millis(500), second_task).await??;
    assert!(matches!(first_result, Err(KvasirClientError::SocketIo)));
    assert!(matches!(second_result, Err(KvasirClientError::SocketIo)));

    Ok(())
}

#[tokio::test]
async fn overview_refresh_subscription_close_unblocks_waiting_reader() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let server = ControlledUsageUpdateServer::start(rpc_socket_path.clone())?;
    let subscription = Arc::new(KvasirOverviewRefreshSubscription::connect(socket_path(
        rpc_socket_path,
    ))?);
    let waiting_subscription = Arc::clone(&subscription);
    let reader_task = tokio::task::spawn_blocking(move || waiting_subscription.next());

    server.wait_until_initial_sent()?;
    subscription.close()?;

    let result = tokio::time::timeout(Duration::from_millis(500), reader_task).await??;
    assert!(matches!(result, Err(KvasirClientError::SocketIo)));

    Ok(())
}

#[tokio::test]
async fn client_queries_cost_rollups_through_daemon_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(native_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let rollups = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.cost_rollups(query)
    })
    .await??;

    assert_eq!(
        rollups,
        vec![
            KvasirCostRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-opus-4-20250514"),
                cost_usd: KvasirCostUsd {
                    nanos: 1_250_000_000,
                },
                source: KvasirCostSource::Native,
            },
            KvasirCostRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                model: model("claude-sonnet-4-20250514"),
                cost_usd: KvasirCostUsd { nanos: 200_000_000 },
                source: KvasirCostSource::Native,
            },
            KvasirCostRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 21,
                },
                repo: kvasir_repo(),
                model: model("claude-opus-4-20250514"),
                cost_usd: KvasirCostUsd { nanos: 500_000_000 },
                source: KvasirCostSource::Native,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn client_queries_tool_call_rollups_through_daemon_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let rollups = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.tool_call_rollups(query)
    })
    .await??;

    assert_eq!(
        rollups,
        vec![
            KvasirToolCallRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                harness: harness("claude_code"),
                tool_name: tool("Bash"),
                call_count: 1,
            },
            KvasirToolCallRollup {
                day: KvasirRollupDay {
                    year: 2026,
                    month: 6,
                    day: 20,
                },
                repo: kvasir_repo(),
                harness: harness("claude_code"),
                tool_name: tool("Read"),
                call_count: 2,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn client_connect_defers_socket_io_until_rpc() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("missing.sock");

    let client = KvasirClient::connect(socket_path(rpc_socket_path))?;

    assert!(matches!(
        client.overview_snapshot(empty_query()).unwrap_err(),
        KvasirClientError::SocketIo
    ));
    Ok(())
}

#[tokio::test]
async fn client_rpc_retries_until_daemon_socket_is_available() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let connecting_socket_path = rpc_socket_path.clone();
    let connecting = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(connecting_socket_path))?;
        client.overview_snapshot(empty_query())
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path,
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let snapshot = connecting.await??;

    assert_eq!(snapshot.totals.total_tokens, 0);
    Ok(())
}

#[tokio::test]
async fn client_reports_response_too_large_for_oversized_daemon_query() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(many_model_token_usage_fixture(7_000))
        .send()
        .await?
        .error_for_status()?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: None,
        harness: None,
        model: None,
        session: None,
        prompt: None,
    };
    let error = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.token_rollups(query)
    })
    .await?
    .expect_err("oversized daemon response should surface as a typed client error");

    assert!(matches!(error, KvasirClientError::RpcResponseTooLarge));

    Ok(())
}

fn kvasir_repo() -> KvasirRepoBucket {
    KvasirRepoBucket {
        kind: KvasirRepoBucketKind::Repo,
        name: Some(KvasirRepoName::try_from("kvasir".to_owned()).unwrap()),
        path: Some(KvasirRepoPath::try_from("/Users/oyr/projects/kvasir".to_owned()).unwrap()),
    }
}

fn core_repo() -> RepoBucket {
    RepoBucket::repo(RepoIdentity::new(
        CoreRepoName::new("kvasir"),
        CoreRepoPath::new("/Users/oyr/projects/kvasir"),
    ))
}

fn valid_panel_request() -> KvasirUsageRollupExplorerPanelRequest {
    KvasirUsageRollupExplorerPanelRequest {
        time_range: KvasirExplorerTimeRange {
            start: timestamp(2026, 6, 19),
            end: timestamp(2026, 6, 22),
        },
        filters: Vec::new(),
        saved_panel: Some(KvasirExplorerSavedPanelDefinition {
            panel: KvasirExplorerSavedPanel::UsageRollupsOverview,
            dataset: KvasirExplorerDataset::UsageRollups,
            measures: vec![
                KvasirExplorerMeasure::TotalTokens,
                KvasirExplorerMeasure::CostUsd,
            ],
            group_by: vec![KvasirExplorerDimension::Day, KvasirExplorerDimension::Repo],
            filters: Vec::new(),
            visualization: KvasirExplorerVisualization::Table,
            limit: 50,
        }),
    }
}

fn core_usage_rollup_saved_panel() -> CoreExplorerSavedPanelDefinition {
    CoreExplorerSavedPanelDefinition {
        panel: CoreExplorerSavedPanel::UsageRollupsOverview,
        dataset: ExplorerDataset::UsageRollups,
        measures: vec![ExplorerMeasure::TotalTokens, ExplorerMeasure::CostUsd],
        group_by: vec![ExplorerDimension::Day, ExplorerDimension::Repo],
        filters: Vec::new(),
        visualization: ExplorerVisualization::Table,
        limit: 50,
    }
}

fn core_usage_rollup_catalog_with_measures(
    measures: Vec<ExplorerMeasure>,
) -> ExplorerDatasetCatalog {
    ExplorerDatasetCatalog {
        dataset: ExplorerDataset::UsageRollups,
        measures,
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
    }
}

fn start_scripted_rpc_server(
    socket_path: std::path::PathBuf,
    responses: Vec<RpcResponse>,
) -> anyhow::Result<thread::JoinHandle<anyhow::Result<()>>> {
    let listener = UnixListener::bind(socket_path)?;
    Ok(thread::spawn(move || {
        for response in responses {
            let (mut stream, _addr) = listener.accept()?;
            let mut request = String::new();
            BufReader::new(stream.try_clone()?).read_line(&mut request)?;
            let mut payload = serde_json::to_vec(&response)?;
            payload.push(b'\n');
            stream.write_all(&payload)?;
        }
        Ok(())
    }))
}

fn socket_path(path: std::path::PathBuf) -> KvasirSocketPath {
    KvasirSocketPath::try_from(path.to_string_lossy().into_owned()).unwrap()
}

fn empty_query() -> KvasirRollupQuery {
    KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: None,
        harness: None,
        model: None,
        session: None,
        prompt: None,
    }
}

async fn post_usage_fixture(otlp_addr: SocketAddr, body: &'static str) -> anyhow::Result<()> {
    reqwest::Client::new()
        .post(format!("http://{otlp_addr}/v1/metrics"))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

struct ControlledUsageUpdateServer {
    command_sender: mpsc::Sender<ControlledUsageUpdateCommand>,
    initial_receiver: mpsc::Receiver<anyhow::Result<()>>,
    _thread: thread::JoinHandle<()>,
}

enum ControlledUsageUpdateCommand {
    Changed,
}

impl ControlledUsageUpdateServer {
    fn start(socket_path: std::path::PathBuf) -> anyhow::Result<Self> {
        let listener = UnixListener::bind(socket_path)?;
        let (initial_sender, initial_receiver) = mpsc::channel();
        let (command_sender, command_receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            if let Err(err) =
                run_controlled_usage_update_server(listener, initial_sender, command_receiver)
            {
                eprintln!("controlled usage update server failed: {err:#}");
            }
        });
        Ok(Self {
            command_sender,
            initial_receiver,
            _thread: thread,
        })
    }

    fn wait_until_initial_sent(&self) -> anyhow::Result<()> {
        self.initial_receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|err| anyhow::anyhow!("usage update server did not send initial: {err}"))?
    }

    fn send_changed(&self) -> anyhow::Result<()> {
        self.command_sender
            .send(ControlledUsageUpdateCommand::Changed)
            .map_err(|err| {
                anyhow::anyhow!("usage update server stopped before changed event: {err}")
            })
    }
}

fn run_controlled_usage_update_server(
    listener: UnixListener,
    initial_sender: mpsc::Sender<anyhow::Result<()>>,
    command_receiver: mpsc::Receiver<ControlledUsageUpdateCommand>,
) -> anyhow::Result<()> {
    let (stream, _addr) = match listener.accept() {
        Ok(connection) => connection,
        Err(err) => {
            let _ = initial_sender.send(Err(err.into()));
            return Ok(());
        }
    };
    let mut reader = match stream.try_clone().map(BufReader::new) {
        Ok(reader) => reader,
        Err(err) => {
            let _ = initial_sender.send(Err(err.into()));
            return Ok(());
        }
    };
    let mut request = String::new();
    if let Err(err) = reader.read_line(&mut request) {
        let _ = initial_sender.send(Err(err.into()));
        return Ok(());
    }
    match serde_json::from_str::<RpcRequest>(&request) {
        Ok(RpcRequest::SubscribeUsageUpdates) => {}
        Ok(other) => {
            let _ = initial_sender.send(Err(anyhow::anyhow!(
                "unexpected subscription request: {other:?}"
            )));
            return Ok(());
        }
        Err(err) => {
            let _ = initial_sender.send(Err(err.into()));
            return Ok(());
        }
    }

    let mut writer = stream;
    match write_usage_update_event(&mut writer, UsageUpdateKind::Initial) {
        Ok(()) => {
            let _ = initial_sender.send(Ok(()));
        }
        Err(err) => {
            let _ = initial_sender.send(Err(err));
            return Ok(());
        }
    }

    for command in command_receiver {
        match command {
            ControlledUsageUpdateCommand::Changed => {
                write_usage_update_event(&mut writer, UsageUpdateKind::Changed)?;
            }
        }
    }
    Ok(())
}

fn write_usage_update_event(writer: &mut UnixStream, kind: UsageUpdateKind) -> anyhow::Result<()> {
    let mut event = serde_json::to_vec(&RpcStreamEvent::UsageUpdate { kind })?;
    event.push(b'\n');
    writer.write_all(&event)?;
    Ok(())
}

fn raw_rpc_request(socket_path: &Path, request: &str) -> anyhow::Result<RpcResponse> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(serde_json::from_str(&response)?)
}

fn timestamp(year: i32, month: u32, day: u32) -> KvasirTimestampMillis {
    KvasirTimestampMillis {
        value: Utc
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .unwrap()
            .timestamp_millis(),
    }
}

fn model(value: &str) -> KvasirModelName {
    KvasirModelName::try_from(value.to_owned()).unwrap()
}

fn harness(value: &str) -> KvasirHarnessName {
    KvasirHarnessName::try_from(value.to_owned()).unwrap()
}

fn tool(value: &str) -> KvasirToolName {
    KvasirToolName::try_from(value.to_owned()).unwrap()
}

fn session(value: &str) -> KvasirSessionId {
    KvasirSessionId::try_from(value.to_owned()).unwrap()
}

fn prompt(value: &str) -> KvasirPromptId {
    KvasirPromptId::try_from(value.to_owned()).unwrap()
}

fn content_text(value: &str) -> KvasirContentText {
    KvasirContentText::try_from(value.to_owned()).unwrap()
}

fn load_content_replay(
    rpc_socket_path: std::path::PathBuf,
    harness: KvasirHarnessName,
    session_id: KvasirSessionId,
    prompt_id: KvasirPromptId,
) -> Result<KvasirContentReplay, KvasirClientError> {
    let request = RpcRequest::Content {
        query: ContentQuery {
            harness: CoreHarnessName::new(String::from(harness)),
            session_id: CoreSessionId::new(String::from(session_id)),
            prompt_id: CorePromptId::new(String::from(prompt_id)),
        },
        bearer_token: BearerToken::new("test-token"),
    };
    let request =
        serde_json::to_string(&request).map_err(|_| KvasirClientError::RpcSerialization)?;
    let response =
        raw_rpc_request(&rpc_socket_path, &request).map_err(|_| KvasirClientError::SocketIo)?;

    match response {
        RpcResponse::Content { replay } => replay.try_into(),
        RpcResponse::Error { error } => Err(error.into()),
        _ => Err(KvasirClientError::WrongResponseType),
    }
}

fn trace_id(value: &str) -> KvasirTraceId {
    KvasirTraceId::try_from(value.to_owned()).unwrap()
}

fn span(value: &str) -> KvasirSpanId {
    KvasirSpanId::try_from(value.to_owned()).unwrap()
}

fn span_name(value: &str) -> KvasirSpanName {
    KvasirSpanName::try_from(value.to_owned()).unwrap()
}

fn claude_trace_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "claude" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "session-12" } },
                    { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "1111111111111111",
                        "name": "claude.interaction",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802750000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    },
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "2222222222222222",
                        "parentSpanId": "1111111111111111",
                        "name": "claude.llm_request",
                        "startTimeUnixNano": "1781956800250000000",
                        "endTimeUnixNano": "1781956802250000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "llm_request" } }
                        ]
                    },
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "3333333333333333",
                        "parentSpanId": "1111111111111111",
                        "name": "claude.tool",
                        "startTimeUnixNano": "1781956802250000000",
                        "endTimeUnixNano": "1781956802750000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "tool" } },
                            { "key": "tool.name", "value": { "stringValue": "Read" } }
                        ]
                    },
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "4444444444444444",
                        "parentSpanId": "1111111111111111",
                        "name": "claude.llm_request",
                        "startTimeUnixNano": "1781956803000000000",
                        "endTimeUnixNano": "1781956804000000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "llm_request" } }
                        ]
                    },
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "5555555555555555",
                        "parentSpanId": "1111111111111111",
                        "name": "claude.tool",
                        "startTimeUnixNano": "1781956804000000000",
                        "endTimeUnixNano": "1781956804250000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "tool" } },
                            { "key": "tool.name", "value": { "stringValue": "Bash" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn two_trace_ids_for_same_prompt_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "claude" } },
                    { "key": "session.id", "value": { "stringValue": "session-12" } },
                    { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "spanId": "1111111111111111",
                        "name": "claude.llm_request",
                        "startTimeUnixNano": "1781956800250000000",
                        "endTimeUnixNano": "1781956802250000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "llm_request" } }
                        ]
                    },
                    {
                        "traceId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "spanId": "2222222222222222",
                        "name": "claude.llm_request",
                        "startTimeUnixNano": "1781956803250000000",
                        "endTimeUnixNano": "1781956804250000000",
                        "attributes": [
                            { "key": "claude.span.kind", "value": { "stringValue": "llm_request" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn codex_trace_reusing_claude_identity_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "codex" } },
                    { "key": "session.id", "value": { "stringValue": "session-12" } },
                    { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                ]
            },
            "scopeSpans": [{
                "spans": [{
                    "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "spanId": "9999999999999999",
                    "name": "codex.interaction",
                    "startTimeUnixNano": "1781956805000000000",
                    "endTimeUnixNano": "1781956806000000000",
                    "attributes": [
                        { "key": "span.kind", "value": { "stringValue": "interaction" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn large_claude_trace_fixture(span_count: usize) -> String {
    let mut spans = String::new();
    for index in 0..span_count {
        if index > 0 {
            spans.push(',');
        }
        let span_id = format!("{:016x}", index + 1);
        let start_ns = 1_781_956_800_000_000_000_u64 + (index as u64 * 1_000_000);
        let end_ns = start_ns + 1_000_000;
        write!(
            spans,
            r#"{{
                "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "spanId": "{span_id}",
                "name": "claude.llm_request",
                "startTimeUnixNano": "{start_ns}",
                "endTimeUnixNano": "{end_ns}",
                "attributes": [
                    {{ "key": "claude.span.kind", "value": {{ "stringValue": "llm_request" }} }}
                ]
            }}"#
        )
        .expect("writing to a String cannot fail");
    }

    format!(
        r#"{{
        "resourceSpans": [{{
            "resource": {{
                "attributes": [
                    {{ "key": "service.name", "value": {{ "stringValue": "claude" }} }},
                    {{ "key": "session.id", "value": {{ "stringValue": "session-large" }} }},
                    {{ "key": "prompt.id", "value": {{ "stringValue": "prompt-large" }} }}
                ]
            }},
            "scopeSpans": [{{
                "spans": [{spans}]
            }}]
        }}]
    }}"#
    )
}

fn opencode_trace_content_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "opencode" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "opencode-session-1" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "cccccccccccccccccccccccccccccccc",
                        "spanId": "2222222222222222",
                        "name": "ai.generateText.doGenerate",
                        "startTimeUnixNano": "1781956800120000000",
                        "endTimeUnixNano": "1781956801920000000",
                        "attributes": [
                            { "key": "message.id", "value": { "stringValue": "opencode-turn-1" } },
                            { "key": "ai.operationId", "value": { "stringValue": "ai.generateText" } },
                            { "key": "ai.model.id", "value": { "stringValue": "gpt-4.1" } },
                            { "key": "ai.prompt.messages", "value": { "stringValue": "summarize README.md" } },
                            { "key": "ai.response.text", "value": { "stringValue": "I need to read it first." } }
                        ]
                    },
                    {
                        "traceId": "cccccccccccccccccccccccccccccccc",
                        "spanId": "3333333333333333",
                        "parentSpanId": "2222222222222222",
                        "name": "execute Read",
                        "startTimeUnixNano": "1781956801920000000",
                        "endTimeUnixNano": "1781956802170000000",
                        "attributes": [
                            { "key": "message.id", "value": { "stringValue": "opencode-turn-1" } },
                            { "key": "ai.operationId", "value": { "stringValue": "toolCall" } },
                            { "key": "ai.toolCall.name", "value": { "stringValue": "Read" } },
                            { "key": "ai.toolCall.args", "value": { "stringValue": "{\"path\":\"README.md\"}" } },
                            { "key": "ai.toolCall.result", "value": { "stringValue": "kvasir is a local telemetry daemon" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn opencode_trace_content_protobuf_fixture() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("service.name", "opencode"),
                    string_attribute("repo.name", "kvasir"),
                    string_attribute("repo.path", "/Users/oyr/projects/kvasir"),
                    string_attribute("session.id", "opencode-session-1"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![
                    Span {
                        trace_id: hex_bytes("cccccccccccccccccccccccccccccccc"),
                        span_id: hex_bytes("2222222222222222"),
                        trace_state: String::new(),
                        parent_span_id: Vec::new(),
                        flags: 0,
                        name: "ai.generateText.doGenerate".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_120_000_000,
                        end_time_unix_nano: 1_781_956_801_920_000_000,
                        attributes: vec![
                            string_attribute("message.id", "opencode-turn-1"),
                            string_attribute("ai.operationId", "ai.generateText"),
                            string_attribute("ai.model.id", "gpt-4.1"),
                            string_attribute("ai.prompt.messages", "summarize README.md"),
                            string_attribute("ai.response.text", "I need to read it first."),
                        ],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                    Span {
                        trace_id: hex_bytes("cccccccccccccccccccccccccccccccc"),
                        span_id: hex_bytes("3333333333333333"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("2222222222222222"),
                        flags: 0,
                        name: "execute Read".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_801_920_000_000,
                        end_time_unix_nano: 1_781_956_802_170_000_000,
                        attributes: vec![
                            string_attribute("message.id", "opencode-turn-1"),
                            string_attribute("ai.operationId", "toolCall"),
                            string_attribute("ai.toolCall.name", "Read"),
                            string_attribute("ai.toolCall.args", r#"{"path":"README.md"}"#),
                            string_attribute(
                                "ai.toolCall.result",
                                "kvasir is a local telemetry daemon",
                            ),
                        ],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
    .encode_to_vec()
}

fn unsupported_harness_trace_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "unknown-harness" } },
                    { "key": "session.id", "value": { "stringValue": "unknown-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "unknown-turn-1" } }
                ]
            },
            "scopeSpans": [{
                "spans": [{
                    "traceId": "dddddddddddddddddddddddddddddddd",
                    "spanId": "1111111111111111",
                    "name": "unknown.interaction",
                    "startTimeUnixNano": "1781956800000000000",
                    "endTimeUnixNano": "1781956800100000000",
                    "attributes": [
                        { "key": "span.kind", "value": { "stringValue": "interaction" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn opencode_content_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "opencode" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "opencode-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "opencode-turn-1" } }
                ]
            },
            "scopeLogs": [{
                "logRecords": [{
                    "timeUnixNano": "1781956802180000000",
                    "eventName": "opencode.content",
                    "body": { "stringValue": "stored assistant text" },
                    "attributes": [
                        { "key": "content.opt_in", "value": { "boolValue": true } },
                        { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn claude_trace_protobuf_fixture() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("service.name", "claude"),
                    string_attribute("session.id", "session-12"),
                    string_attribute("prompt.id", "prompt-7"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![Span {
                    trace_id: hex_bytes("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                    span_id: hex_bytes("1111111111111111"),
                    trace_state: String::new(),
                    parent_span_id: Vec::new(),
                    flags: 0,
                    name: "claude.llm_request".to_owned(),
                    kind: 0,
                    start_time_unix_nano: 1_781_956_800_250_000_000,
                    end_time_unix_nano: 1_781_956_802_250_000_000,
                    attributes: vec![string_attribute("claude.span.kind", "llm_request")],
                    dropped_attributes_count: 0,
                    events: Vec::new(),
                    dropped_events_count: 0,
                    links: Vec::new(),
                    dropped_links_count: 0,
                    status: None,
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
    .encode_to_vec()
}

fn string_attribute(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_owned())),
        }),
    }
}

fn hex_bytes(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = hex_nibble(chunk[0]);
            let low = hex_nibble(chunk[1]);
            (high << 4) | low
        })
        .collect()
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => panic!("test fixture contains non-hex digit"),
    }
}

fn claude_content_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "claude" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "claude-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "claude-turn-1" } }
                ]
            },
            "scopeLogs": [{
                "logRecords": [
                    {
                        "timeUnixNano": "1781956802180000000",
                        "eventName": "claude.content",
                        "body": { "stringValue": "explain this repository" },
                        "attributes": [
                            { "key": "content.opt_in", "value": { "boolValue": true } },
                            { "key": "content.type", "value": { "stringValue": "user_prompt" } }
                        ]
                    },
                    {
                        "timeUnixNano": "1781956802200000000",
                        "eventName": "claude.content",
                        "body": { "stringValue": "authorization: bearer secret" },
                        "attributes": [
                            { "key": "content.opt_in", "value": { "boolValue": true } },
                            { "key": "content.type", "value": { "stringValue": "raw_api_request" } }
                        ]
                    },
                    {
                        "timeUnixNano": "1781956802220000000",
                        "eventName": "claude.content",
                        "body": { "stringValue": "README.md contains the project overview" },
                        "attributes": [
                            { "key": "content.opt_in", "value": { "boolValue": true } },
                            { "key": "content.type", "value": { "stringValue": "tool_output" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn codex_content_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "codex" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "codex-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "codex-turn-1" } }
                ]
            },
            "scopeLogs": [{
                "logRecords": [{
                    "timeUnixNano": "1781956802180000000",
                    "eventName": "codex.content",
                    "body": { "stringValue": "codex response text" },
                    "attributes": [
                        { "key": "content.opt_in", "value": { "boolValue": true } },
                        { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn unknown_harness_content_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "random-service" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "unknown-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "unknown-turn-1" } }
                ]
            },
            "scopeLogs": [{
                "logRecords": [{
                    "timeUnixNano": "1781956802180000000",
                    "eventName": "random-service.content",
                    "body": { "stringValue": "do not store this content" },
                    "attributes": [
                        { "key": "content.opt_in", "value": { "boolValue": true } },
                        { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn claude_tool_result_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [
            {
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [
                        {
                            "timeUnixNano": "1781956800000000000",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Read" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781956900000000000",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Read" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781957000000000000",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Bash" } }
                            ]
                        }
                    ]
                }]
            },
            {
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/tmp/other-kvasir" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "Edit" } }
                        ]
                    }]
                }]
            }
        ]
    }"#
}

fn native_cost_usage_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                ]
            },
            "scopeMetrics": [{
                "metrics": [{
                    "name": "cost.usage",
                    "sum": {
                        "dataPoints": [{
                            "startTimeUnixNano": "1781956700000000000",
                            "timeUnixNano": "1781956800000000000",
                            "asDouble": 1.25,
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                            ]
                        },
                        {
                            "startTimeUnixNano": "1781956700000000000",
                            "timeUnixNano": "1782043200000000000",
                            "asDouble": 1.75,
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                            ]
                        },
                        {
                            "startTimeUnixNano": "1781962100000000000",
                            "timeUnixNano": "1781962200000000000",
                            "asDouble": 0.2,
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "claude-sonnet-4-20250514" } }
                            ]
                        }]
                    }
                }]
            }]
        }]
    }"#
}

fn repo_and_other_token_usage_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [
            {
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "100",
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                    { "key": "token.type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            },
            {
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/tmp/other-kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "40",
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                    { "key": "token.type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            }
        ]
    }"#
}

fn second_repo_token_usage_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                ]
            },
            "scopeMetrics": [{
                "metrics": [{
                    "name": "token.usage",
                    "sum": {
                        "dataPoints": [{
                            "startTimeUnixNano": "1781956700000000000",
                            "timeUnixNano": "1781956900000000000",
                            "asInt": "125",
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                { "key": "token.type", "value": { "stringValue": "input" } }
                            ]
                        }]
                    }
                }]
            }]
        }]
    }"#
}

fn other_repo_token_usage_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/tmp/other-kvasir" } }
                ]
            },
            "scopeMetrics": [{
                "metrics": [{
                    "name": "token.usage",
                    "sum": {
                        "dataPoints": [{
                            "startTimeUnixNano": "1781956700000000000",
                            "timeUnixNano": "1781956900000000000",
                            "asInt": "999",
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                { "key": "token.type", "value": { "stringValue": "input" } }
                            ]
                        }]
                    }
                }]
            }]
        }]
    }"#
}

fn many_model_token_usage_fixture(model_count: usize) -> String {
    let data_points = (0..model_count)
        .map(|index| {
            format!(
                r#"{{
                    "startTimeUnixNano": "1781956700000000000",
                    "timeUnixNano": "1781956800000000000",
                    "asInt": "1",
                    "attributes": [
                        {{ "key": "model", "value": {{ "stringValue": "claude-generated-model-{index:04}" }} }},
                        {{ "key": "token.type", "value": {{ "stringValue": "input" }} }}
                    ]
                }}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"{{
            "resourceMetrics": [{{
                "resource": {{
                    "attributes": [
                        {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                        {{ "key": "repo.path", "value": {{ "stringValue": "/Users/oyr/projects/kvasir" }} }}
                    ]
                }},
                "scopeMetrics": [{{
                    "metrics": [{{
                        "name": "token.usage",
                        "sum": {{
                            "dataPoints": [{data_points}]
                        }}
                    }}]
                }}]
            }}]
        }}"#
    )
}
