use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use base64::Engine;
use chrono::{TimeZone, Utc};
use http::StatusCode;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use kvasir_core::rpc::{
    BearerToken, CostRollup, CostRollupQuery, CostSource, HarnessName, ModelName, RollupDay,
    RollupQuery, RpcRequest, RpcStreamEvent, TimestampMillis, TokenRollup, ToolCallRollup,
    ToolCallRollupQuery, ToolName,
};
use kvasir_core::{
    CostUsd, ModelTokenPrices, PriceTable, RepoBucket, RepoIdentity, RepoName, RepoPath,
};
use kvasird::{
    DaemonConfig, RunningDaemon, StoreKeySource, query_cost_rollup, query_token_rollup,
    query_tool_call_rollup, start_with_store_key_source,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{
    LogRecord, ResourceLogs as OtlpResourceLogs, ScopeLogs,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn golden_claude_metrics_replay_returns_per_model_day_rollup() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let client = reqwest::Client::new();
    let endpoint = format!("http://{}/v1/metrics", daemon.otlp_addr());
    let json_fixture = include_str!("fixtures/claude_token_usage_otlp.json");

    client
        .post(&endpoint)
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(json_fixture)
        .send()
        .await?
        .error_for_status()?;

    client
        .post(&endpoint)
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(claude_token_usage_protobuf_fixture()?)
        .send()
        .await?
        .error_for_status()?;

    let rollups = query_token_rollup(
        rpc_socket_path.clone(),
        RollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
        ),
    )
    .await?;

    assert_eq!(
        rollups,
        vec![
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 1100,
                output_tokens: 550,
                cache_tokens: 125,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                input_tokens: 300,
                output_tokens: 120,
                cache_tokens: 30,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-21")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                input_tokens: 2100,
                output_tokens: 900,
                cache_tokens: 75,
            }
        ]
    );

    let cost_rollups = query_cost_rollup(
        rpc_socket_path.clone(),
        CostRollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
        ),
    )
    .await?;

    assert_eq!(
        cost_rollups,
        vec![
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: CostUsd::from_nanos(57_937_500).unwrap(),
                source: CostSource::Estimated,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: CostUsd::from_nanos(2_709_000).unwrap(),
                source: CostSource::Estimated,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-21")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: CostUsd::from_nanos(19_822_500).unwrap(),
                source: CostSource::Estimated,
            },
        ]
    );

    let later_rollups = query_token_rollup(
        rpc_socket_path,
        RollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 20, 14, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap()),
        ),
    )
    .await?;

    assert_eq!(later_rollups, Vec::new());

    Ok(())
}

#[tokio::test]
async fn golden_copilot_metrics_replay_returns_repo_model_rollups_with_cost() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(include_str!("fixtures/copilot_token_usage_otlp.json"))
        .send()
        .await?
        .error_for_status()?;

    let token_query = RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), token_query).await?,
        vec![
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("gpt-5.4"),
                input_tokens: 100,
                output_tokens: 20,
                cache_tokens: 0,
            }
        ]
    );

    let cost_query = CostRollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_cost_rollup(rpc_socket_path, cost_query).await?,
        vec![
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(6_000_000).unwrap(),
                source: CostSource::Estimated,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(550_000).unwrap(),
                source: CostSource::Estimated,
            }
        ]
    );

    Ok(())
}

