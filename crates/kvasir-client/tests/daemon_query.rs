use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use kvasir_client::{
    KvasirBearerToken, KvasirClient, KvasirClientError, KvasirContentAvailability,
    KvasirContentKind, KvasirContentKindAvailability, KvasirContentQuery, KvasirContentReplay,
    KvasirContentReplayItem, KvasirContentText, KvasirContentUnavailableReason, KvasirCostRollup,
    KvasirCostUsd, KvasirHarnessName, KvasirModelName, KvasirOverviewRollup, KvasirPromptId,
    KvasirRepoBucket, KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath, KvasirRollupDay,
    KvasirRollupQuery, KvasirSessionId, KvasirSocketPath, KvasirSpanId, KvasirSpanName,
    KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate, KvasirToolCallRollup,
    KvasirToolName, KvasirTraceDurationMeasures, KvasirTraceId, KvasirTraceQuery, KvasirTraceSpan,
    KvasirTraceSpanKind,
};
use kvasir_core::PriceTable;
use kvasir_core::rpc::BearerToken;
use kvasird::{DaemonConfig, StoreKeySource, start_with_store_key_source};
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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("opencode"),
            session_id: session("opencode-session-1"),
            prompt_id: prompt("opencode-turn-1"),
            bearer_token: bearer_token("test-token"),
        })
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
                ],
            },
        }
    );

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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("claude"),
            session_id: session("claude-session-1"),
            prompt_id: prompt("claude-turn-1"),
            bearer_token: bearer_token("test-token"),
        })
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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("codex"),
            session_id: session("codex-session-1"),
            prompt_id: prompt("codex-turn-1"),
            bearer_token: bearer_token("test-token"),
        })
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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("random_service"),
            session_id: session("unknown-session-1"),
            prompt_id: prompt("unknown-turn-1"),
            bearer_token: bearer_token("test-token"),
        })
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
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let replay = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("opencode"),
            session_id: session("missing-session"),
            prompt_id: prompt("missing-prompt"),
            bearer_token: bearer_token("test-token"),
        })
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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("claude"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
            bearer_token: bearer_token("test-token"),
        })
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
                ],
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
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.content_replay(KvasirContentQuery {
            harness: harness("codex"),
            session_id: session("session-12"),
            prompt_id: prompt("prompt-7"),
            bearer_token: bearer_token("test-token"),
        })
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
    };
    let overview = tokio::task::spawn_blocking(move || {
        let client = KvasirClient::connect(socket_path(rpc_socket_path))?;
        client.overview_rollups(query)
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
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    let query = KvasirRollupQuery {
        start: timestamp(2026, 6, 19),
        end: timestamp(2026, 6, 22),
        repo: Some(kvasir_repo()),
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
async fn client_connect_retries_until_daemon_socket_is_available() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let connecting_socket_path = rpc_socket_path.clone();
    let connecting = tokio::task::spawn_blocking(move || {
        KvasirClient::connect(socket_path(connecting_socket_path))
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    let _daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path,
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    connecting.await??;

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

fn socket_path(path: std::path::PathBuf) -> KvasirSocketPath {
    KvasirSocketPath::try_from(path.to_string_lossy().into_owned()).unwrap()
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

fn bearer_token(value: &str) -> KvasirBearerToken {
    KvasirBearerToken::try_from(value.to_owned()).unwrap()
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
