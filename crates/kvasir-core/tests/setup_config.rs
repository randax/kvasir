use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use kvasir_core::{
    BearerToken, ClaudeCodeSettings, CodexConfigToml, CopilotShellProfile, KvasirEndpoint,
    OpenCodeSetup, RawBodyDirectory, SetupConfig,
    setup::{RepoInjectionShell, RepoInjectionShellHook, RepoInjectionShellProfile},
};
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
fn claude_code_settings_reject_non_object_settings() {
    let err = ClaudeCodeSettings::generate(
        "[]",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("Claude Code settings root must be an object");

    assert!(matches!(err, kvasir_core::SetupError::SettingsNotObject));
}

#[test]
fn claude_code_settings_reject_invalid_json() {
    let err = ClaudeCodeSettings::generate(
        "{",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("invalid Claude Code settings JSON must be rejected");

    assert!(matches!(
        err,
        kvasir_core::SetupError::InvalidSettingsJson(_)
    ));
}

#[test]
fn claude_code_settings_reject_non_object_env() {
    let err = ClaudeCodeSettings::generate(
        r#"{
  "env": []
}"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("Claude Code settings env field must be an object");

    assert!(matches!(err, kvasir_core::SetupError::EnvNotObject));
}

#[test]
fn codex_config_toml_enables_kvasir_otlp_http_logs() -> Result<(), Box<dyn std::error::Error>> {
    let config = SetupConfig::new(
        KvasirEndpoint::new("http://127.0.0.1:4318"),
        BearerToken::new("test-token"),
        RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
    );

    let generated = CodexConfigToml::generate(
        r#"model = "gpt-5.4"

[tui]
status_line = true
"#,
        &config,
    )?;

    assert_eq!(
        generated.as_str(),
        r#"model = "gpt-5.4"

[tui]
status_line = true

# BEGIN KVASIR MANAGED CODEX OTEL
[otel]
log_user_prompt = true
exporter = { otlp-http = { endpoint = "http://127.0.0.1:4318/v1/logs", protocol = "binary", headers = { "Authorization" = "Bearer test-token" } } }
trace_exporter = { otlp-http = { endpoint = "http://127.0.0.1:4318/v1/traces", protocol = "binary", headers = { "Authorization" = "Bearer test-token" } } }
metrics_exporter = { otlp-http = { endpoint = "http://127.0.0.1:4318/v1/metrics", protocol = "binary", headers = { "Authorization" = "Bearer test-token" } } }
# END KVASIR MANAGED CODEX OTEL
"#
    );
    Ok(())
}

#[test]
fn codex_config_toml_replaces_only_kvasir_managed_block() -> Result<(), Box<dyn std::error::Error>>
{
    let first = CodexConfigToml::generate(
        r#"model = "gpt-5.4"

# BEGIN KVASIR MANAGED CODEX OTEL
[otel]
log_user_prompt = false
exporter = "none"
# END KVASIR MANAGED CODEX OTEL

[tools]
view_image = true
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318/"),
            BearerToken::new("first-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let second = CodexConfigToml::generate(
        first.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let third = CodexConfigToml::generate(
        second.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(second.as_str(), third.as_str());
    assert!(second.as_str().contains("model = \"gpt-5.4\""));
    assert!(second.as_str().contains("[tools]\nview_image = true"));
    assert!(
        second
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4319/v1/logs\"")
    );
    assert!(
        second
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4319/v1/traces\"")
    );
    assert!(
        second
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4319/v1/metrics\"")
    );
    assert!(
        second
            .as_str()
            .contains("\"Authorization\" = \"Bearer second-token\"")
    );
    assert!(!second.as_str().contains("first-token"));
    Ok(())
}

#[test]
fn codex_config_toml_inserts_managed_values_inside_existing_otel_table()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        r#"model = "gpt-5.4"

[otel]
environment = "dev"

[tools]
view_image = true
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(generated.as_str().matches("[otel]").count(), 1);
    assert!(
        generated
            .as_str()
            .contains("[otel]\n# BEGIN KVASIR MANAGED CODEX OTEL")
    );
    assert!(generated.as_str().contains("environment = \"dev\""));
    assert!(generated.as_str().contains("[tools]\nview_image = true"));
    Ok(())
}

#[test]
fn codex_config_toml_replaces_unmanaged_otel_assignments() -> Result<(), Box<dyn std::error::Error>>
{
    let generated = CodexConfigToml::generate(
        r#"[otel]
environment = "dev"
log_user_prompt = false
exporter = "none"
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(generated.as_str().contains("environment = \"dev\""));
    assert!(generated.as_str().contains("log_user_prompt = true"));
    assert!(
        generated
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4318/v1/logs\"")
    );
    assert!(!generated.as_str().contains("exporter = \"none\""));
    Ok(())
}

#[test]
fn codex_config_toml_replaces_unmanaged_otel_subtables() -> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        r#"[otel]
log_user_prompt = true
environment = "dev"

[otel.exporter.otlp-http]
endpoint = "http://old.example/v1/logs"
protocol = "binary"

[otel.exporter.otlp-http.headers]
Authorization = "Bearer old-token"

[otel.trace_exporter.otlp-http]
endpoint = "http://old.example/v1/traces"
protocol = "binary"

[otel.metrics_exporter.otlp-http]
endpoint = "http://old.example/v1/metrics"
protocol = "binary"

[tools]
view_image = true
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(generated.as_str().contains("environment = \"dev\""));
    assert!(generated.as_str().contains("[tools]\nview_image = true"));
    assert!(
        generated
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4318/v1/logs\"")
    );
    assert!(
        generated
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4318/v1/traces\"")
    );
    assert!(
        generated
            .as_str()
            .contains("endpoint = \"http://127.0.0.1:4318/v1/metrics\"")
    );
    assert!(!generated.as_str().contains("[otel.exporter.otlp-http]"));
    assert!(
        !generated
            .as_str()
            .contains("[otel.trace_exporter.otlp-http]")
    );
    assert!(
        !generated
            .as_str()
            .contains("[otel.metrics_exporter.otlp-http]")
    );
    assert!(!generated.as_str().contains("old-token"));
    Ok(())
}

