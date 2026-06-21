use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use kvasir_client::{
    KvasirClient, KvasirCostRollup, KvasirCostUsd, KvasirModelName, KvasirRepoBucket,
    KvasirRepoBucketKind, KvasirRepoName, KvasirRepoPath, KvasirRollupDay, KvasirRollupQuery,
    KvasirSocketPath, KvasirTimestampMillis, KvasirTokenRollup, KvasirTokenRollupUpdate,
};
use kvasir_core::rpc::BearerToken;
use kvasird::{DaemonConfig, StoreKeySource, start_with_store_key_source};
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
async fn client_subscription_delivers_live_token_rollup_updates() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let rpc_socket_path = temp.path().join("kvasird.sock");
    let daemon = start_with_store_key_source(
        DaemonConfig {
            otlp_bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rpc_socket_path: rpc_socket_path.clone(),
            database_path: temp.path().join("usage.sqlite3"),
            bearer_token: BearerToken::new("test-token"),
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
        },
        StoreKeySource::static_key_for_test([11; 32]),
    )
    .await?;

    connecting.await??;

    Ok(())
}

fn kvasir_repo() -> KvasirRepoBucket {
    KvasirRepoBucket {
        kind: KvasirRepoBucketKind::Repo,
        name: Some(KvasirRepoName {
            value: "kvasir".to_owned(),
        }),
        path: Some(KvasirRepoPath {
            value: "/Users/oyr/projects/kvasir".to_owned(),
        }),
    }
}

fn socket_path(path: std::path::PathBuf) -> KvasirSocketPath {
    KvasirSocketPath {
        value: path.to_string_lossy().into_owned(),
    }
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
    KvasirModelName {
        value: value.to_owned(),
    }
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