#[tokio::test]
async fn daemon_reopens_encrypted_store_with_configured_key() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let bearer_token = BearerToken::new("test-token");

    {
        let daemon = start_with_store_key_source(
            DaemonConfig {
                otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                rpc_socket_path: rpc_socket_path.clone(),
                database_path: database_path.clone(),
                bearer_token: bearer_token.clone(),
                price_table: PriceTable::bundled_defaults(),
            },
            StoreKeySource::static_key_for_test([11; 32]),
        )
        .await?;

        reqwest::Client::new()
            .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
            .header(AUTHORIZATION, "Bearer test-token")
            .header(CONTENT_TYPE, "application/json")
            .body(repo_and_no_repo_metrics_fixture())
            .send()
            .await?
            .error_for_status()?;
    }

    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path,
            bearer_token,
            price_table: PriceTable::bundled_defaults(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    assert_eq!(
        query_token_rollup(
            rpc_socket_path,
            RollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
        )
        .await?,
        vec![
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: RepoBucket::no_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 25,
                output_tokens: 0,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: other_kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 40,
                output_tokens: 0,
                cache_tokens: 0,
            },
        ]
    );

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_attributes_rollups_to_repo_and_no_repo_buckets() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(repo_and_no_repo_metrics_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    let rollups = query_token_rollup(rpc_socket_path.clone(), query.clone()).await?;

    assert_eq!(
        rollups,
        vec![
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: RepoBucket::no_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 25,
                output_tokens: 0,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 100,
                output_tokens: 0,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: other_kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 40,
                output_tokens: 0,
                cache_tokens: 0,
            },
        ]
    );

    assert_eq!(
        query_token_rollup(
            rpc_socket_path.clone(),
            query.clone().with_repo(kvasir_repo())
        )
        .await?,
        vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("claude-opus-4-20250514"),
            input_tokens: 100,
            output_tokens: 0,
            cache_tokens: 0,
        }]
    );

    assert_eq!(
        query_token_rollup(rpc_socket_path, query.with_repo(RepoBucket::no_repo())).await?,
        vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: RepoBucket::no_repo(),
            model: ModelName::new("claude-opus-4-20250514"),
            input_tokens: 25,
            output_tokens: 0,
            cache_tokens: 0,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_returns_native_cost_rollups() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(native_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = CostRollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );

    assert_eq!(
        query_cost_rollup(rpc_socket_path.clone(), query.clone()).await?,
        vec![
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: cost_usd("1.25"),
                source: CostSource::Native,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: other_kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: cost_usd("0.375"),
                source: CostSource::Native,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-21")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: cost_usd("0.5"),
                source: CostSource::Native,
            },
        ]
    );

    assert_eq!(
        query_cost_rollup(rpc_socket_path, query.with_repo(kvasir_repo())).await?,
        vec![
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: cost_usd("1.25"),
                source: CostSource::Native,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-sonnet-4-20250514"),
                cost_usd: cost_usd("0.2"),
                source: CostSource::Native,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-21")?,
                repo: kvasir_repo(),
                model: ModelName::new("claude-opus-4-20250514"),
                cost_usd: cost_usd("0.5"),
                source: CostSource::Native,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn logs_ingest_returns_tool_call_rollups_by_tool_and_repo() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = ToolCallRollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );

    assert_eq!(
        query_tool_call_rollup(rpc_socket_path.clone(), query.clone()).await?,
        vec![
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: RepoBucket::no_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Read"),
                call_count: 1,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Bash"),
                call_count: 1,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Read"),
                call_count: 2,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: other_kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Edit"),
                call_count: 1,
            },
        ]
    );

    assert_eq!(
        query_tool_call_rollup(rpc_socket_path, query.with_repo(kvasir_repo())).await?,
        vec![
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Bash"),
                call_count: 1,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Read"),
                call_count: 2,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn protobuf_logs_ingest_returns_tool_call_rollups() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(claude_tool_result_logs_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_tool_call_rollup(
            rpc_socket_path,
            ToolCallRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
            .with_repo(kvasir_repo())
        )
        .await?,
        vec![ToolCallRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            harness: claude_code_harness(),
            tool_name: ToolName::new("Read"),
            call_count: 1,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn logs_ingest_accepts_batches_without_tool_results_as_noop() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_non_tool_logs_fixture())
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_tool_call_rollup(
            rpc_socket_path,
            ToolCallRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
        )
        .await?,
        Vec::new()
    );

    Ok(())
}

#[tokio::test]
async fn logs_ingest_deduplicates_replayed_tool_result_events() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;
    let client = reqwest::Client::new();
    let endpoint = format!("http://{}/v1/logs", daemon.otlp_addr());

    for _ in 0..2 {
        client
            .post(&endpoint)
            .header(AUTHORIZATION, "Bearer test-token")
            .header(CONTENT_TYPE, "application/json")
            .body(claude_tool_result_logs_fixture())
            .send()
            .await?
            .error_for_status()?;
    }
    client
        .post(&endpoint)
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture_with_trace_variation())
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_tool_call_rollup(
            rpc_socket_path,
            ToolCallRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
            .with_repo(kvasir_repo())
        )
        .await?,
        vec![
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Bash"),
                call_count: 1,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: claude_code_harness(),
                tool_name: ToolName::new("Read"),
                call_count: 2,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn daemon_refuses_to_replace_non_socket_rpc_path() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("not-a-socket");
    std::fs::write(&rpc_socket_path, "do not remove")?;

    let result = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await;

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(rpc_socket_path)?, "do not remove");

    Ok(())
}

