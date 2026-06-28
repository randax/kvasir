use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex as StdMutex};
use std::time::Duration;

use base64::Engine;
use chrono::{TimeZone, Utc};
use http::StatusCode;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};
use kvasir_core::rpc::{
    BearerToken, ContentAvailability, ContentKindAvailability, ContentQuery, ContentReplay,
    ContentReplayItem, ContentUnavailableReason, CostRollup, CostRollupQuery, CostSource,
    HarnessName, ModelName, RollupDay, RollupQuery, RpcRequest, RpcStreamEvent, TimestampMillis,
    TokenRollup, ToolCallRollup, ToolCallRollupQuery, ToolName, TraceSpanKind,
};
use kvasir_core::{
    ContentKind, ContentText, CostUsd, ModelTokenPrices, PriceTable, RepoBucket, RepoIdentity,
    RepoName, RepoPath, StoreKey, UsageStore,
};
use kvasird::{
    DaemonConfig, RunningDaemon, StoreKeySource, query_content, query_cost_rollup,
    query_token_rollup, query_tool_call_rollup, query_trace, start, start_with_store_key_source,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{
    LogRecord, ResourceLogs as OtlpResourceLogs, ScopeLogs,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use zeroize::Zeroizing;

const TEST_STORE_KEY_BYTES: [u8; 32] = [11; 32];
static TEST_KEYRING_DEFAULT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
                repo: copilot_kvasir_repo(),
                model: ModelName::new("gpt-4.1"),
                input_tokens: 1200,
                output_tokens: 450,
                cache_tokens: 0,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: copilot_kvasir_repo(),
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
                repo: copilot_kvasir_repo(),
                model: ModelName::new("gpt-4.1"),
                cost_usd: CostUsd::from_nanos(6_000_000).unwrap(),
                source: CostSource::Estimated,
            },
            CostRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: copilot_kvasir_repo(),
                model: ModelName::new("gpt-5.4"),
                cost_usd: CostUsd::from_nanos(550_000).unwrap(),
                source: CostSource::Estimated,
            }
        ]
    );

    Ok(())
}

