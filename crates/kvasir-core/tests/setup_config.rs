use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use kvasir_core::{
    BearerToken, ClaudeCodeSettings, CodexConfigToml, CopilotShellProfile, KvasirEndpoint,
    RawBodyDirectory, SetupConfig,
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
exporter = "none"

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
    assert!(!generated.as_str().contains("exporter = \"none\""));
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

    assert!(generated.as_str().contains(
        "export OTEL_EXPORTER_OTLP_ENDPOINT='http://127.0.0.1:4318/it'\\''s'$'\\n''next'"
    ));
    assert!(generated.as_str().contains(
        "export OTEL_EXPORTER_OTLP_HEADERS='Authorization=Bearer test'\\''token'$'\\n''line'"
    ));
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