#[tokio::test]
async fn daemon_creates_private_rpc_socket() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let mode = std::fs::metadata(rpc_socket_path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_returns_mixed_cost_rollups_with_time_boundaries() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(mixed_cost_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_cost_rollup(
            rpc_socket_path.clone(),
            CostRollupQuery::new(
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap()
                ),
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 13, 0, 0).unwrap()
                ),
            )
        )
        .await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("claude-sonnet-4-20250514"),
            cost_usd: cost_usd("0.2"),
            source: CostSource::Native,
        }]
    );

    assert_eq!(
        query_cost_rollup(
            rpc_socket_path.clone(),
            CostRollupQuery::new(
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 13, 0, 0).unwrap()
                ),
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 14, 0, 0).unwrap()
                ),
            )
        )
        .await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("claude-sonnet-4-20250514"),
            cost_usd: CostUsd::from_nanos(3_000_000).unwrap(),
            source: CostSource::Estimated,
        }]
    );

    assert_eq!(
        query_cost_rollup(
            rpc_socket_path,
            CostRollupQuery::new(
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap()
                ),
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 14, 0, 0).unwrap()
                ),
            )
        )
        .await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("claude-sonnet-4-20250514"),
            cost_usd: cost_usd("0.203"),
            source: CostSource::Mixed,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_uses_configured_price_table_for_estimated_cost() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let price_table = PriceTable::from_prices(vec![ModelTokenPrices::new(
        ModelName::new("local-test-model"),
        CostUsd::from_nanos(10).unwrap(),
        CostUsd::from_nanos(20).unwrap(),
        CostUsd::from_nanos(5).unwrap(),
    )]);
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
        price_table,
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(custom_price_token_usage_fixture())
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_cost_rollup(
            rpc_socket_path,
            CostRollupQuery::new(
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 12, 0, 0).unwrap()
                ),
                TimestampMillis::from_datetime(
                    Utc.with_ymd_and_hms(2026, 6, 20, 13, 0, 0).unwrap()
                ),
            )
        )
        .await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("local-test-model"),
            cost_usd: CostUsd::from_nanos(1_000).unwrap(),
            source: CostSource::Estimated,
        }]
    );

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_rejects_oversized_bodies() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let mut stream = tokio::net::TcpStream::connect(daemon.otlp_addr()).await?;
    tokio::io::AsyncWriteExt::write_all(
        &mut stream,
        b"POST /v1/metrics HTTP/1.1\r\n\
          Host: localhost\r\n\
          Authorization: Bearer test-token\r\n\
          Content-Type: application/json\r\n\
          Content-Length: 9437184\r\n\
          \r\n",
    )
    .await?;
    let mut response = vec![0_u8; 256];
    let bytes_read = tokio::io::AsyncReadExt::read(&mut stream, &mut response).await?;
    let response = String::from_utf8(response[..bytes_read].to_vec())?;

    assert!(response.starts_with("HTTP/1.1 413 Payload Too Large"));

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_rejects_payloads_without_token_usage_metrics() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"resourceMetrics":[]}"#)
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    Ok(())
}

