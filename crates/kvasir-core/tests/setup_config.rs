use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use kvasir_core::{BearerToken, ClaudeCodeSettings, KvasirEndpoint, RawBodyDirectory, SetupConfig};
use serde_json::json;

#[test]
fn claude_code_settings_enable_kvasir_telemetry() -> Result<(), Box<dyn std::error::Error>> {
    let settings = ClaudeCodeSettings::generate(
        "{}",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    let generated: serde_json::Value = serde_json::from_str(settings.as_str())?;
    assert_eq!(
        generated["env"],
        json!({
            "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
            "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
            "CLAUDE_CODE_ENABLE_TRACE_BETA": "1",
            "CLAUDE_CODE_ENABLE_CONTENT_GATES": "1",
            "OTEL_EXPORTER_OTLP_ENDPOINT": "http://127.0.0.1:4318",
            "OTEL_EXPORTER_OTLP_HEADERS": "Authorization=Bearer test-token",
            "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
            "OTEL_LOGS_EXPORTER": "otlp",
            "OTEL_LOG_RAW_API_BODIES": "file:/tmp/kvasir/raw-bodies",
            "OTEL_LOG_TOOL_CONTENT": "1",
            "OTEL_LOG_TOOL_DETAILS": "1",
            "OTEL_LOG_USER_PROMPTS": "1",
            "OTEL_METRICS_EXPORTER": "otlp",
            "OTEL_TRACES_EXPORTER": "otlp"
        })
    );

    assert_eq!(
        settings.managed_env_keys(),
        [
            "CLAUDE_CODE_ENABLE_TELEMETRY",
            "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA",
            "CLAUDE_CODE_ENABLE_TRACE_BETA",
            "CLAUDE_CODE_ENABLE_CONTENT_GATES",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_LOGS_EXPORTER",
            "OTEL_LOG_RAW_API_BODIES",
            "OTEL_LOG_TOOL_CONTENT",
            "OTEL_LOG_TOOL_DETAILS",
            "OTEL_LOG_USER_PROMPTS",
            "OTEL_METRICS_EXPORTER",
            "OTEL_TRACES_EXPORTER"
        ]
    );
    assert_eq!(
        generated["kvasirManaged"]["env"],
        json!([
            "CLAUDE_CODE_ENABLE_TELEMETRY",
            "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA",
            "CLAUDE_CODE_ENABLE_TRACE_BETA",
            "CLAUDE_CODE_ENABLE_CONTENT_GATES",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_LOGS_EXPORTER",
            "OTEL_LOG_RAW_API_BODIES",
            "OTEL_LOG_TOOL_CONTENT",
            "OTEL_LOG_TOOL_DETAILS",
            "OTEL_LOG_USER_PROMPTS",
            "OTEL_METRICS_EXPORTER",
            "OTEL_TRACES_EXPORTER"
        ])
    );

    Ok(())
}

#[test]
fn claude_code_settings_replace_only_kvasir_managed_env() -> Result<(), Box<dyn std::error::Error>>
{
    let first = ClaudeCodeSettings::generate(
        r#"{
          "theme": "dark",
          "kvasirManaged": {
            "env": ["STALE_KEY"]
          },
          "env": {
            "PATH": "/usr/bin",
            "STALE_KEY": "remove-me",
            "CLAUDE_CODE_ENABLE_TRACE_BETA": "0",
            "OTEL_EXPORTER_OTLP_ENDPOINT": "http://old.example"
          }
        }"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("first-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let second = ClaudeCodeSettings::generate(
        first.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/new-raw-bodies")),
        ),
    )?;
    let third = ClaudeCodeSettings::generate(
        second.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/new-raw-bodies")),
        ),
    )?;

    assert_eq!(second.as_str(), third.as_str());

    let generated: serde_json::Value = serde_json::from_str(second.as_str())?;
    assert_eq!(generated["theme"], "dark");
    assert_eq!(generated["env"]["PATH"], "/usr/bin");
    assert!(generated["env"].get("STALE_KEY").is_none());
    assert_eq!(generated["env"]["CLAUDE_CODE_ENABLE_TRACE_BETA"], "1");
    assert_eq!(generated["env"]["CLAUDE_CODE_ENABLE_CONTENT_GATES"], "1");
    assert_eq!(
        generated["env"]["OTEL_EXPORTER_OTLP_ENDPOINT"],
        "http://127.0.0.1:4319"
    );
    assert_eq!(
        generated["env"]["OTEL_EXPORTER_OTLP_HEADERS"],
        "Authorization=Bearer second-token"
    );
    assert_eq!(
        generated["env"]["OTEL_LOG_RAW_API_BODIES"],
        "file:/tmp/kvasir/new-raw-bodies"
    );
    assert_eq!(
        generated["kvasirManaged"]["env"],
        json!(second.managed_env_keys())
    );

    Ok(())
}

#[test]
fn bearer_tokens_are_generated_as_hex_secrets() -> Result<(), Box<dyn std::error::Error>> {
    let token = BearerToken::generate()?;

    assert_eq!(token.as_str().len(), 64);
    assert!(
        token
            .as_str()
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    assert_eq!(
        token.authorization_header(),
        format!("Bearer {}", token.as_str())
    );
    assert_eq!(format!("{token:?}"), "BearerToken(<redacted>)");

    Ok(())
}

#[test]
fn kvasir_endpoint_is_generated_from_otlp_socket_address() {
    let endpoint = KvasirEndpoint::from_otlp_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 4318)));

    assert_eq!(endpoint.as_str(), "http://127.0.0.1:4318");
}