#[test]
fn codex_config_toml_inserts_after_final_otel_header_without_concatenating_marker()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        "[otel]",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(
        generated
            .as_str()
            .starts_with("[otel]\n# BEGIN KVASIR MANAGED CODEX OTEL\n")
    );
    Ok(())
}

#[test]
fn codex_config_toml_preserves_crlf_when_inserting_managed_otel_values()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        "model = \"gpt-5.4\"\r\n\r\n[otel]\r\nenvironment = \"dev\"\r\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_lone_lf(generated.as_str());
    assert!(
        generated
            .as_str()
            .contains("[otel]\r\n# BEGIN KVASIR MANAGED CODEX OTEL\r\n")
    );
    assert!(generated.as_str().contains("environment = \"dev\"\r\n"));
    Ok(())
}

#[test]
fn codex_config_toml_normalizes_mixed_line_endings_to_dominant_crlf()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        "model = \"gpt-5.4\"\r\n\r\n[otel]\nenvironment = \"dev\"\r\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_lone_lf(generated.as_str());
    assert!(
        generated
            .as_str()
            .contains("model = \"gpt-5.4\"\r\n\r\n[otel]\r\n# BEGIN")
    );
    assert!(
        generated
            .as_str()
            .contains("# END KVASIR MANAGED CODEX OTEL\r\nenvironment = \"dev\"\r\n")
    );
    Ok(())
}

#[test]
fn codex_config_toml_prefers_unmanaged_lf_over_removed_crlf_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        "model = \"gpt-5.4\"\n\n# BEGIN KVASIR MANAGED CODEX OTEL\r\n[otel]\r\nlog_user_prompt = false\r\nexporter = \"none\"\r\ntrace_exporter = \"none\"\r\nmetrics_exporter = \"none\"\r\n# END KVASIR MANAGED CODEX OTEL\r\n\n[tools]\nview_image = true\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_cr(generated.as_str());
    assert!(
        generated
            .as_str()
            .contains("model = \"gpt-5.4\"\n\n[tools]")
    );
    assert!(
        generated
            .as_str()
            .contains("[tools]\nview_image = true\n\n# BEGIN KVASIR MANAGED CODEX OTEL\n")
    );
    Ok(())
}

#[test]
fn codex_config_toml_recognizes_existing_otel_table_with_inline_comment()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        r#"model = "gpt-5.4"

[otel] # owner comment
environment = "dev"

[tools]
view_image = true
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(generated.as_str().matches("[otel]").count(), 1);
    assert!(
        generated
            .as_str()
            .contains("[otel] # owner comment\n# BEGIN KVASIR MANAGED CODEX OTEL")
    );
    assert!(generated.as_str().contains("[tools]\nview_image = true"));
    Ok(())
}

#[test]
fn codex_config_toml_escapes_endpoint_and_token_as_toml_strings()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        "",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318/otel?label=\"dev\\ops\"\nnext"),
            BearerToken::new("test\"token\\next\nline"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(generated.as_str().contains(
        "endpoint = \"http://127.0.0.1:4318/otel?label=\\\"dev\\\\ops\\\"\\nnext/v1/logs\""
    ));
    assert!(
        generated
            .as_str()
            .contains("\"Authorization\" = \"Bearer test\\\"token\\\\next\\nline\"")
    );
    Ok(())
}