#[tokio::test]
async fn metrics_ingest_rejects_mixed_batches_with_empty_token_usage_metrics() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(
            r#"{
                "resourceMetrics": [{
                    "scopeMetrics": [{
                        "metrics": [
                            {
                                "name": "token.usage",
                                "sum": {
                                    "dataPoints": [{
                                        "timeUnixNano": "1781956800000000000",
                                        "asInt": "100",
                                        "attributes": [
                                            { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                            { "key": "token.type", "value": { "stringValue": "input" } }
                                        ]
                                    }]
                                }
                            },
                            {
                                "name": "token.usage",
                                "sum": { "dataPoints": [] }
                            }
                        ]
                    }]
                }]
            }"#,
        )
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    Ok(())
}

#[tokio::test]
async fn rpc_client_rejects_oversized_responses() -> anyhow::Result<()> {
    const OVERSIZED_RPC_RESPONSE_BYTES: usize = 1024 * 1024 + 1;

    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("oversized-response.sock");
    let listener = tokio::net::UnixListener::bind(&rpc_socket_path)?;
    let server = tokio::spawn(async move {
        let (mut stream, _addr) = listener.accept().await?;
        tokio::io::AsyncWriteExt::write_all(&mut stream, &vec![b'a'; OVERSIZED_RPC_RESPONSE_BYTES])
            .await?;
        tokio::io::AsyncWriteExt::write_all(&mut stream, b"\n").await?;
        anyhow::Ok(())
    });

    let result = query_token_rollup(
        rpc_socket_path,
        RollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
        ),
    )
    .await;

    assert!(result.is_err());
    server.await??;

    Ok(())
}

#[tokio::test]
async fn rpc_subscription_closes_when_extra_input_arrives_after_subscribe_request()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let _daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let mut stream = tokio::net::UnixStream::connect(rpc_socket_path).await?;
    let request = RpcRequest::SubscribeTokenRollup {
        query: RollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
        ),
    };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    request_bytes.extend_from_slice(b"unexpected-client-input");
    stream.write_all(&request_bytes).await?;

    let mut reader = BufReader::new(stream);
    let mut response = Vec::new();
    reader.read_until(b'\n', &mut response).await?;
    assert_eq!(
        serde_json::from_slice::<RpcStreamEvent>(&response)?,
        RpcStreamEvent::TokenRollup {
            rollups: Vec::new()
        }
    );

    let mut eof = [0_u8; 1];
    let bytes_read = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut eof)).await??;
    assert_eq!(bytes_read, 0);

    Ok(())
}

#[tokio::test]
async fn daemon_returns_bounded_error_for_oversized_rpc_query_response() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(many_model_token_usage_fixture(7_000))
        .send()
        .await?
        .error_for_status()?;

    let err = query_token_rollup(
        rpc_socket_path,
        RollupQuery::new(
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
            TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
        ),
    )
    .await
    .expect_err("oversized query response should return a bounded rpc error");

    assert!(err.to_string().contains("ResponseTooLarge"));

    Ok(())
}

fn claude_token_usage_protobuf_fixture() -> anyhow::Result<Vec<u8>> {
    let encoded = include_str!("fixtures/claude_token_usage_otlp.pb.base64").trim();
    Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
}

async fn start_test_daemon(config: DaemonConfig) -> anyhow::Result<RunningDaemon> {
    start_with_store_key_source(config, StoreKeySource::static_key_for_test([11; 32])).await
}

fn kvasir_repo() -> RepoBucket {
    RepoBucket::repo(RepoIdentity::new(
        RepoName::new("kvasir"),
        RepoPath::new("/Users/oyr/projects/kvasir"),
    ))
}

fn other_kvasir_repo() -> RepoBucket {
    RepoBucket::repo(RepoIdentity::new(
        RepoName::new("kvasir"),
        RepoPath::new("/tmp/other-kvasir"),
    ))
}

fn claude_code_harness() -> HarnessName {
    HarnessName::new("claude_code")
}

fn cost_usd(value: &str) -> CostUsd {
    CostUsd::from_decimal_str(value).expect("test cost must be valid")
}

