use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;

use base64::Engine;
use chrono::{TimeZone, Utc};
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use kvasir_core::rpc::{
    BearerToken, ModelName, RollupDay, RollupQuery, TimestampMillis, TokenRollup,
};
use kvasird::{DaemonConfig, query_token_rollup, start};
use tempfile::tempdir;

#[tokio::test]
async fn golden_claude_metrics_replay_returns_per_model_day_rollup() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let bearer_token = BearerToken::new("test-token");
    let daemon = start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token,
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
                model: ModelName::new("claude-opus-4-20250514"),
                input_tokens: 1100,
                output_tokens: 550,
                cache_tokens: 125,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-20")?,
                model: ModelName::new("claude-sonnet-4-20250514"),
                input_tokens: 300,
                output_tokens: 120,
                cache_tokens: 30,
            },
            TokenRollup {
                day: RollupDay::parse("2026-06-21")?,
                model: ModelName::new("claude-sonnet-4-20250514"),
                input_tokens: 2100,
                output_tokens: 900,
                cache_tokens: 75,
            }
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
async fn daemon_refuses_to_replace_non_socket_rpc_path() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("not-a-socket");
    std::fs::write(&rpc_socket_path, "do not remove")?;

    let result = start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
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
    let _daemon = start(DaemonConfig {
        otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        rpc_socket_path: rpc_socket_path.clone(),
        database_path: temp.path().join("usage.sqlite3"),
        bearer_token: BearerToken::new("test-token"),
    })
    .await?;

    let mode = std::fs::metadata(rpc_socket_path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    Ok(())
}

fn claude_token_usage_protobuf_fixture() -> anyhow::Result<Vec<u8>> {
    let encoded = include_str!("fixtures/claude_token_usage_otlp.pb.base64").trim();
    Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
}