#[test]
fn codex_config_toml_rejects_whitespace_corrupted_managed_markers() {
    let err = CodexConfigToml::generate(
        r#"  # BEGIN KVASIR MANAGED CODEX OTEL
[otel]
exporter = { otlp-http = { endpoint = "http://old.example/v1/logs", protocol = "binary", headers = { "Authorization" = "Bearer stale-token" } } }
# END KVASIR MANAGED CODEX OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("whitespace-corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn codex_config_toml_rejects_internally_corrupted_managed_markers() {
    let err = CodexConfigToml::generate(
        r#"# BEGIN KVASIR  MANAGED CODEX OTEL
[otel]
exporter = { otlp-http = { endpoint = "http://old.example/v1/logs", protocol = "binary", headers = { "Authorization" = "Bearer stale-token" } } }
# END KVASIR MANAGED CODEX  OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("internally corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn codex_config_toml_rejects_suffix_corrupted_managed_markers() {
    let err = CodexConfigToml::generate(
        r#"# BEGIN KVASIR MANAGED CODEX OTEL extra
[otel]
exporter = { otlp-http = { endpoint = "http://old.example/v1/logs", protocol = "binary", headers = { "Authorization" = "Bearer stale-token" } } }
# END KVASIR MANAGED CODEX OTEL extra
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("suffix-corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn opencode_setup_generates_otlp_env_and_enables_open_telemetry()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = OpenCodeSetup::generate(
        r#"{
  "theme": "system"
}"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(
        generated.env().otlp_endpoint().as_str(),
        "http://127.0.0.1:4318"
    );
    assert_eq!(
        generated.env().otlp_headers(),
        "Authorization=Bearer test-token"
    );
    let endpoint_variable = generated.otlp_endpoint_variable();
    assert_eq!(
        endpoint_variable.key().as_str(),
        "OTEL_EXPORTER_OTLP_ENDPOINT"
    );
    assert_eq!(endpoint_variable.value(), "http://127.0.0.1:4318");
    let headers_variable = generated.otlp_headers_variable();
    assert_eq!(
        headers_variable.key().as_str(),
        "OTEL_EXPORTER_OTLP_HEADERS"
    );
    assert_eq!(headers_variable.value(), "Authorization=Bearer test-token");

    let opencode_json: serde_json::Value = serde_json::from_str(generated.opencode_json())?;
    assert_eq!(opencode_json["theme"], "system");
    assert_eq!(opencode_json["experimental"]["openTelemetry"], true);
    let experimental = opencode_json["experimental"]
        .as_object()
        .expect("experimental config is an object");
    assert!(!experimental.contains_key("recordInputs"));
    assert!(!experimental.contains_key("recordOutputs"));
    assert_eq!(
        opencode_json["kvasirManaged"]["experimental"],
        json!(generated.managed_experimental_keys())
    );

    Ok(())
}

#[test]
fn opencode_setup_replaces_only_kvasir_managed_experimental_config()
-> Result<(), Box<dyn std::error::Error>> {
    let first = OpenCodeSetup::generate(
        r#"{
  "theme": "system",
  "experimental": {
    "localFeature": false,
    "openTelemetry": false
  },
  "kvasirManaged": {
    "experimental": ["localFeature"],
    "futureSection": ["futureKey"]
  }
}"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("first-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let second = OpenCodeSetup::generate(
        first.opencode_json(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let third = OpenCodeSetup::generate(
        second.opencode_json(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(second.opencode_json(), third.opencode_json());
    assert_eq!(
        second.env().otlp_endpoint().as_str(),
        "http://127.0.0.1:4319"
    );
    assert_eq!(
        second.env().otlp_headers(),
        "Authorization=Bearer second-token"
    );

    let opencode_json: serde_json::Value = serde_json::from_str(second.opencode_json())?;
    assert_eq!(opencode_json["theme"], "system");
    assert_eq!(opencode_json["experimental"]["localFeature"], false);
    assert_eq!(opencode_json["experimental"]["openTelemetry"], true);
    assert_eq!(
        opencode_json["kvasirManaged"]["experimental"],
        json!(second.managed_experimental_keys())
    );
    assert_eq!(
        opencode_json["kvasirManaged"]["futureSection"],
        json!(["futureKey"])
    );
    assert!(!second.opencode_json().contains("first-token"));
    assert!(!second.opencode_json().contains("second-token"));

    Ok(())
}

#[test]
fn opencode_setup_rejects_invalid_json() {
    let err = OpenCodeSetup::generate(
        "{",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("invalid OpenCode JSON must be rejected");

    assert!(matches!(
        err,
        kvasir_core::SetupError::InvalidOpenCodeConfigJson(_)
    ));
    let source = std::error::Error::source(&err)
        .expect("invalid OpenCode JSON should expose serde_json::Error as source");
    assert!(source.downcast_ref::<serde_json::Error>().is_some());
}

#[test]
fn opencode_setup_rejects_non_object_config() {
    let err = OpenCodeSetup::generate(
        "[]",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode config root must be an object");

    assert!(matches!(
        err,
        kvasir_core::SetupError::OpenCodeConfigNotObject
    ));
}

#[test]
fn opencode_setup_rejects_non_object_experimental_config() {
    let err = OpenCodeSetup::generate(
        r#"{
  "experimental": []
}"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode experimental config must be an object");

    assert!(matches!(
        err,
        kvasir_core::SetupError::OpenCodeExperimentalNotObject
    ));
}

#[test]
fn opencode_setup_rejects_non_object_kvasir_managed_block() {
    let err = OpenCodeSetup::generate(
        r#"{
  "kvasirManaged": "user-data"
}"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode kvasir managed block must be an object");

    assert!(matches!(
        err,
        kvasir_core::SetupError::OpenCodeManagedBlockNotObject
    ));
}

#[test]
fn opencode_setup_rejects_control_characters_in_endpoint_env() {
    let err = OpenCodeSetup::generate(
        "{}",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318\nnext"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode endpoint env must reject control characters");

    assert!(matches!(
        err,
        kvasir_core::SetupError::InvalidOpenCodeOtlpEndpointEnvValue
    ));
}

#[test]
fn opencode_setup_rejects_header_delimiters_in_bearer_token_env() {
    let err = OpenCodeSetup::generate(
        "{}",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token,Other=header"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode OTLP headers env must reject extra header delimiters");

    assert!(matches!(
        err,
        kvasir_core::SetupError::InvalidOpenCodeOtlpHeadersEnvValue
    ));
}

#[test]
fn opencode_setup_rejects_control_characters_in_bearer_token_env() {
    let err = OpenCodeSetup::generate(
        "{}",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token\nOther=header"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("OpenCode OTLP headers env must reject control characters");

    assert!(matches!(
        err,
        kvasir_core::SetupError::InvalidOpenCodeOtlpHeadersEnvValue
    ));
}

#[test]
fn repo_injection_zsh_profile_sources_managed_hook() -> Result<(), Box<dyn std::error::Error>> {
    let generated = RepoInjectionShellProfile::generate(
        "export PATH='/usr/local/bin:$PATH'\n",
        Path::new("/Users/oyr/.kvasir/repo-hook.zsh"),
    )?;

    assert_eq!(
        generated.as_str(),
        r#"export PATH='/usr/local/bin:$PATH'

# BEGIN KVASIR MANAGED REPO OTEL
if [ -f '/Users/oyr/.kvasir/repo-hook.zsh' ] && [ -r '/Users/oyr/.kvasir/repo-hook.zsh' ]; then . '/Users/oyr/.kvasir/repo-hook.zsh'; fi
# END KVASIR MANAGED REPO OTEL
"#
    );

    let hook = RepoInjectionShellHook::generate(RepoInjectionShell::Zsh);
    assert!(
        hook.as_str()
            .contains("git rev-parse --show-toplevel 2>/dev/null")
    );
    assert!(
        hook.as_str()
            .contains("_kvasir_escape_otel_resource_attribute_value")
    );
    assert!(hook.as_str().contains("autoload -Uz add-zsh-hook"));
    assert!(
        hook.as_str()
            .contains("add-zsh-hook chpwd _kvasir_update_otel_repo_resource")
    );
    assert!(
        hook.as_str()
            .contains("add-zsh-hook precmd _kvasir_update_otel_repo_resource")
    );
    assert!(hook.as_str().contains(
        "export OTEL_RESOURCE_ATTRIBUTES=\"${current_resource_attributes:+${current_resource_attributes},}repo.name=${escaped_repo_name},repo.path=${escaped_repo_path}\""
    ));
    Ok(())
}

#[test]
fn repo_injection_bash_profile_sources_managed_hook() -> Result<(), Box<dyn std::error::Error>> {
    let generated = RepoInjectionShellProfile::generate(
        "alias gs='git status'\n",
        Path::new("/Users/oyr/.kvasir/repo-hook.bash"),
    )?;

    assert_eq!(
        generated.as_str(),
        r#"alias gs='git status'

# BEGIN KVASIR MANAGED REPO OTEL
if [ -f '/Users/oyr/.kvasir/repo-hook.bash' ] && [ -r '/Users/oyr/.kvasir/repo-hook.bash' ]; then . '/Users/oyr/.kvasir/repo-hook.bash'; fi
# END KVASIR MANAGED REPO OTEL
"#
    );

    let hook = RepoInjectionShellHook::generate(RepoInjectionShell::Bash);
    assert!(hook.as_str().contains("case \";${PROMPT_COMMAND:-};\" in"));
    assert!(
        hook.as_str()
            .contains("PROMPT_COMMAND=\"_kvasir_update_otel_repo_resource; ${PROMPT_COMMAND}\"")
    );
    assert!(
        hook.as_str()
            .contains("PROMPT_COMMAND='_kvasir_update_otel_repo_resource'")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_shell_profile_skips_missing_hook_under_errexit()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let temp = tempfile::tempdir()?;
    let profile_path = temp.path().join("profile.sh");
    let missing_hook_path = temp.path().join("missing-repo-hook.bash");
    let generated = RepoInjectionShellProfile::generate("", &missing_hook_path)?;
    std::fs::write(&profile_path, generated.as_str())?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
printf 'profile-ok\n'
"#,
        )
        .arg("kvasir-profile-test")
        .arg(&profile_path)
        .output()?;

    assert!(
        output.status.success(),
        "profile source failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "profile-ok\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_shell_profile_skips_directory_hook_path_under_errexit()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let temp = tempfile::tempdir()?;
    let profile_path = temp.path().join("profile.sh");
    let directory_hook_path = temp.path().join("repo-hook.d");
    std::fs::create_dir(&directory_hook_path)?;
    let generated = RepoInjectionShellProfile::generate("", &directory_hook_path)?;
    std::fs::write(&profile_path, generated.as_str())?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
printf 'profile-ok\n'
"#,
        )
        .arg("kvasir-profile-test")
        .arg(&profile_path)
        .output()?;

    assert!(
        output.status.success(),
        "profile source failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "profile-ok\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_updates_resource_attributes_for_repo_and_no_repo()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
printf 'initial=%s\n' "${OTEL_RESOURCE_ATTRIBUTES-}"
cd "$2"
eval "$PROMPT_COMMAND"
printf 'repo=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "initial=repo.name=,repo.path=\nrepo=repo.name=repo,repo.path={}\n",
            fixture.repo_dir.display()
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_preserves_resource_attributes_and_escapes_repo_values()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo,eq=ual")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
export OTEL_RESOURCE_ATTRIBUTES='service.name=kvasir,repo.name=stale,repo.path=/stale,deployment.environment=dev'
. "$1"
printf 'initial=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
cd "$2"
eval "$PROMPT_COMMAND"
printf 'repo=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "initial=service.name=kvasir,deployment.environment=dev,repo.name=,repo.path=\nrepo=service.name=kvasir,deployment.environment=dev,repo.name=repo\\,eq\\=ual,repo.path={}\n",
            escaped_otel_resource_attribute_value(&fixture.repo_dir.display().to_string())
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_preserves_resource_attributes_changed_after_source()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
export OTEL_RESOURCE_ATTRIBUTES='service.name=initial'
. "$1"
export OTEL_RESOURCE_ATTRIBUTES='service.name=after,repo.name=stale,repo.path=/stale,tenant.id=abc'
cd "$2"
eval "$PROMPT_COMMAND"
printf 'repo=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "repo=service.name=after,tenant.id=abc,repo.name=repo,repo.path={}\n",
            fixture.repo_dir.display()
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_discards_whitespace_prefixed_stale_repo_attributes()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
export OTEL_RESOURCE_ATTRIBUTES='service.name=kvasir, repo.name=stale, repo.path=/stale,tenant.id=abc'
. "$1"
cd "$2"
eval "$PROMPT_COMMAND"
printf 'repo=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "repo=service.name=kvasir,tenant.id=abc,repo.name=repo,repo.path={}\n",
            fixture.repo_dir.display()
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_preserves_newline_inside_existing_resource_attribute()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
export OTEL_RESOURCE_ATTRIBUTES=$'service.name=kvasir\n,repo.name=stale,tenant.id=abc'
. "$1"
cd "$2"
eval "$PROMPT_COMMAND"
printf '%s' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "service.name=kvasir\n,tenant.id=abc,repo.name=repo,repo.path={}",
            fixture.repo_dir.display()
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_sanitizes_control_characters_in_repo_values()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo\tname")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
cd "$2"
eval "$PROMPT_COMMAND"
printf 'repo=%s\n' "$OTEL_RESOURCE_ATTRIBUTES"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "repo=repo.name=repo name,repo.path={}\n",
            escaped_otel_resource_attribute_value(&fixture.repo_dir.display().to_string())
        )
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_preserves_prompt_command_array_on_resource()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
PROMPT_COMMAND=('history -a' '_kvasir_update_otel_repo_resource')
. "$1"
declare -p PROMPT_COMMAND
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("[0]=\"history -a\""), "{stdout}");
    assert!(
        stdout.contains("[1]=\"_kvasir_update_otel_repo_resource\""),
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("_kvasir_update_otel_repo_resource").count(),
        1
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_bash_hook_uses_local_preserved_attribute_accumulator()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Bash, "repo-hook.bash")?;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
_kvasir_preserved_otel_resource_attributes='user-state'
_kvasir_without_repo_resource_attributes 'service.name=kvasir' >/dev/null
printf 'scratch=%s\n' "${_kvasir_preserved_otel_resource_attributes-unset}"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "bash hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(String::from_utf8(output.stdout)?, "scratch=user-state\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn repo_injection_zsh_hook_updates_resource_attributes_for_repo_and_no_repo()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let zsh_available = Command::new("zsh")
        .arg("-fc")
        .arg("exit 0")
        .output()
        .is_ok_and(|output| output.status.success());
    if !zsh_available {
        return Ok(());
    }

    let fixture = RepoHookShellFixture::new("repo")?;
    let hook_path = fixture.write_hook(RepoInjectionShell::Zsh, "repo-hook.zsh")?;

    let output = Command::new("zsh")
        .arg("-fc")
        .arg(
            r#"set -e
. "$1"
print -r -- "initial=${OTEL_RESOURCE_ATTRIBUTES-}"
cd "$2"
print -r -- "repo=${OTEL_RESOURCE_ATTRIBUTES-}"
"#,
        )
        .arg("kvasir-repo-hook-test")
        .arg(&hook_path)
        .arg(&fixture.repo_dir)
        .current_dir(&fixture.no_repo_dir)
        .env("PATH", &fixture.path)
        .env("KVASIR_TEST_REPO_PATH", &fixture.repo_dir)
        .output()?;

    assert!(
        output.status.success(),
        "zsh hook failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "initial=repo.name=,repo.path=\nrepo=repo.name=repo,repo.path={}\n",
            fixture.repo_dir.display()
        )
    );
    Ok(())
}

#[test]
fn repo_injection_shell_profile_replaces_only_managed_source_line()
-> Result<(), Box<dyn std::error::Error>> {
    let first = RepoInjectionShellProfile::generate(
        r#"alias ll='ls -la'

# BEGIN KVASIR MANAGED REPO OTEL
. '/Users/oyr/.kvasir/stale-repo-hook.zsh'
# END KVASIR MANAGED REPO OTEL

export EDITOR='vim'
"#,
        Path::new("/Users/oyr/.kvasir/repo-hook.zsh"),
    )?;
    let second = RepoInjectionShellProfile::generate(
        first.as_str(),
        Path::new("/Users/oyr/.kvasir/repo-hook.zsh"),
    )?;

    assert_eq!(first.as_str(), second.as_str());
    assert_eq!(
        second.as_str(),
        r#"alias ll='ls -la'

export EDITOR='vim'

# BEGIN KVASIR MANAGED REPO OTEL
if [ -f '/Users/oyr/.kvasir/repo-hook.zsh' ] && [ -r '/Users/oyr/.kvasir/repo-hook.zsh' ]; then . '/Users/oyr/.kvasir/repo-hook.zsh'; fi
# END KVASIR MANAGED REPO OTEL
"#
    );
    assert!(!second.as_str().contains("stale-repo-hook"));
    Ok(())
}

#[test]
fn copilot_shell_profile_exports_kvasir_otlp_http_env() -> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        "export PATH='/usr/local/bin:$PATH'\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(
        generated.as_str(),
        r#"export PATH='/usr/local/bin:$PATH'

# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4318'
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer test-token'
export OTEL_EXPORTER_OTLP_PROTOCOL='http/protobuf'
export OTEL_LOGS_EXPORTER='otlp'
export OTEL_METRICS_EXPORTER='otlp'
export OTEL_TRACES_EXPORTER='otlp'
# END KVASIR MANAGED COPILOT OTEL
"#
    );
    Ok(())
}

#[test]
fn copilot_shell_profile_quotes_values_without_raw_newlines()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        "",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318/it's\nnext"),
            BearerToken::new("test'token\nline"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(
        generated
            .as_str()
            .contains("export OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4318/it'\\''s\nnext'")
    );
    assert!(
        generated.as_str().contains(
            "export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer test'\\''token\nline'"
        )
    );
    Ok(())
}

#[test]
fn copilot_shell_profile_replaces_only_kvasir_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let first = CopilotShellProfile::generate(
        r#"alias ll='ls -la'

# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_ENDPOINT='http://old.example'
# END KVASIR MANAGED COPILOT OTEL

export EDITOR='vim'
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("first-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let second = CopilotShellProfile::generate(
        first.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;
    let third = CopilotShellProfile::generate(
        second.as_str(),
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4319"),
            BearerToken::new("second-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_eq!(second.as_str(), third.as_str());
    assert_eq!(
        second.as_str(),
        r#"alias ll='ls -la'

export EDITOR='vim'

# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4319'
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer second-token'
export OTEL_EXPORTER_OTLP_PROTOCOL='http/protobuf'
export OTEL_LOGS_EXPORTER='otlp'
export OTEL_METRICS_EXPORTER='otlp'
export OTEL_TRACES_EXPORTER='otlp'
# END KVASIR MANAGED COPILOT OTEL
"#
    );
    assert!(!second.as_str().contains("first-token"));
    assert!(!second.as_str().contains("http://old.example"));
    Ok(())
}

#[test]
fn copilot_shell_profile_preserves_crlf_when_replacing_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        "alias ll='ls -la'\r\n\r\n# BEGIN KVASIR MANAGED COPILOT OTEL\r\nexport OTEL_EXPORTER_OTLP_ENDPOINT='http://old.example'\r\n# END KVASIR MANAGED COPILOT OTEL\r\n\r\nexport EDITOR='vim'\r\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_lone_lf(generated.as_str());
    assert_eq!(
        generated.as_str(),
        "alias ll='ls -la'\r\n\r\nexport EDITOR='vim'\r\n\r\n# BEGIN KVASIR MANAGED COPILOT OTEL\r\nexport OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4318'\r\nexport OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer test-token'\r\nexport OTEL_EXPORTER_OTLP_PROTOCOL='http/protobuf'\r\nexport OTEL_LOGS_EXPORTER='otlp'\r\nexport OTEL_METRICS_EXPORTER='otlp'\r\nexport OTEL_TRACES_EXPORTER='otlp'\r\n# END KVASIR MANAGED COPILOT OTEL\r\n"
    );
    Ok(())
}

#[test]
fn copilot_shell_profile_normalizes_mixed_line_endings_at_replacement_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        "alias ll='ls -la'\r\n\r\n# BEGIN KVASIR MANAGED COPILOT OTEL\nexport OTEL_EXPORTER_OTLP_ENDPOINT='http://old.example'\r\n# END KVASIR MANAGED COPILOT OTEL\r\n\nexport EDITOR='vim'\r\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_lone_lf(generated.as_str());
    assert!(
        generated
            .as_str()
            .contains("alias ll='ls -la'\r\n\r\nexport EDITOR='vim'\r\n\r\n# BEGIN")
    );
    assert!(!generated.as_str().contains("\r\n\r\n\r\nexport EDITOR"));
    Ok(())
}

#[test]
fn copilot_shell_profile_prefers_unmanaged_lf_over_removed_crlf_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        "alias ll='ls -la'\n\n# BEGIN KVASIR MANAGED COPILOT OTEL\r\nexport OTEL_EXPORTER_OTLP_ENDPOINT='http://old.example'\r\nexport OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer old-token'\r\nexport OTEL_LOGS_EXPORTER='otlp'\r\nexport OTEL_TRACES_EXPORTER='otlp'\r\n# END KVASIR MANAGED COPILOT OTEL\r\n\nexport EDITOR='vim'\n",
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert_no_cr(generated.as_str());
    assert!(
        generated
            .as_str()
            .contains("alias ll='ls -la'\n\nexport EDITOR='vim'")
    );
    assert!(
        generated
            .as_str()
            .contains("export EDITOR='vim'\n\n# BEGIN KVASIR MANAGED COPILOT OTEL\n")
    );
    Ok(())
}

#[test]
fn managed_block_replacement_ignores_marker_text_inside_unmanaged_lines()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        r#"echo '# BEGIN KVASIR MANAGED COPILOT OTEL is documentation'

# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_ENDPOINT='http://old.example'
# END KVASIR MANAGED COPILOT OTEL

echo '# END KVASIR MANAGED COPILOT OTEL is documentation'
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(
        generated
            .as_str()
            .contains("echo '# BEGIN KVASIR MANAGED COPILOT OTEL is documentation'")
    );
    assert!(
        generated
            .as_str()
            .contains("echo '# END KVASIR MANAGED COPILOT OTEL is documentation'")
    );
    assert!(!generated.as_str().contains("http://old.example"));
    Ok(())
}

#[test]
fn copilot_shell_profile_preserves_valid_codex_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CopilotShellProfile::generate(
        r#"# BEGIN KVASIR MANAGED CODEX OTEL
[otel]
exporter = { otlp-http = { endpoint = "http://old.example/v1/logs", protocol = "binary", headers = { "Authorization" = "Bearer codex-token" } } }
# END KVASIR MANAGED CODEX OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("copilot-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(
        generated
            .as_str()
            .contains("# BEGIN KVASIR MANAGED CODEX OTEL")
    );
    assert!(generated.as_str().contains("Bearer codex-token"));
    assert!(
        generated
            .as_str()
            .contains("# BEGIN KVASIR MANAGED COPILOT OTEL")
    );
    assert!(
        generated
            .as_str()
            .contains("Authorization=Bearer copilot-token")
    );
    Ok(())
}

#[test]
fn codex_config_toml_preserves_valid_copilot_managed_block()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = CodexConfigToml::generate(
        r#"# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer copilot-token'
# END KVASIR MANAGED COPILOT OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("codex-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )?;

    assert!(
        generated
            .as_str()
            .contains("# BEGIN KVASIR MANAGED COPILOT OTEL")
    );
    assert!(generated.as_str().contains("Bearer copilot-token"));
    assert!(
        generated
            .as_str()
            .contains("# BEGIN KVASIR MANAGED CODEX OTEL")
    );
    assert!(generated.as_str().contains("Bearer codex-token"));
    Ok(())
}

#[test]
fn copilot_shell_profile_rejects_missing_managed_block_end_marker() {
    let err = CopilotShellProfile::generate(
        r#"# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("malformed managed block must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn copilot_shell_profile_rejects_nested_managed_block_start_marker() {
    let err = CopilotShellProfile::generate(
        r#"# BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
# BEGIN KVASIR MANAGED COPILOT OTEL
# END KVASIR MANAGED COPILOT OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("nested managed block must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn copilot_shell_profile_rejects_orphan_managed_block_end_marker() {
    let err = CopilotShellProfile::generate(
        r#"export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
# END KVASIR MANAGED COPILOT OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("orphan managed end marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn copilot_shell_profile_rejects_whitespace_corrupted_managed_markers() {
    let err = CopilotShellProfile::generate(
        r#"  # BEGIN KVASIR MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
# END KVASIR MANAGED COPILOT OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("whitespace-corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn copilot_shell_profile_rejects_internally_corrupted_managed_markers() {
    let err = CopilotShellProfile::generate(
        r#"# BEGIN KVASIR  MANAGED COPILOT OTEL
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
# END KVASIR MANAGED COPILOT  OTEL
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("internally corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
}

#[test]
fn copilot_shell_profile_rejects_suffix_corrupted_managed_markers() {
    let err = CopilotShellProfile::generate(
        r#"# BEGIN KVASIR MANAGED COPILOT OTEL extra
export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer stale-token'
# END KVASIR MANAGED COPILOT OTEL extra
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("fresh-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("suffix-corrupted managed marker must not preserve stale tokens");

    assert!(matches!(
        err,
        kvasir_core::SetupError::MalformedManagedBlock
    ));
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

#[cfg(unix)]
struct RepoHookShellFixture {
    temp: tempfile::TempDir,
    repo_dir: PathBuf,
    no_repo_dir: PathBuf,
    path: std::ffi::OsString,
}

#[cfg(unix)]
impl RepoHookShellFixture {
    fn new(repo_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let bin_dir = temp.path().join("bin");
        let repo_dir = temp.path().join(repo_name);
        let no_repo_dir = temp.path().join("outside");
        fs::create_dir(&bin_dir)?;
        fs::create_dir(&repo_dir)?;
        fs::create_dir(&no_repo_dir)?;

        let git_path = bin_dir.join("git");
        fs::write(
            &git_path,
            r#"#!/bin/sh
if [ "$1" = "rev-parse" ] && [ "$2" = "--show-toplevel" ]; then
    case "$PWD" in
        "$KVASIR_TEST_REPO_PATH"|"$KVASIR_TEST_REPO_PATH"/*)
            printf '%s\n' "$KVASIR_TEST_REPO_PATH"
            exit 0
            ;;
    esac
fi
exit 128
"#,
        )?;
        let mut permissions = fs::metadata(&git_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_path, permissions)?;

        let mut paths = vec![bin_dir];
        if let Some(existing_path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing_path));
        }
        let path = std::env::join_paths(paths)?;

        Ok(Self {
            temp,
            repo_dir,
            no_repo_dir,
            path,
        })
    }

    fn write_hook(
        &self,
        shell: RepoInjectionShell,
        file_name: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let hook_path = self.temp.path().join(file_name);
        let hook = RepoInjectionShellHook::generate(shell);
        std::fs::write(&hook_path, hook.as_str())?;
        Ok(hook_path)
    }
}

fn escaped_otel_resource_attribute_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace('=', "\\=")
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn assert_no_lone_lf(value: &str) {
    for (index, byte) in value.as_bytes().iter().enumerate() {
        if *byte == b'\n' {
            assert!(
                index > 0 && value.as_bytes()[index - 1] == b'\r',
                "found LF without preceding CR in {value:?}"
            );
        }
    }
}

fn assert_no_cr(value: &str) {
    assert!(!value.as_bytes().contains(&b'\r'), "found CR in {value:?}");
}