fn repo_and_no_repo_metrics_fixture() -> &'static str {
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
            },
            {
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "25",
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

fn native_cost_usage_fixture() -> &'static str {
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
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 0.375,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                ]
                            }]
                        }
                    }]
                }]
            }
        ]
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
                                { "key": "tool_name", "value": { "stringValue": "Read" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781957000000000000",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Bash" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781957100000000000",
                            "eventName": "user_prompt",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Ignored" } }
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
            },
            {
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "Read" } }
                        ]
                    }]
                }]
            }
        ]
    }"#
}

fn claude_tool_result_logs_fixture_with_trace_variation() -> &'static str {
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
                            "traceId": "11111111111111111111111111111111",
                            "spanId": "2222222222222222",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Read" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781956900000000000",
                            "traceId": "33333333333333333333333333333333",
                            "spanId": "4444444444444444",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool_name", "value": { "stringValue": "Read" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781957000000000000",
                            "traceId": "55555555555555555555555555555555",
                            "spanId": "6666666666666666",
                            "eventName": "tool_result",
                            "attributes": [
                                { "key": "tool.name", "value": { "stringValue": "Bash" } }
                            ]
                        }
                    ]
                }]
            }
        ]
    }"#
}

fn claude_tool_result_logs_protobuf_fixture() -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![OtlpResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("repo.name", "kvasir"),
                    string_attribute("repo.path", "/Users/oyr/projects/kvasir"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![
                    LogRecord {
                        time_unix_nano: 1_781_956_800_000_000_000,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: None,
                        attributes: vec![string_attribute("tool.name", "Read")],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: "tool_result".to_owned(),
                    },
                    LogRecord {
                        time_unix_nano: 1_781_956_900_000_000_000,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: None,
                        attributes: vec![string_attribute("tool.name", "Ignored")],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: "user_prompt".to_owned(),
                    },
                ],
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

fn claude_non_tool_logs_fixture() -> &'static str {
    r#"{
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                ]
            },
            "scopeLogs": [{
                "logRecords": [{
                    "timeUnixNano": "1781956800000000000",
                    "eventName": "user_prompt",
                    "attributes": [
                        { "key": "tool.name", "value": { "stringValue": "Read" } }
                    ]
                }]
            }]
        }]
    }"#
}

fn many_model_token_usage_fixture(model_count: usize) -> String {
    let mut data_points = String::new();
    for index in 0..model_count {
        if index > 0 {
            data_points.push(',');
        }
        data_points.push_str(&format!(
            r#"{{
                "startTimeUnixNano": "1781956700000000000",
                "timeUnixNano": "1781956800000000000",
                "asInt": "1",
                "attributes": [
                    {{ "key": "model", "value": {{ "stringValue": "model-{index:04}" }} }},
                    {{ "key": "token.type", "value": {{ "stringValue": "input" }} }}
                ]
            }}"#
        ));
    }

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

fn mixed_cost_usage_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                ]
            },
            "scopeMetrics": [{
                "metrics": [
                    {
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781956800000000000",
                                    "asInt": "1000",
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": "claude-sonnet-4-20250514" } },
                                        { "key": "token.type", "value": { "stringValue": "input" } }
                                    ]
                                },
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781960400000000000",
                                    "asInt": "200",
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": "claude-sonnet-4-20250514" } },
                                        { "key": "token.type", "value": { "stringValue": "output" } }
                                    ]
                                }
                            ]
                        }
                    },
                    {
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 0.2,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-sonnet-4-20250514" } }
                                ]
                            }]
                        }
                    }
                ]
            }]
        }]
    }"#
}

fn custom_price_token_usage_fixture() -> &'static str {
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
                            "timeUnixNano": "1781956800000000000",
                            "asInt": "100",
                            "attributes": [
                                { "key": "model", "value": { "stringValue": "local-test-model" } },
                                { "key": "token.type", "value": { "stringValue": "input" } }
                            ]
                        }]
                    }
                }]
            }]
        }]
    }"#
}