#[tokio::test]
async fn golden_codex_trace_replay_returns_canonical_span_tree() -> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(codex_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = query_trace(
        rpc_socket_path,
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("codex"),
            session_id: kvasir_core::rpc::SessionId::new("codex-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("codex-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(150));
    assert_eq!(traces[0].durations.request_ms, Some(1600));
    assert_eq!(traces[0].durations.tool_ms, Some(300));
    assert_eq!(traces[0].spans[0].kind, TraceSpanKind::Interaction);
    assert_eq!(traces[0].spans[1].kind, TraceSpanKind::LlmRequest);
    assert_eq!(traces[0].spans[2].kind, TraceSpanKind::ToolCall);
    assert_eq!(traces[0].spans[2].tool_name, Some(ToolName::new("Read")));

    Ok(())
}

#[tokio::test]
async fn golden_copilot_trace_replay_returns_canonical_span_tree() -> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(copilot_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = query_trace(
        rpc_socket_path,
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("github_copilot"),
            session_id: kvasir_core::rpc::SessionId::new("copilot-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("copilot-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(200));
    assert_eq!(traces[0].durations.request_ms, Some(1400));
    assert_eq!(traces[0].durations.tool_ms, Some(400));
    assert_eq!(traces[0].spans[0].kind, TraceSpanKind::Interaction);
    assert_eq!(traces[0].spans[1].kind, TraceSpanKind::LlmRequest);
    assert_eq!(traces[0].spans[2].kind, TraceSpanKind::ToolCall);
    assert_eq!(traces[0].spans[2].tool_name, Some(ToolName::new("Read")));

    Ok(())
}

#[tokio::test]
async fn protobuf_codex_trace_replay_returns_canonical_span_tree() -> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(codex_trace_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = query_trace(
        rpc_socket_path,
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("codex"),
            session_id: kvasir_core::rpc::SessionId::new("codex-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("codex-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(150));
    assert_eq!(traces[0].durations.request_ms, Some(1600));
    assert_eq!(traces[0].durations.tool_ms, Some(300));
    assert_eq!(traces[0].spans[0].kind, TraceSpanKind::Interaction);
    assert_eq!(traces[0].spans[1].kind, TraceSpanKind::LlmRequest);
    assert_eq!(traces[0].spans[2].kind, TraceSpanKind::ToolCall);
    assert_eq!(traces[0].spans[2].tool_name, Some(ToolName::new("Read")));

    Ok(())
}

#[tokio::test]
async fn protobuf_copilot_trace_replay_returns_canonical_span_tree() -> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(copilot_trace_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    let traces = query_trace(
        rpc_socket_path,
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("github_copilot"),
            session_id: kvasir_core::rpc::SessionId::new("copilot-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("copilot-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(200));
    assert_eq!(traces[0].durations.request_ms, Some(1400));
    assert_eq!(traces[0].durations.tool_ms, Some(400));
    assert_eq!(traces[0].spans[0].kind, TraceSpanKind::Interaction);
    assert_eq!(traces[0].spans[1].kind, TraceSpanKind::LlmRequest);
    assert_eq!(traces[0].spans[2].kind, TraceSpanKind::ToolCall);
    assert_eq!(traces[0].spans[2].tool_name, Some(ToolName::new("Read")));

    Ok(())
}

#[tokio::test]
async fn golden_opencode_trace_log_replay_returns_trace_primary_rollups() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: database_path.clone(),
        bearer_token,
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;
    let client = reqwest::Client::new();

    client
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(opencode_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let token_query = RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), token_query).await?,
        vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("gpt-4.1"),
            input_tokens: 1200,
            output_tokens: 450,
            cache_tokens: 80,
        }]
    );

    let cost_query = CostRollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_cost_rollup(rpc_socket_path.clone(), cost_query).await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("gpt-4.1"),
            cost_usd: CostUsd::from_nanos(6_040_000).unwrap(),
            source: CostSource::Estimated,
        }]
    );

    let tool_query = ToolCallRollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_tool_call_rollup(rpc_socket_path.clone(), tool_query).await?,
        vec![ToolCallRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            harness: HarnessName::new("opencode"),
            tool_name: ToolName::new("Read"),
            call_count: 1,
        }]
    );

    let traces = query_trace(
        rpc_socket_path.clone(),
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("opencode"),
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(120));
    assert_eq!(traces[0].durations.request_ms, Some(1800));
    assert_eq!(traces[0].durations.tool_ms, Some(250));

    let unauthorized = query_content(
        rpc_socket_path.clone(),
        ContentQuery {
            harness: HarnessName::new("opencode"),
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
        },
        BearerToken::new("wrong-token"),
    )
    .await
    .expect_err("content replay should require the daemon bearer token");
    assert!(unauthorized.to_string().contains("Unauthorized"));

    assert_eq!(
        query_content(
            rpc_socket_path,
            ContentQuery {
                harness: HarnessName::new("opencode"),
                session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
                prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
            },
            BearerToken::new("test-token"),
        )
        .await?,
        ContentReplay {
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
            items: vec![
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_801_920),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::UserPrompt,
                    content: ContentText::new("summarize README.md").unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_801_920),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::AssistantMessage,
                    content: ContentText::new("I need to read it first.").unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_170),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::ToolInput,
                    content: ContentText::new(r#"{"path":"README.md"}"#).unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_170),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::ToolOutput,
                    content: ContentText::new("kvasir is a local telemetry daemon").unwrap(),
                },
            ],
            availability: ContentAvailability::Captured {
                harness: HarnessName::new("opencode"),
                kinds: vec![
                    ContentKindAvailability::Captured {
                        kind: ContentKind::UserPrompt,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::AssistantMessage,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::ToolInput,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::ToolOutput,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::RawApiRequest,
                        reason: ContentUnavailableReason::NotProvidedByHarness,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::RawApiResponse,
                        reason: ContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    drop(daemon);
    assert_eq!(
        persisted_opencode_content_rows(&database_path)?,
        vec![
            (
                "opencode".to_owned(),
                "user_prompt".to_owned(),
                "summarize README.md".to_owned(),
            ),
            (
                "opencode".to_owned(),
                "assistant_message".to_owned(),
                "I need to read it first.".to_owned(),
            ),
            (
                "opencode".to_owned(),
                "tool_input".to_owned(),
                r#"{"path":"README.md"}"#.to_owned(),
            ),
            (
                "opencode".to_owned(),
                "tool_output".to_owned(),
                "kvasir is a local telemetry daemon".to_owned(),
            ),
        ]
    );

    Ok(())
}

#[tokio::test]
async fn claude_raw_body_file_imports_into_content_replay_and_removes_source() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let request_body_path = raw_body_dir.join("request-1.json");
    std::fs::write(
        &request_body_path,
        r#"{"messages":[{"role":"user","content":"show me the full context"}]}"#,
    )?;
    let response_body_path = raw_body_dir.join("response-1.json");
    std::fs::write(
        &response_body_path,
        r#"{"content":[{"type":"text","text":"full model response"}]}"#,
    )?;

    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_raw_api_body_ref_logs_fixture(
            "request-1.json",
            "response-1.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    assert!(
        !request_body_path.exists(),
        "imported raw request body source file should be removed"
    );
    assert!(
        !response_body_path.exists(),
        "imported raw response body source file should be removed"
    );
    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_raw_api_body_ref_logs_fixture(
            "request-1.json",
            "response-1.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    assert_eq!(
        query_content(
            rpc_socket_path,
            ContentQuery {
                harness: HarnessName::new("claude_code"),
                session_id: kvasir_core::rpc::SessionId::new("claude-session-1"),
                prompt_id: kvasir_core::rpc::PromptId::new("claude-turn-1"),
            },
            BearerToken::new("test-token"),
        )
        .await?,
        ContentReplay {
            session_id: kvasir_core::rpc::SessionId::new("claude-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("claude-turn-1"),
            items: vec![
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_180),
                    harness: HarnessName::new("claude_code"),
                    kind: ContentKind::RawApiRequest,
                    content: ContentText::new(
                        r#"{"messages":[{"role":"user","content":"show me the full context"}]}"#,
                    )
                    .unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_220),
                    harness: HarnessName::new("claude_code"),
                    kind: ContentKind::RawApiResponse,
                    content: ContentText::new(
                        r#"{"content":[{"type":"text","text":"full model response"}]}"#,
                    )
                    .unwrap(),
                }
            ],
            availability: ContentAvailability::Captured {
                harness: HarnessName::new("claude_code"),
                kinds: vec![
                    ContentKindAvailability::Captured {
                        kind: ContentKind::RawApiRequest,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::RawApiResponse,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::UserPrompt,
                        reason: ContentUnavailableReason::NotCapturedForPrompt,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::AssistantMessage,
                        reason: ContentUnavailableReason::NotCapturedForPrompt,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::ToolInput,
                        reason: ContentUnavailableReason::NotCapturedForPrompt,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::ToolOutput,
                        reason: ContentUnavailableReason::NotCapturedForPrompt,
                    },
                ],
            },
        }
    );

    drop(daemon);
    let raw_body_rows = persisted_raw_body_rows(&database_path)?;
    assert_eq!(raw_body_rows.len(), 2);
    assert_eq!(raw_body_rows[0].harness, "claude_code");
    assert_eq!(raw_body_rows[0].content_kind, "raw_api_request");
    assert_eq!(raw_body_rows[0].compression, "zstd");
    assert_eq!(raw_body_rows[1].harness, "claude_code");
    assert_eq!(raw_body_rows[1].content_kind, "raw_api_response");
    assert_eq!(raw_body_rows[1].compression, "zstd");
    assert_ne!(
        raw_body_rows[0].compressed_body,
        r#"{"messages":[{"role":"user","content":"show me the full context"}]}"#.as_bytes(),
        "raw body should be compressed before storage"
    );
    assert_eq!(persisted_opencode_content_rows(&database_path)?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn protobuf_claude_raw_body_file_imports_into_content_replay() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let request_body_path = raw_body_dir.join("request-protobuf.json");
    std::fs::write(
        &request_body_path,
        r#"{"messages":[{"role":"user","content":"protobuf full context"}]}"#,
    )?;
    let response_body_path = raw_body_dir.join("response-protobuf.json");
    std::fs::write(
        &response_body_path,
        r#"{"content":[{"type":"text","text":"protobuf model response"}]}"#,
    )?;

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
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(claude_raw_api_body_ref_logs_protobuf_fixture(
            "request-protobuf.json",
            "response-protobuf.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    assert!(
        !request_body_path.exists(),
        "protobuf raw request body source file should be removed"
    );
    assert!(
        !response_body_path.exists(),
        "protobuf raw response body source file should be removed"
    );
    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(claude_raw_api_body_ref_logs_protobuf_fixture(
            "request-protobuf.json",
            "response-protobuf.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    let replay = query_content(
        rpc_socket_path,
        ContentQuery {
            harness: HarnessName::new("claude_code"),
            session_id: kvasir_core::rpc::SessionId::new("claude-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("claude-turn-1"),
        },
        BearerToken::new("test-token"),
    )
    .await?;

    assert_eq!(replay.items.len(), 2);
    assert_eq!(replay.items[0].kind, ContentKind::RawApiRequest);
    assert_eq!(
        replay.items[0].content.as_str(),
        r#"{"messages":[{"role":"user","content":"protobuf full context"}]}"#
    );
    assert_eq!(replay.items[1].kind, ContentKind::RawApiResponse);
    assert_eq!(
        replay.items[1].content.as_str(),
        r#"{"content":[{"type":"text","text":"protobuf model response"}]}"#
    );

    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn claude_raw_body_files_arriving_after_otlp_are_imported_by_one_shot_scan()
-> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let request_body_path = raw_body_dir.join("late-request.json");
    let response_body_path = raw_body_dir.join("late-response.json");

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
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_raw_api_body_ref_logs_fixture(
            "late-request.json",
            "late-response.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    std::fs::write(
        &request_body_path,
        r#"{"messages":[{"role":"user","content":"arrived after log"}]}"#,
    )?;
    std::fs::write(
        &response_body_path,
        r#"{"content":[{"type":"text","text":"arrived after log response"}]}"#,
    )?;

    daemon.import_available_raw_bodies_once().await?;
    let replay = query_content(
        rpc_socket_path.clone(),
        ContentQuery {
            harness: HarnessName::new("claude_code"),
            session_id: kvasir_core::rpc::SessionId::new("claude-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("claude-turn-1"),
        },
        BearerToken::new("test-token"),
    )
    .await?;
    assert_eq!(
        replay.items[0].content.as_str(),
        r#"{"messages":[{"role":"user","content":"arrived after log"}]}"#
    );
    assert_eq!(
        replay.items[1].content.as_str(),
        r#"{"content":[{"type":"text","text":"arrived after log response"}]}"#
    );
    assert!(!request_body_path.exists());
    assert!(!response_body_path.exists());

    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn persisted_raw_body_row_retries_plaintext_cleanup() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let request_body_path = raw_body_dir.join("orphan-request.json");

    let database_path = temp.path().join("usage.sqlite3");
    std::fs::write(&request_body_path, r#"{"plaintext":"left behind"}"#)?;
    initialize_test_store(&database_path)?;
    let event_key =
        insert_raw_body_import_queue_row(&database_path, "orphan-request.json", 1_781_956_802_180)?;
    insert_persisted_raw_body_row_for_event(&database_path, &event_key, "zstd")?;

    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    daemon.import_available_raw_bodies_once().await?;

    assert!(!request_body_path.exists());
    assert!(
        !raw_body_import_queue_body_refs(&database_path)?
            .contains(&"orphan-request.json".to_owned())
    );

    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn duplicate_persisted_raw_body_event_does_not_delete_reused_body_ref() -> anyhow::Result<()>
{
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let database_path = temp.path().join("usage.sqlite3");
    let reused_path = raw_body_dir.join("reused-request.json");
    initialize_test_store(&database_path)?;
    let event_key =
        insert_raw_body_import_queue_row(&database_path, "reused-request.json", 1_781_956_802_180)?;
    insert_persisted_raw_body_row_for_event(&database_path, &event_key, "zstd")?;
    delete_raw_body_import_queue_event(&database_path, &event_key)?;
    std::fs::write(&reused_path, r#"{"plaintext":"belongs to a newer event"}"#)?;

    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(raw_api_body_ref_logs_fixture_with_nanos(
            "reused-request.json",
            1_781_956_802_180_000_000,
        ))
        .send()
        .await?
        .error_for_status()?;
    daemon.import_available_raw_bodies_once().await?;

    assert_eq!(
        std::fs::read_to_string(reused_path)?,
        r#"{"plaintext":"belongs to a newer event"}"#
    );
    assert!(
        !raw_body_import_queue_body_refs(&database_path)?
            .contains(&"reused-request.json".to_owned())
    );

    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn persisted_raw_body_row_without_source_drains_import_queue() -> anyhow::Result<()> {
    let temp = tempdir()?;
    std::fs::create_dir_all(temp.path().join("raw-bodies"))?;
    let database_path = temp.path().join("usage.sqlite3");
    initialize_test_store(&database_path)?;
    let event_key = insert_raw_body_import_queue_row(
        &database_path,
        "already-cleaned.json",
        1_781_956_802_180,
    )?;
    insert_persisted_raw_body_row_for_event(&database_path, &event_key, "zstd")?;

    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    daemon.import_available_raw_bodies_once().await?;

    assert!(
        !raw_body_import_queue_body_refs(&database_path)?
            .contains(&"already-cleaned.json".to_owned())
    );
    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn unsupported_stored_raw_body_compression_is_repaired_by_reingest() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let database_path = temp.path().join("usage.sqlite3");
    let body_ref = "bad-compression.json";
    let body_path = raw_body_dir.join(body_ref);
    std::fs::write(&body_path, r#"{"plaintext":"recoverable"}"#)?;
    initialize_test_store(&database_path)?;
    let event_key = insert_raw_body_import_queue_row(&database_path, body_ref, 1_781_956_802_180)?;
    insert_persisted_raw_body_row_for_event(&database_path, &event_key, "gzip")?;
    delete_raw_body_import_queue_event(&database_path, &event_key)?;

    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(raw_api_body_ref_logs_fixture_with_nanos(
            body_ref,
            1_781_956_802_180_000_000,
        ))
        .send()
        .await?
        .error_for_status()?;

    assert!(!body_path.exists());
    let raw_body_rows = persisted_raw_body_rows(&database_path)?;
    assert_eq!(raw_body_rows.len(), 1);
    assert_eq!(raw_body_rows[0].compression, "zstd");
    assert!(!raw_body_import_queue_body_refs(&database_path)?.contains(&body_ref.to_owned()));
    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn missing_raw_body_queue_rows_do_not_starve_later_ready_files() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let database_path = temp.path().join("usage.sqlite3");
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    for index in 0..65 {
        insert_raw_body_import_queue_row(
            &database_path,
            &format!("missing-{index}.json"),
            1_781_956_800_000 + index,
        )?;
    }
    let ready_ref = "ready-after-missing.json";
    std::fs::write(
        raw_body_dir.join(ready_ref),
        r#"{"messages":[{"role":"user","content":"not starved"}]}"#,
    )?;
    insert_raw_body_import_queue_row(&database_path, ready_ref, 1_781_956_900_000)?;

    daemon.import_available_raw_bodies_once().await?;
    daemon.import_available_raw_bodies_once().await?;

    let replay = query_content(
        rpc_socket_path,
        ContentQuery {
            harness: HarnessName::new("claude_code"),
            session_id: kvasir_core::rpc::SessionId::new("claude-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("claude-turn-1"),
        },
        BearerToken::new("test-token"),
    )
    .await?;
    assert_eq!(replay.items.len(), 1);
    assert_eq!(
        replay.items[0].content.as_str(),
        r#"{"messages":[{"role":"user","content":"not starved"}]}"#
    );

    drop(daemon);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn claude_raw_body_file_import_rejects_symlink_sources() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let secret_path = temp.path().join("outside-secret.json");
    std::fs::write(&secret_path, r#"{"secret":"do not import"}"#)?;
    std::os::unix::fs::symlink(&secret_path, raw_body_dir.join("request-1.json"))?;

    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_raw_api_body_ref_logs_fixture(
            "request-1.json",
            "missing-response.json",
        ))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(secret_path.exists());
    assert!(raw_body_dir.join("request-1.json").exists());

    drop(daemon);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn claude_raw_body_file_import_rejects_hardlinked_sources() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let secret_path = temp.path().join("outside-secret.json");
    std::fs::write(&secret_path, r#"{"secret":"do not import"}"#)?;
    std::fs::hard_link(&secret_path, raw_body_dir.join("request-1.json"))?;

    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_raw_api_body_ref_logs_fixture(
            "request-1.json",
            "missing-response.json",
        ))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        std::fs::read_to_string(secret_path)?,
        r#"{"secret":"do not import"}"#
    );

    drop(daemon);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_rejects_symlinked_raw_body_directory() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let real_raw_body_dir = temp.path().join("real-raw-bodies");
    std::fs::create_dir_all(&real_raw_body_dir)?;
    std::os::unix::fs::symlink(&real_raw_body_dir, temp.path().join("raw-bodies"))?;

    let result = start_test_daemon(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: temp.path().join("kvasird.sock"),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await;

    let error = match result {
        Ok(_daemon) => anyhow::bail!("daemon should reject symlinked raw body directory"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("raw body directory must be a real directory")
    );
    Ok(())
}

#[tokio::test]
async fn raw_body_file_import_ignores_non_claude_code_harnesses() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let raw_body_dir = temp.path().join("raw-bodies");
    std::fs::create_dir_all(&raw_body_dir)?;
    let request_body_path = raw_body_dir.join("codex-request.json");
    std::fs::write(&request_body_path, r#"{"input":"should stay untouched"}"#)?;

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
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(raw_api_body_ref_logs_fixture_for_harness(
            "codex",
            "codex-session-1",
            "codex-turn-1",
            "codex-request.json",
        ))
        .send()
        .await?
        .error_for_status()?;

    assert!(request_body_path.exists());
    let replay = query_content(
        rpc_socket_path,
        ContentQuery {
            harness: HarnessName::new("codex"),
            session_id: kvasir_core::rpc::SessionId::new("codex-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("codex-turn-1"),
        },
        BearerToken::new("test-token"),
    )
    .await?;
    assert_eq!(
        replay.availability,
        ContentAvailability::Unavailable {
            reason: ContentUnavailableReason::PromptNotFound,
        }
    );

    drop(daemon);
    Ok(())
}

#[tokio::test]
async fn protobuf_opencode_trace_replay_returns_trace_primary_rollups() -> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/x-protobuf")
        .body(opencode_trace_protobuf_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), query).await?,
        vec![TokenRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("gpt-4.1"),
            input_tokens: 1200,
            output_tokens: 450,
            cache_tokens: 80,
        }]
    );

    assert_eq!(
        query_tool_call_rollup(
            rpc_socket_path.clone(),
            ToolCallRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
        )
        .await?,
        vec![ToolCallRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            harness: HarnessName::new("opencode"),
            tool_name: ToolName::new("Read"),
            call_count: 1,
        }]
    );

    assert_eq!(
        query_cost_rollup(
            rpc_socket_path.clone(),
            CostRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
        )
        .await?,
        vec![CostRollup {
            day: RollupDay::parse("2026-06-20")?,
            repo: kvasir_repo(),
            model: ModelName::new("gpt-4.1"),
            cost_usd: CostUsd::from_nanos(6_040_000).unwrap(),
            source: CostSource::Estimated,
        }]
    );

    let traces = query_trace(
        rpc_socket_path.clone(),
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("opencode"),
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].durations.ttft_ms, Some(120));
    assert_eq!(traces[0].durations.request_ms, Some(1800));
    assert_eq!(traces[0].durations.tool_ms, Some(250));

    assert_eq!(
        query_content(
            rpc_socket_path,
            ContentQuery {
                harness: HarnessName::new("opencode"),
                session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
                prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
            },
            BearerToken::new("test-token"),
        )
        .await?,
        ContentReplay {
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-1"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-1"),
            items: vec![
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_801_920),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::UserPrompt,
                    content: ContentText::new("summarize README.md").unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_801_920),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::AssistantMessage,
                    content: ContentText::new("I need to read it first.").unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_170),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::ToolInput,
                    content: ContentText::new(r#"{"path":"README.md"}"#).unwrap(),
                },
                ContentReplayItem {
                    occurred_at: TimestampMillis::from_millis(1_781_956_802_170),
                    harness: HarnessName::new("opencode"),
                    kind: ContentKind::ToolOutput,
                    content: ContentText::new("kvasir is a local telemetry daemon").unwrap(),
                },
            ],
            availability: ContentAvailability::Captured {
                harness: HarnessName::new("opencode"),
                kinds: vec![
                    ContentKindAvailability::Captured {
                        kind: ContentKind::UserPrompt,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::AssistantMessage,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::ToolInput,
                    },
                    ContentKindAvailability::Captured {
                        kind: ContentKind::ToolOutput,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::RawApiRequest,
                        reason: ContentUnavailableReason::NotProvidedByHarness,
                    },
                    ContentKindAvailability::Unavailable {
                        kind: ContentKind::RawApiResponse,
                        reason: ContentUnavailableReason::NotProvidedByHarness,
                    },
                ],
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn opencode_trace_ingest_degrades_when_experimental_attributes_are_missing()
-> anyhow::Result<()> {
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
        .post(format!("http://{}/v1/traces", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(opencode_degraded_trace_fixture())
        .send()
        .await?
        .error_for_status()?;

    let query = RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    );
    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), query).await?,
        Vec::new()
    );
    assert_eq!(
        query_tool_call_rollup(
            rpc_socket_path.clone(),
            ToolCallRollupQuery::new(
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
                TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            )
        )
        .await?,
        Vec::new()
    );

    let traces = query_trace(
        rpc_socket_path,
        kvasir_core::rpc::TraceQuery {
            harness: HarnessName::new("opencode"),
            session_id: kvasir_core::rpc::SessionId::new("opencode-session-2"),
            prompt_id: kvasir_core::rpc::PromptId::new("opencode-turn-2"),
        },
    )
    .await?;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].spans.len(), 3);
    assert_eq!(traces[0].spans[2].tool_name, None);

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
            StoreKeySource::static_key_for_test(TEST_STORE_KEY_BYTES),
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
        StoreKeySource::static_key_for_test(TEST_STORE_KEY_BYTES),
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
async fn daemon_default_start_generates_reuses_and_requires_keychain_key() -> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path,
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    {
        let daemon = start(config.clone()).await?;

        reqwest::Client::new()
            .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
            .header(AUTHORIZATION, "Bearer test-token")
            .header(CONTENT_TYPE, "application/json")
            .body(repo_and_no_repo_metrics_fixture())
            .send()
            .await?
            .error_for_status()?;
    }

    assert_eq!(test_keyring.set_count(), 1);

    let daemon = start(config.clone()).await?;

    assert_eq!(test_keyring.set_count(), 1);
    assert_eq!(test_keyring.get_count(), 2);
    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), repo_and_no_repo_rollup_query()).await?,
        repo_and_no_repo_expected_rollups()?
    );

    drop(daemon);
    let saved_passwords = test_keyring.take_passwords();

    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("missing key must fail startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(
            &error,
            "store key is missing for existing encrypted database"
        ),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);

    test_keyring.replace_passwords(saved_passwords);
    let daemon = start(config.clone()).await?;

    assert_eq!(
        query_token_rollup(rpc_socket_path.clone(), repo_and_no_repo_rollup_query()).await?,
        repo_and_no_repo_expected_rollups()?
    );

    drop(daemon);

    let saved_passwords = test_keyring.take_passwords();
    let mut wrong_key_passwords = saved_passwords.clone();
    let wrong_key = StoreKey::from_bytes([12; 32]).to_hex_secret();
    for password in wrong_key_passwords.values_mut() {
        *password = wrong_key.as_bytes().to_vec();
    }
    test_keyring.replace_passwords(wrong_key_passwords);

    let get_count_before_wrong_key = test_keyring.get_count();
    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("wrong key must fail startup");
        }
        Err(error) => error,
    };

    assert_eq!(test_keyring.get_count(), get_count_before_wrong_key + 1);
    assert!(
        !error_chain_contains(
            &error,
            "store key is missing for existing encrypted database"
        ),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);

    test_keyring.replace_passwords(saved_passwords);
    let daemon = start(config).await?;

    assert_eq!(
        query_token_rollup(rpc_socket_path, repo_and_no_repo_rollup_query()).await?,
        repo_and_no_repo_expected_rollups()?
    );

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_generates_key_for_empty_placeholder_database() -> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    std::fs::File::create(&database_path)?;

    let daemon = start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path,
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await?;

    assert_eq!(test_keyring.set_count(), 1);

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_requires_key_for_empty_database_with_sidecar() -> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let wal_path = sqlite_sidecar_test_path(&database_path, "-wal");
    std::fs::File::create(&database_path)?;
    std::fs::write(&wal_path, "preserved wal")?;

    let error = match start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await
    {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("sidecar state without a key must fail startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(
            &error,
            "store key is missing for existing encrypted database"
        ),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 0);
    assert_eq!(std::fs::metadata(&database_path)?.len(), 0);
    assert_eq!(std::fs::read_to_string(&wal_path)?, "preserved wal");

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn daemon_default_start_requires_key_for_symlinked_empty_database_with_sidecar()
-> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let real_database_path = temp.path().join("real.sqlite3");
    let symlink_database_path = temp.path().join("alias.sqlite3");
    let wal_path = sqlite_sidecar_test_path(&real_database_path, "-wal");
    std::fs::File::create(&real_database_path)?;
    std::os::unix::fs::symlink(&real_database_path, &symlink_database_path)?;
    std::fs::write(&wal_path, "preserved canonical wal")?;

    let error = match start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: symlink_database_path,
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await
    {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("canonical sidecar state without a key must fail startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(
            &error,
            "store key is missing for existing encrypted database"
        ),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 0);
    assert_eq!(std::fs::metadata(&real_database_path)?.len(), 0);
    assert_eq!(
        std::fs::read_to_string(&wal_path)?,
        "preserved canonical wal"
    );

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn daemon_default_start_rejects_dangling_database_symlink_before_key_persistence()
-> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    std::os::unix::fs::symlink(temp.path().join("missing.sqlite3"), &database_path)?;

    let error = match start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path,
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    })
    .await
    {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("dangling database symlink must fail startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(&error, "database path is not a regular file"),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn daemon_default_start_opens_stable_database_path_after_alias_retarget() -> anyhow::Result<()>
{
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let keyring_set_pause = test_keyring.pause_next_set();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let first_database_path = temp.path().join("first.sqlite3");
    let second_database_path = temp.path().join("second.sqlite3");
    let symlink_database_path = temp.path().join("alias.sqlite3");
    std::fs::File::create(&first_database_path)?;
    std::os::unix::fs::symlink(&first_database_path, &symlink_database_path)?;
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: symlink_database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    let start_task = tokio::spawn(start(config));
    keyring_set_pause.wait_until_set_paused();
    std::fs::remove_file(&symlink_database_path)?;
    std::os::unix::fs::symlink(&second_database_path, &symlink_database_path)?;
    keyring_set_pause.release_set();

    let daemon = start_task.await??;

    assert_eq!(test_keyring.set_count(), 1);
    assert!(std::fs::metadata(&first_database_path)?.len() > 0);
    assert!(!second_database_path.try_exists()?);

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_cleans_bootstrap_database_when_keychain_write_fails()
-> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    test_keyring.fail_next_set();
    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("keychain write failure must fail first startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(&error, "configured keyring write failure"),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);
    assert!(!database_path.try_exists()?);

    let daemon = start(config).await?;

    assert_eq!(test_keyring.set_count(), 2);
    assert!(database_path.try_exists()?);
    assert!(std::fs::metadata(&database_path)?.len() > 0);

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_restores_empty_placeholder_when_keychain_write_fails()
-> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    std::fs::File::create(&database_path)?;
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    test_keyring.fail_next_set();
    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("keychain write failure must fail placeholder startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(&error, "configured keyring write failure"),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);
    assert!(database_path.try_exists()?);
    assert_eq!(std::fs::metadata(&database_path)?.len(), 0);

    let daemon = start(config).await?;

    assert_eq!(test_keyring.set_count(), 2);
    assert!(std::fs::metadata(&database_path)?.len() > 0);

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_preserves_key_when_store_open_bootstrap_fails() -> anyhow::Result<()>
{
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let mut lock_path = database_path.as_os_str().to_os_string();
    lock_path.push(".startup-lock");
    let lock_path = PathBuf::from(lock_path);
    std::fs::File::create(&database_path)?;
    std::fs::File::create(&lock_path)?;
    std::fs::set_permissions(&database_path, std::fs::Permissions::from_mode(0o600))?;
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))?;
    let directory_permissions_restore =
        DirectoryPermissionsRestore::make_read_only(temp.path().to_path_buf())?;
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("store-open bootstrap failure must fail first startup");
        }
        Err(error) => error,
    };

    assert!(
        !error_chain_contains(
            &error,
            "store key is missing for existing encrypted database"
        ),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);
    assert_eq!(test_keyring.stored_password_count(), 1);
    assert_eq!(std::fs::metadata(&database_path)?.len(), 0);

    drop(directory_permissions_restore);
    let daemon = start(config).await?;

    assert_eq!(test_keyring.set_count(), 1);
    assert!(std::fs::metadata(&database_path)?.len() > 0);

    drop(daemon);

    Ok(())
}

#[tokio::test]
async fn daemon_default_start_preserves_key_when_bootstrap_database_prepare_fails()
-> anyhow::Result<()> {
    let _keyring_default_guard = TEST_KEYRING_DEFAULT_LOCK.lock().await;
    let test_keyring = PersistentTestKeyring::default();
    let _keyring_default_override = test_keyring.install_as_default_keyring();
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let database_path = temp.path().join("usage.sqlite3");
    let mut lock_path = database_path.as_os_str().to_os_string();
    lock_path.push(".startup-lock");
    let lock_path = PathBuf::from(lock_path);
    std::fs::File::create(&lock_path)?;
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))?;
    let directory_permissions_restore =
        DirectoryPermissionsRestore::make_read_only(temp.path().to_path_buf())?;
    let config = DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path,
        database_path: database_path.clone(),
        bearer_token: BearerToken::new("test-token"),
        price_table: PriceTable::bundled_defaults(),
    };

    let error = match start(config.clone()).await {
        Ok(daemon) => {
            drop(daemon);
            anyhow::bail!("database prepare failure must fail first startup");
        }
        Err(error) => error,
    };

    assert!(
        error_chain_contains(&error, "Permission denied"),
        "{error:?}"
    );
    assert_eq!(test_keyring.set_count(), 1);
    assert!(!database_path.try_exists()?);
    assert_eq!(test_keyring.stored_password_count(), 1);

    drop(directory_permissions_restore);
    let daemon = start(config).await?;

    assert_eq!(test_keyring.set_count(), 1);
    assert!(database_path.try_exists()?);

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
async fn metrics_and_logs_ingest_return_tool_call_rollups_for_all_harnesses() -> anyhow::Result<()>
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
    let client = reqwest::Client::new();

    client
        .post(format!("http://{}/v1/logs", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(claude_tool_result_logs_fixture())
        .send()
        .await?
        .error_for_status()?;
    client
        .post(format!("http://{}/v1/metrics", daemon.otlp_addr()))
        .header(AUTHORIZATION, "Bearer test-token")
        .header(CONTENT_TYPE, "application/json")
        .body(tool_call_metrics_fixture())
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
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: HarnessName::new("codex"),
                tool_name: ToolName::new("Read"),
                call_count: 2,
            },
            ToolCallRollup {
                day: RollupDay::parse("2026-06-20")?,
                repo: kvasir_repo(),
                harness: HarnessName::new("github_copilot"),
                tool_name: ToolName::new("Read"),
                call_count: 3,
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
    start_with_store_key_source(
        config,
        StoreKeySource::static_key_for_test(TEST_STORE_KEY_BYTES),
    )
    .await
}

fn kvasir_repo() -> RepoBucket {
    RepoBucket::repo(RepoIdentity::new(
        RepoName::new("kvasir"),
        RepoPath::new("/Users/oyr/projects/kvasir"),
    ))
}

fn copilot_kvasir_repo() -> RepoBucket {
    RepoBucket::repo(RepoIdentity::new(
        RepoName::new("kvasir"),
        RepoPath::new("/repos/kvasir"),
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

fn codex_trace_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "codex" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "codex-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "codex-turn-1" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                        "spanId": "1111111111111111",
                        "name": "codex.session",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802050000000",
                        "attributes": [
                            { "key": "codex.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    },
                    {
                        "traceId": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                        "spanId": "2222222222222222",
                        "parentSpanId": "1111111111111111",
                        "name": "codex.llm_request",
                        "startTimeUnixNano": "1781956800150000000",
                        "endTimeUnixNano": "1781956801750000000",
                        "attributes": [
                            { "key": "codex.span.kind", "value": { "stringValue": "llm_request" } },
                            { "key": "model", "value": { "stringValue": "gpt-5.4" } }
                        ]
                    },
                    {
                        "traceId": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                        "spanId": "3333333333333333",
                        "parentSpanId": "1111111111111111",
                        "name": "codex.tool",
                        "startTimeUnixNano": "1781956801750000000",
                        "endTimeUnixNano": "1781956802050000000",
                        "attributes": [
                            { "key": "codex.span.kind", "value": { "stringValue": "tool" } },
                            { "key": "tool.name", "value": { "stringValue": "Read" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn copilot_trace_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "github-copilot" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } },
                    { "key": "conversation.id", "value": { "stringValue": "copilot-session-1" } },
                    { "key": "prompt.id", "value": { "stringValue": "copilot-turn-1" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "ffffffffffffffffffffffffffffffff",
                        "spanId": "1111111111111111",
                        "name": "github.copilot.chat",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956801800000000",
                        "attributes": [
                            { "key": "gen_ai.operation.name", "value": { "stringValue": "chat" } },
                            { "key": "github.copilot.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    },
                    {
                        "traceId": "ffffffffffffffffffffffffffffffff",
                        "spanId": "2222222222222222",
                        "parentSpanId": "1111111111111111",
                        "name": "gen_ai.chat",
                        "startTimeUnixNano": "1781956800200000000",
                        "endTimeUnixNano": "1781956801600000000",
                        "attributes": [
                            { "key": "github.copilot.span.kind", "value": { "stringValue": "llm_request" } },
                            { "key": "model", "value": { "stringValue": "gpt-4.1" } }
                        ]
                    },
                    {
                        "traceId": "ffffffffffffffffffffffffffffffff",
                        "spanId": "3333333333333333",
                        "parentSpanId": "1111111111111111",
                        "name": "gen_ai.tool.call",
                        "startTimeUnixNano": "1781956801600000000",
                        "endTimeUnixNano": "1781956802000000000",
                        "attributes": [
                            { "key": "github.copilot.span.kind", "value": { "stringValue": "tool_call" } },
                            { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn opencode_trace_fixture() -> &'static str {
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
                        "spanId": "1111111111111111",
                        "name": "opencode.session",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802170000000",
                        "attributes": [
                            { "key": "message.id", "value": { "stringValue": "opencode-turn-1" } },
                            { "key": "opencode.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    },
                    {
                        "traceId": "cccccccccccccccccccccccccccccccc",
                        "spanId": "2222222222222222",
                        "parentSpanId": "1111111111111111",
                        "name": "ai.generateText.doGenerate",
                        "startTimeUnixNano": "1781956800120000000",
                        "endTimeUnixNano": "1781956801920000000",
                        "attributes": [
                            { "key": "message.id", "value": { "stringValue": "opencode-turn-1" } },
                            { "key": "ai.operationId", "value": { "stringValue": "ai.generateText" } },
                            { "key": "ai.model.id", "value": { "stringValue": "gpt-4.1" } },
                            { "key": "ai.usage.promptTokens", "value": { "intValue": "1200" } },
                            { "key": "ai.usage.completionTokens", "value": { "intValue": "450" } },
                            { "key": "ai.usage.cachedInputTokens", "value": { "intValue": "80" } },
                            { "key": "ai.prompt.messages", "value": { "stringValue": "summarize README.md" } },
                            { "key": "ai.response.text", "value": { "stringValue": "I need to read it first." } }
                        ]
                    },
                    {
                        "traceId": "cccccccccccccccccccccccccccccccc",
                        "spanId": "3333333333333333",
                        "parentSpanId": "1111111111111111",
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

fn claude_raw_api_body_ref_logs_fixture(request_body_ref: &str, response_body_ref: &str) -> String {
    format!(
        r#"{{
        "resourceLogs": [{{
            "resource": {{
                "attributes": [
                    {{ "key": "service.name", "value": {{ "stringValue": "claude_code" }} }},
                    {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                    {{ "key": "repo.path", "value": {{ "stringValue": "/Users/oyr/projects/kvasir" }} }},
                    {{ "key": "session.id", "value": {{ "stringValue": "claude-session-1" }} }},
                    {{ "key": "prompt.id", "value": {{ "stringValue": "claude-turn-1" }} }}
                ]
            }},
            "scopeLogs": [{{
                "logRecords": [{{
                    "timeUnixNano": "1781956802180000000",
                    "eventName": "claude_code.api_request_body",
                    "body": {{ "stringValue": "" }},
                    "attributes": [
                        {{ "key": "content.opt_in", "value": {{ "boolValue": true }} }},
                        {{ "key": "content.type", "value": {{ "stringValue": "raw_api_request" }} }},
                        {{ "key": "body_ref", "value": {{ "stringValue": "{request_body_ref}" }} }}
                    ]
                }},
                {{
                    "timeUnixNano": "1781956802220000000",
                    "eventName": "claude_code.api_response_body",
                    "body": {{ "stringValue": "" }},
                    "attributes": [
                        {{ "key": "content.opt_in", "value": {{ "boolValue": true }} }},
                        {{ "key": "content.type", "value": {{ "stringValue": "raw_api_response" }} }},
                        {{ "key": "body_ref", "value": {{ "stringValue": "{response_body_ref}" }} }}
                    ]
                }}]
            }}]
        }}]
    }}"#
    )
}

fn raw_api_body_ref_logs_fixture_for_harness(
    harness: &str,
    session_id: &str,
    prompt_id: &str,
    request_body_ref: &str,
) -> String {
    format!(
        r#"{{
        "resourceLogs": [{{
            "resource": {{
                "attributes": [
                    {{ "key": "service.name", "value": {{ "stringValue": "{harness}" }} }},
                    {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                    {{ "key": "repo.path", "value": {{ "stringValue": "/Users/oyr/projects/kvasir" }} }},
                    {{ "key": "session.id", "value": {{ "stringValue": "{session_id}" }} }},
                    {{ "key": "prompt.id", "value": {{ "stringValue": "{prompt_id}" }} }}
                ]
            }},
            "scopeLogs": [{{
                "logRecords": [{{
                    "timeUnixNano": "1781956802180000000",
                    "eventName": "{harness}.content",
                    "body": {{ "stringValue": "" }},
                    "attributes": [
                        {{ "key": "content.opt_in", "value": {{ "boolValue": true }} }},
                        {{ "key": "content.type", "value": {{ "stringValue": "raw_api_request" }} }},
                        {{ "key": "body_ref", "value": {{ "stringValue": "{request_body_ref}" }} }}
                    ]
                }}]
            }}]
        }}]
    }}"#
    )
}

fn raw_api_body_ref_logs_fixture_with_nanos(request_body_ref: &str, time_unix_nano: u64) -> String {
    format!(
        r#"{{
        "resourceLogs": [{{
            "resource": {{
                "attributes": [
                    {{ "key": "service.name", "value": {{ "stringValue": "claude_code" }} }},
                    {{ "key": "repo.name", "value": {{ "stringValue": "kvasir" }} }},
                    {{ "key": "repo.path", "value": {{ "stringValue": "/Users/oyr/projects/kvasir" }} }},
                    {{ "key": "session.id", "value": {{ "stringValue": "claude-session-1" }} }},
                    {{ "key": "prompt.id", "value": {{ "stringValue": "claude-turn-1" }} }}
                ]
            }},
            "scopeLogs": [{{
                "logRecords": [{{
                    "timeUnixNano": "{time_unix_nano}",
                    "eventName": "claude_code.api_request_body",
                    "body": {{ "stringValue": "" }},
                    "attributes": [
                        {{ "key": "content.opt_in", "value": {{ "boolValue": true }} }},
                        {{ "key": "content.type", "value": {{ "stringValue": "raw_api_request" }} }},
                        {{ "key": "body_ref", "value": {{ "stringValue": "{request_body_ref}" }} }}
                    ]
                }}]
            }}]
        }}]
    }}"#
    )
}

fn opencode_degraded_trace_fixture() -> &'static str {
    r#"{
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "opencode" } },
                    { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                    { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } },
                    { "key": "session.id", "value": { "stringValue": "opencode-session-2" } },
                    { "key": "prompt.id", "value": { "stringValue": "opencode-turn-2" } }
                ]
            },
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "dddddddddddddddddddddddddddddddd",
                        "spanId": "1111111111111111",
                        "name": "opencode.session",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802170000000",
                        "attributes": [
                            { "key": "opencode.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    },
                    {
                        "traceId": "dddddddddddddddddddddddddddddddd",
                        "spanId": "2222222222222222",
                        "parentSpanId": "1111111111111111",
                        "name": "ai.generateText.doGenerate",
                        "startTimeUnixNano": "1781956800120000000",
                        "endTimeUnixNano": "1781956801920000000",
                        "attributes": [
                            { "key": "ai.operationId", "value": { "stringValue": "ai.generateText" } },
                            { "key": "ai.usage.promptTokens", "value": { "intValue": "1200" } }
                        ]
                    },
                    {
                        "traceId": "dddddddddddddddddddddddddddddddd",
                        "spanId": "4444444444444444",
                        "parentSpanId": "1111111111111111",
                        "name": "runtime.flush",
                        "startTimeUnixNano": "1781956802170000000",
                        "endTimeUnixNano": "1781956802180000000",
                        "attributes": []
                    },
                    {
                        "traceId": "dddddddddddddddddddddddddddddddd",
                        "spanId": "3333333333333333",
                        "parentSpanId": "1111111111111111",
                        "name": "execute missing tool name",
                        "startTimeUnixNano": "1781956801920000000",
                        "endTimeUnixNano": "1781956802170000000",
                        "attributes": [
                            { "key": "ai.operationId", "value": { "stringValue": "toolCall" } }
                        ]
                    }
                ]
            }]
        }]
    }"#
}

fn codex_trace_protobuf_fixture() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("service.name", "codex"),
                    string_attribute("repo.name", "kvasir"),
                    string_attribute("repo.path", "/Users/oyr/projects/kvasir"),
                    string_attribute("session.id", "codex-session-1"),
                    string_attribute("prompt.id", "codex-turn-1"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![
                    Span {
                        trace_id: hex_bytes("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
                        span_id: hex_bytes("1111111111111111"),
                        trace_state: String::new(),
                        parent_span_id: Vec::new(),
                        flags: 0,
                        name: "codex.session".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_000_000_000,
                        end_time_unix_nano: 1_781_956_802_050_000_000,
                        attributes: vec![string_attribute("codex.span.kind", "interaction")],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                    Span {
                        trace_id: hex_bytes("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
                        span_id: hex_bytes("2222222222222222"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("1111111111111111"),
                        flags: 0,
                        name: "codex.llm_request".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_150_000_000,
                        end_time_unix_nano: 1_781_956_801_750_000_000,
                        attributes: vec![
                            string_attribute("codex.span.kind", "llm_request"),
                            string_attribute("model", "gpt-5.4"),
                        ],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                    Span {
                        trace_id: hex_bytes("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
                        span_id: hex_bytes("3333333333333333"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("1111111111111111"),
                        flags: 0,
                        name: "codex.tool".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_801_750_000_000,
                        end_time_unix_nano: 1_781_956_802_050_000_000,
                        attributes: vec![
                            string_attribute("codex.span.kind", "tool"),
                            string_attribute("tool.name", "Read"),
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

fn copilot_trace_protobuf_fixture() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("service.name", "github-copilot"),
                    string_attribute("repo.name", "kvasir"),
                    string_attribute("repo.path", "/repos/kvasir"),
                    string_attribute("conversation.id", "copilot-session-1"),
                    string_attribute("prompt.id", "copilot-turn-1"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![
                    Span {
                        trace_id: hex_bytes("ffffffffffffffffffffffffffffffff"),
                        span_id: hex_bytes("1111111111111111"),
                        trace_state: String::new(),
                        parent_span_id: Vec::new(),
                        flags: 0,
                        name: "github.copilot.chat".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_000_000_000,
                        end_time_unix_nano: 1_781_956_801_800_000_000,
                        attributes: vec![
                            string_attribute("gen_ai.operation.name", "chat"),
                            string_attribute("github.copilot.span.kind", "interaction"),
                        ],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                    Span {
                        trace_id: hex_bytes("ffffffffffffffffffffffffffffffff"),
                        span_id: hex_bytes("2222222222222222"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("1111111111111111"),
                        flags: 0,
                        name: "gen_ai.chat".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_200_000_000,
                        end_time_unix_nano: 1_781_956_801_600_000_000,
                        attributes: vec![
                            string_attribute("github.copilot.span.kind", "llm_request"),
                            string_attribute("model", "gpt-4.1"),
                        ],
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    },
                    Span {
                        trace_id: hex_bytes("ffffffffffffffffffffffffffffffff"),
                        span_id: hex_bytes("3333333333333333"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("1111111111111111"),
                        flags: 0,
                        name: "gen_ai.tool.call".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_801_600_000_000,
                        end_time_unix_nano: 1_781_956_802_000_000_000,
                        attributes: vec![
                            string_attribute("github.copilot.span.kind", "tool_call"),
                            string_attribute("gen_ai.tool.name", "Read"),
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

fn opencode_trace_protobuf_fixture() -> Vec<u8> {
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
                        span_id: hex_bytes("1111111111111111"),
                        trace_state: String::new(),
                        parent_span_id: Vec::new(),
                        flags: 0,
                        name: "opencode.session".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_000_000_000,
                        end_time_unix_nano: 1_781_956_802_170_000_000,
                        attributes: vec![
                            string_attribute("message.id", "opencode-turn-1"),
                            string_attribute("opencode.span.kind", "interaction"),
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
                        span_id: hex_bytes("2222222222222222"),
                        trace_state: String::new(),
                        parent_span_id: hex_bytes("1111111111111111"),
                        flags: 0,
                        name: "ai.generateText.doGenerate".to_owned(),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_120_000_000,
                        end_time_unix_nano: 1_781_956_801_920_000_000,
                        attributes: vec![
                            string_attribute("message.id", "opencode-turn-1"),
                            string_attribute("ai.operationId", "ai.generateText"),
                            string_attribute("ai.model.id", "gpt-4.1"),
                            int_attribute("ai.usage.promptTokens", 1200),
                            int_attribute("ai.usage.completionTokens", 450),
                            int_attribute("ai.usage.cachedInputTokens", 80),
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
                        parent_span_id: hex_bytes("1111111111111111"),
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

fn tool_call_metrics_fixture() -> &'static str {
    r#"{
        "resourceMetrics": [
            {
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "codex" } },
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.tool.call",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 2,
                                "attributes": [
                                    { "key": "tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            },
            {
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "github-copilot" } },
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/Users/oyr/projects/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
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

fn claude_raw_api_body_ref_logs_protobuf_fixture(
    request_body_ref: &str,
    response_body_ref: &str,
) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![OtlpResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    string_attribute("service.name", "claude_code"),
                    string_attribute("repo.name", "kvasir"),
                    string_attribute("repo.path", "/Users/oyr/projects/kvasir"),
                    string_attribute("session.id", "claude-session-1"),
                    string_attribute("prompt.id", "claude-turn-1"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![
                    LogRecord {
                        time_unix_nano: 1_781_956_802_180_000_000,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(String::new())),
                        }),
                        attributes: vec![
                            bool_attribute("content.opt_in", true),
                            string_attribute("content.type", "raw_api_request"),
                            string_attribute("body_ref", request_body_ref),
                        ],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: "claude_code.api_request_body".to_owned(),
                    },
                    LogRecord {
                        time_unix_nano: 1_781_956_802_220_000_000,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(String::new())),
                        }),
                        attributes: vec![
                            bool_attribute("content.opt_in", true),
                            string_attribute("content.type", "raw_api_response"),
                            string_attribute("body_ref", response_body_ref),
                        ],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: "claude_code.api_response_body".to_owned(),
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

fn int_attribute(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(any_value::Value::IntValue(value)),
        }),
    }
}

fn bool_attribute(key: &str, value: bool) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(any_value::Value::BoolValue(value)),
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

fn hex_nibble(character: u8) -> u8 {
    match character {
        b'0'..=b'9' => character - b'0',
        b'a'..=b'f' => character - b'a' + 10,
        b'A'..=b'F' => character - b'A' + 10,
        _ => panic!("test fixture hex contains invalid character"),
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

fn persisted_opencode_content_rows(
    database_path: &Path,
) -> anyhow::Result<Vec<(String, String, String)>> {
    let connection = open_test_store_connection(database_path)?;
    let mut statement = connection.prepare(
        "SELECT harness, content_kind, content
         FROM canonical_content_records
         ORDER BY occurred_at_ms, id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

struct PersistedRawBodyRow {
    harness: String,
    content_kind: String,
    compression: String,
    compressed_body: Vec<u8>,
}

fn persisted_raw_body_rows(database_path: &Path) -> anyhow::Result<Vec<PersistedRawBodyRow>> {
    let connection = open_test_store_connection(database_path)?;
    let mut statement = connection.prepare(
        "SELECT harness, content_kind, compression, compressed_body
         FROM canonical_raw_body_records
         ORDER BY event_key",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(PersistedRawBodyRow {
                harness: row.get(0)?,
                content_kind: row.get(1)?,
                compression: row.get(2)?,
                compressed_body: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn initialize_test_store(database_path: &Path) -> anyhow::Result<()> {
    let key = test_store_key();
    let store =
        UsageStore::open_with_price_table(database_path, &key, PriceTable::bundled_defaults())?;
    drop(store);
    Ok(())
}

fn open_test_store_connection(database_path: &Path) -> anyhow::Result<rusqlite::Connection> {
    let connection = rusqlite::Connection::open(database_path)?;
    let raw_key = test_store_key().to_hex_secret();
    let sql = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", raw_key.as_str()));
    connection.execute_batch(sql.as_str())?;
    Ok(connection)
}

fn test_store_key() -> StoreKey {
    StoreKey::from_bytes(TEST_STORE_KEY_BYTES)
}

fn sqlite_sidecar_test_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn repo_and_no_repo_rollup_query() -> RollupQuery {
    RollupQuery::new(
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 19, 0, 0, 0).unwrap()),
        TimestampMillis::from_datetime(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
    )
}

fn repo_and_no_repo_expected_rollups() -> anyhow::Result<Vec<TokenRollup>> {
    Ok(vec![
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
    ])
}

fn error_chain_contains(error: &anyhow::Error, expected: &str) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains(expected))
}

#[derive(Clone, Default)]
struct PersistentTestKeyring {
    passwords: Arc<StdMutex<HashMap<TestCredentialKey, Vec<u8>>>>,
    get_count: Arc<AtomicUsize>,
    set_count: Arc<AtomicUsize>,
    set_failures_remaining: Arc<AtomicUsize>,
    set_pause: Arc<StdMutex<Option<Arc<KeyringSetPause>>>>,
}

impl PersistentTestKeyring {
    fn install_as_default_keyring(&self) -> KeyringDefaultOverride {
        keyring::set_default_credential_builder(Box::new(PersistentTestCredentialBuilder {
            keyring: self.clone(),
        }));
        KeyringDefaultOverride
    }

    fn get_count(&self) -> usize {
        self.get_count.load(Ordering::SeqCst)
    }

    fn set_count(&self) -> usize {
        self.set_count.load(Ordering::SeqCst)
    }

    fn fail_next_set(&self) {
        self.set_failures_remaining.store(1, Ordering::SeqCst);
    }

    fn pause_next_set(&self) -> Arc<KeyringSetPause> {
        let pause = Arc::new(KeyringSetPause::new());
        *self.set_pause.lock().expect("test keyring mutex poisoned") = Some(pause.clone());
        pause
    }

    fn stored_password_count(&self) -> usize {
        self.passwords
            .lock()
            .expect("test keyring mutex poisoned")
            .len()
    }

    fn take_passwords(&self) -> HashMap<TestCredentialKey, Vec<u8>> {
        std::mem::take(&mut *self.passwords.lock().expect("test keyring mutex poisoned"))
    }

    fn replace_passwords(&self, passwords: HashMap<TestCredentialKey, Vec<u8>>) {
        self.passwords
            .lock()
            .expect("test keyring mutex poisoned")
            .extend(passwords);
    }
}

struct KeyringSetPause {
    set_paused: Barrier,
    set_released: Barrier,
}

impl KeyringSetPause {
    fn new() -> Self {
        Self {
            set_paused: Barrier::new(2),
            set_released: Barrier::new(2),
        }
    }

    fn wait_until_set_paused(&self) {
        self.set_paused.wait();
    }

    fn release_set(&self) {
        self.set_released.wait();
    }

    fn pause_set(&self) {
        self.set_paused.wait();
        self.set_released.wait();
    }
}

struct KeyringDefaultOverride;

impl Drop for KeyringDefaultOverride {
    fn drop(&mut self) {
        keyring::set_default_credential_builder(keyring::default::default_credential_builder());
    }
}

struct PersistentTestCredentialBuilder {
    keyring: PersistentTestKeyring,
}

impl CredentialBuilderApi for PersistentTestCredentialBuilder {
    fn build(
        &self,
        target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(PersistentTestCredential {
            key: TestCredentialKey {
                target: target.map(str::to_owned),
                service: service.to_owned(),
                user: user.to_owned(),
            },
            keyring: self.keyring.clone(),
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn persistence(&self) -> CredentialPersistence {
        CredentialPersistence::UntilDelete
    }
}

struct PersistentTestCredential {
    key: TestCredentialKey,
    keyring: PersistentTestKeyring,
}

impl CredentialApi for PersistentTestCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.keyring.set_count.fetch_add(1, Ordering::SeqCst);
        if self
            .keyring
            .set_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("configured keyring write failure"),
            )));
        }
        self.keyring
            .passwords
            .lock()
            .expect("test keyring mutex poisoned")
            .insert(self.key.clone(), secret.to_vec());
        if let Some(pause) = self
            .keyring
            .set_pause
            .lock()
            .expect("test keyring mutex poisoned")
            .take()
        {
            pause.pause_set();
        }
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.keyring.get_count.fetch_add(1, Ordering::SeqCst);
        self.keyring
            .passwords
            .lock()
            .expect("test keyring mutex poisoned")
            .get(&self.key)
            .cloned()
            .ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        let removed = self
            .keyring
            .passwords
            .lock()
            .expect("test keyring mutex poisoned")
            .remove(&self.key);
        match removed {
            Some(_) => Ok(()),
            None => Err(keyring::Error::NoEntry),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn debug_fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PersistentTestCredential(<redacted>)")
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct TestCredentialKey {
    target: Option<String>,
    service: String,
    user: String,
}

struct DirectoryPermissionsRestore {
    path: PathBuf,
    mode: u32,
}

impl DirectoryPermissionsRestore {
    fn make_read_only(path: PathBuf) -> anyhow::Result<Self> {
        let mode = std::fs::metadata(&path)?.permissions().mode();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500))?;
        Ok(Self { path, mode })
    }
}

impl Drop for DirectoryPermissionsRestore {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
    }
}

fn insert_raw_body_import_queue_row(
    database_path: &Path,
    body_ref: &str,
    occurred_at_ms: i64,
) -> anyhow::Result<String> {
    let connection = open_test_store_connection(database_path)?;
    let occurred_at_nanos = occurred_at_ms * 1_000_000;
    let event_key = format!(
        "otel-raw-body-file\nrepo_bucket=repo\nrepo_name=kvasir\nrepo_path=/Users/oyr/projects/kvasir\nharness=claude_code\nsession_id=claude-session-1\nprompt_id=claude-turn-1\nkind=raw_api_request\noccurred_at_nanos={occurred_at_nanos}\nbody_ref={body_ref}\n"
    );
    connection.execute(
        "INSERT INTO raw_body_import_queue (
            event_key,
            occurred_at_ms,
            session_id,
            prompt_id,
            day,
            repo_bucket,
            repo_name,
            repo_path,
            harness,
            content_kind,
            body_ref
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        (
            event_key.as_str(),
            occurred_at_ms,
            "claude-session-1",
            "claude-turn-1",
            "2026-06-20",
            "repo",
            "kvasir",
            "/Users/oyr/projects/kvasir",
            "claude_code",
            "raw_api_request",
            body_ref,
        ),
    )?;
    Ok(event_key)
}

fn insert_persisted_raw_body_row_for_event(
    database_path: &Path,
    event_key: &str,
    compression: &str,
) -> anyhow::Result<()> {
    let connection = open_test_store_connection(database_path)?;
    connection.execute(
        "INSERT INTO canonical_raw_body_records (
            event_key,
            occurred_at_ms,
            session_id,
            prompt_id,
            day,
            repo_bucket,
            repo_name,
            repo_path,
            harness,
            content_kind,
            compression,
            compressed_body
        )
        SELECT
            event_key,
            occurred_at_ms,
            session_id,
            prompt_id,
            day,
            repo_bucket,
            repo_name,
            repo_path,
            harness,
            content_kind,
            ?2,
            x'00'
        FROM raw_body_import_queue
        WHERE event_key = ?1",
        (event_key, compression),
    )?;
    Ok(())
}

fn raw_body_import_queue_body_refs(database_path: &Path) -> anyhow::Result<Vec<String>> {
    let connection = open_test_store_connection(database_path)?;
    let mut statement =
        connection.prepare("SELECT body_ref FROM raw_body_import_queue ORDER BY body_ref")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn delete_raw_body_import_queue_event(database_path: &Path, event_key: &str) -> anyhow::Result<()> {
    let connection = open_test_store_connection(database_path)?;
    connection.execute(
        "DELETE FROM raw_body_import_queue WHERE event_key = ?1",
        [event_key],
    )?;
    Ok(())
}
