use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use kvasir_core::{
    BearerToken, ClaudeCodeSettings, CodexConfigToml, CopilotShellProfile, KvasirEndpoint,
    OpenCodeSetup, RawBodyDirectory, SetupConfig,
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
fn codex_config_toml_rejects_unmanaged_otel_exporter_assignment() {
    let err = CodexConfigToml::generate(
        r#"[otel]
environment = "dev"
exporter = "none"
"#,
        &SetupConfig::new(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            BearerToken::new("test-token"),
            RawBodyDirectory::new(PathBuf::from("/tmp/kvasir/raw-bodies")),
        ),
    )
    .expect_err("unmanaged Codex OTEL keys must not be silently removed");

    assert!(matches!(
        err,
        kvasir_core::SetupError::ConflictingCodexOtelKeys
    ));
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
