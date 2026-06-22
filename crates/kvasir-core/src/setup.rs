use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::rpc::BearerToken;

const MANAGED_BLOCK_KEY: &str = "kvasirManaged";
const CODEX_OTEL_BLOCK_START: &str = "# BEGIN KVASIR MANAGED CODEX OTEL";
const CODEX_OTEL_BLOCK_END: &str = "# END KVASIR MANAGED CODEX OTEL";
const COPILOT_OTEL_BLOCK_START: &str = "# BEGIN KVASIR MANAGED COPILOT OTEL";
const COPILOT_OTEL_BLOCK_END: &str = "# END KVASIR MANAGED COPILOT OTEL";

const MANAGED_ENV_KEYS: &[&str] = &[
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
    "OTEL_TRACES_EXPORTER",
];

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("Claude Code settings must be a JSON object")]
    SettingsNotObject,
    #[error("Claude Code settings env field must be a JSON object")]
    EnvNotObject,
    #[error("Claude Code settings JSON is invalid")]
    InvalidSettingsJson(#[from] serde_json::Error),
    #[error("kvasir managed block is malformed")]
    MalformedManagedBlock,
    #[error("Codex [otel] already contains unmanaged keys managed by kvasir")]
    ConflictingCodexOtelKeys,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvasirEndpoint(String);

impl KvasirEndpoint {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn from_otlp_addr(addr: SocketAddr) -> Self {
        Self(format!("http://{addr}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawBodyDirectory(PathBuf);

impl RawBodyDirectory {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SetupConfig {
    endpoint: KvasirEndpoint,
    bearer_token: BearerToken,
    raw_body_directory: RawBodyDirectory,
}

impl std::fmt::Debug for SetupConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SetupConfig")
            .field("endpoint", &self.endpoint)
            .field("bearer_token", &self.bearer_token)
            .field("raw_body_directory", &self.raw_body_directory)
            .finish()
    }
}

impl SetupConfig {
    pub fn new(
        endpoint: KvasirEndpoint,
        bearer_token: BearerToken,
        raw_body_directory: RawBodyDirectory,
    ) -> Self {
        Self {
            endpoint,
            bearer_token,
            raw_body_directory,
        }
    }

    pub fn endpoint(&self) -> &KvasirEndpoint {
        &self.endpoint
    }

    pub fn bearer_token(&self) -> &BearerToken {
        &self.bearer_token
    }

    pub fn raw_body_directory(&self) -> &RawBodyDirectory {
        &self.raw_body_directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeSettings {
    json: String,
}

impl ClaudeCodeSettings {
    pub fn generate(existing_settings: &str, config: &SetupConfig) -> Result<Self, SetupError> {
        let mut root = parse_settings_root(existing_settings)?;
        let previously_managed_env_keys = managed_env_keys_from_root(&root);
        let env = env_object(&mut root)?;

        for key in previously_managed_env_keys {
            env.remove(&key);
        }
        for key in MANAGED_ENV_KEYS {
            env.remove(*key);
        }
        for (key, value) in managed_env_values(config) {
            env.insert(key.to_owned(), Value::String(value));
        }
        root.insert(
            MANAGED_BLOCK_KEY.to_owned(),
            managed_block_value(MANAGED_ENV_KEYS),
        );

        let json = serde_json::to_string_pretty(&Value::Object(root))?;
        Ok(Self { json })
    }

    pub fn as_str(&self) -> &str {
        &self.json
    }

    pub fn managed_env_keys(&self) -> &'static [&'static str] {
        MANAGED_ENV_KEYS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexConfigToml {
    toml: String,
}

impl CodexConfigToml {
    pub fn generate(existing_config: &str, config: &SetupConfig) -> Result<Self, SetupError> {
        let unmanaged_config = remove_managed_block(
            existing_config,
            CODEX_OTEL_BLOCK_START,
            CODEX_OTEL_BLOCK_END,
        )?;
        let line_ending = preferred_line_ending(&unmanaged_config, existing_config);
        let toml = insert_codex_otel_block(&unmanaged_config, config, line_ending)?;
        Ok(Self { toml })
    }

    pub fn as_str(&self) -> &str {
        &self.toml
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotShellProfile {
    shell: String,
}

impl CopilotShellProfile {
    pub fn generate(existing_profile: &str, config: &SetupConfig) -> Result<Self, SetupError> {
        let unmanaged_profile = remove_managed_block(
            existing_profile,
            COPILOT_OTEL_BLOCK_START,
            COPILOT_OTEL_BLOCK_END,
        )?;
        let line_ending = preferred_line_ending(&unmanaged_profile, existing_profile);
        let mut shell = unmanaged_profile.trim_end().to_owned();
        if !shell.is_empty() {
            shell.push_str(line_ending);
            shell.push_str(line_ending);
        }
        shell.push_str(&copilot_otel_block(config, line_ending));
        Ok(Self { shell })
    }

    pub fn as_str(&self) -> &str {
        &self.shell
    }
}

fn managed_env_keys_from_root(root: &Map<String, Value>) -> Vec<String> {
    root.get(MANAGED_BLOCK_KEY)
        .and_then(|value| value.get("env"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn copilot_otel_block(config: &SetupConfig, line_ending: &str) -> String {
    format!(
        "{COPILOT_OTEL_BLOCK_START}{line_ending}\
         export OTEL_EXPORTER_OTLP_ENDPOINT={}\
         {line_ending}export OTEL_EXPORTER_OTLP_HEADERS={}\
         {line_ending}export OTEL_EXPORTER_OTLP_PROTOCOL='http/protobuf'\
         {line_ending}export OTEL_LOGS_EXPORTER='otlp'\
         {line_ending}export OTEL_METRICS_EXPORTER='otlp'\
         {line_ending}export OTEL_TRACES_EXPORTER='otlp'\
         {line_ending}{COPILOT_OTEL_BLOCK_END}{line_ending}",
        shell_single_quote(config.endpoint.as_str()),
        shell_single_quote(&format!(
            "Authorization={}",
            config.bearer_token.authorization_header()
        )),
    )
}

fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        match character {
            '\'' => quoted.push_str("'\\''"),
            _ => quoted.push(character),
        }
    }
    quoted.push('\'');
    quoted
}

fn codex_otel_block(config: &SetupConfig, line_ending: &str) -> String {
    format!(
        "{CODEX_OTEL_BLOCK_START}{line_ending}\
         [otel]{line_ending}\
         {}\
         {CODEX_OTEL_BLOCK_END}{line_ending}",
        codex_otel_assignments(config, line_ending),
    )
}

fn codex_otel_assignments(config: &SetupConfig, line_ending: &str) -> String {
    format!(
        "log_user_prompt = true{line_ending}\
         exporter = {{ otlp-http = {{ endpoint = \"{}\", protocol = \"binary\", headers = {{ \"Authorization\" = \"{}\" }} }} }}{line_ending}\
         trace_exporter = {{ otlp-http = {{ endpoint = \"{}\", protocol = \"binary\", headers = {{ \"Authorization\" = \"{}\" }} }} }}{line_ending}\
         metrics_exporter = {{ otlp-http = {{ endpoint = \"{}\", protocol = \"binary\", headers = {{ \"Authorization\" = \"{}\" }} }} }}{line_ending}",
        toml_basic_string_content(&otlp_logs_endpoint(config.endpoint.as_str())),
        toml_basic_string_content(&config.bearer_token.authorization_header()),
        toml_basic_string_content(&otlp_signal_endpoint(config.endpoint.as_str(), "traces")),
        toml_basic_string_content(&config.bearer_token.authorization_header()),
        toml_basic_string_content(&otlp_signal_endpoint(config.endpoint.as_str(), "metrics")),
        toml_basic_string_content(&config.bearer_token.authorization_header()),
    )
}

fn otlp_logs_endpoint(endpoint: &str) -> String {
    otlp_signal_endpoint(endpoint, "logs")
}

fn otlp_signal_endpoint(endpoint: &str, signal: &str) -> String {
    format!("{}/v1/{signal}", endpoint.trim_end_matches('/'))
}

fn insert_codex_otel_block(
    existing: &str,
    config: &SetupConfig,
    line_ending: &str,
) -> Result<String, SetupError> {
    let mut output = String::with_capacity(existing.len() + 512);
    let mut inserted = false;
    let mut inside_otel_table = false;

    for line in existing.split_inclusive('\n') {
        let trimmed_line = line.trim_end_matches(['\r', '\n']).trim();
        let table_header = toml_table_header_name(trimmed_line);
        if table_header == Some("otel") {
            inside_otel_table = true;
        } else if table_header.is_some() {
            inside_otel_table = false;
        }

        if inside_otel_table && inserted && is_codex_managed_otel_assignment(line) {
            return Err(SetupError::ConflictingCodexOtelKeys);
        }

        push_line_with_ending(&mut output, line, line_ending);
        if !inserted && table_header == Some("otel") {
            ensure_line_break(&mut output, line_ending);
            output.push_str(CODEX_OTEL_BLOCK_START);
            output.push_str(line_ending);
            output.push_str(&codex_otel_assignments(config, line_ending));
            output.push_str(CODEX_OTEL_BLOCK_END);
            output.push_str(line_ending);
            inserted = true;
        }
    }

    if inserted {
        let mut toml = output.trim_end().to_owned();
        toml.push_str(line_ending);
        return Ok(toml);
    }

    let mut toml = existing.trim_end().to_owned();
    if !toml.is_empty() {
        toml.push_str(line_ending);
        toml.push_str(line_ending);
    }
    toml.push_str(&codex_otel_block(config, line_ending));
    Ok(toml)
}

fn is_codex_managed_otel_assignment(line: &str) -> bool {
    [
        "log_user_prompt",
        "exporter",
        "trace_exporter",
        "metrics_exporter",
    ]
    .into_iter()
    .any(|key| is_assignment_for_key(line, key))
}

fn is_assignment_for_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix(key) else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

fn toml_table_header_name(line: &str) -> Option<&str> {
    let without_comment = line
        .split_once('#')
        .map_or(line, |(before, _)| before)
        .trim();
    if !without_comment.starts_with('[') || !without_comment.ends_with(']') {
        return None;
    }

    let name = without_comment
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

fn remove_managed_block(
    existing: &str,
    start_marker: &str,
    end_marker: &str,
) -> Result<String, SetupError> {
    let mut output = String::with_capacity(existing.len());
    let mut inside_managed_block = false;
    let mut removed_block = false;
    let mut skip_one_boundary_blank_line = false;

    for line in existing.split_inclusive('\n') {
        let marker_candidate = line.trim_end_matches(['\r', '\n']);
        let trimmed_marker_candidate = marker_candidate.trim();
        let is_start_marker = marker_candidate == start_marker;
        let is_end_marker = marker_candidate == end_marker;
        let is_corrupted_marker = !is_start_marker
            && !is_end_marker
            && is_kvasir_managed_marker_like_comment(trimmed_marker_candidate, start_marker);

        if is_corrupted_marker {
            return Err(SetupError::MalformedManagedBlock);
        }

        if skip_one_boundary_blank_line {
            skip_one_boundary_blank_line = false;
            if marker_candidate.trim().is_empty() && ends_with_blank_line_pair(&output) {
                continue;
            }
        }

        if inside_managed_block {
            if is_start_marker {
                return Err(SetupError::MalformedManagedBlock);
            }
            if is_end_marker {
                inside_managed_block = false;
                removed_block = true;
                skip_one_boundary_blank_line = true;
            }
            continue;
        }

        if is_end_marker {
            return Err(SetupError::MalformedManagedBlock);
        }

        if is_start_marker {
            inside_managed_block = true;
            continue;
        }

        output.push_str(line);
    }

    if inside_managed_block {
        return Err(SetupError::MalformedManagedBlock);
    }

    if removed_block {
        trim_one_blank_line_at_removal_boundary(&mut output);
    }

    Ok(output)
}

fn toml_basic_string_content(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\u{08}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn is_kvasir_managed_marker_like_comment(line: &str, start_marker: &str) -> bool {
    if !line.starts_with("# BEGIN KVASIR") && !line.starts_with("# END KVASIR") {
        return false;
    }

    let words = line.split_whitespace().collect::<Vec<_>>();
    let Some(identifier) = managed_marker_identifier(start_marker) else {
        return false;
    };

    words.len() >= 4
        && words[0] == "#"
        && matches!(words[1], "BEGIN" | "END")
        && words[2] == "KVASIR"
        && words.iter().skip(3).any(|word| *word == identifier)
        && words.iter().skip(3).any(|word| *word == "OTEL")
}

fn managed_marker_identifier(marker: &str) -> Option<&str> {
    marker
        .split_whitespace()
        .find(|word| matches!(*word, "CODEX" | "COPILOT"))
}

fn dominant_line_ending(text: &str) -> &'static str {
    let crlf_count = text
        .as_bytes()
        .windows(2)
        .filter(|bytes| *bytes == b"\r\n")
        .count();
    let lf_count = text
        .as_bytes()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let lone_lf_count = lf_count.saturating_sub(crlf_count);

    if crlf_count > lone_lf_count {
        "\r\n"
    } else {
        "\n"
    }
}

fn preferred_line_ending(unmanaged: &str, original: &str) -> &'static str {
    if unmanaged.contains('\n') {
        dominant_line_ending(unmanaged)
    } else {
        dominant_line_ending(original)
    }
}

fn ends_with_blank_line_pair(output: &str) -> bool {
    let Some(last_line_ending_len) = trailing_line_ending_len(output) else {
        return false;
    };
    trailing_line_ending_len(&output[..output.len() - last_line_ending_len]).is_some()
}

fn push_line_with_ending(output: &mut String, line: &str, line_ending: &str) {
    let Some(without_lf) = line.strip_suffix('\n') else {
        output.push_str(line);
        return;
    };

    output.push_str(without_lf.strip_suffix('\r').unwrap_or(without_lf));
    output.push_str(line_ending);
}

fn trim_one_blank_line_at_removal_boundary(output: &mut String) {
    if ends_with_blank_line_pair(output) {
        let line_ending_len = trailing_line_ending_len(output).expect("line ending checked");
        output.truncate(output.len() - line_ending_len);
    }
}

fn trailing_line_ending_len(text: &str) -> Option<usize> {
    if text.ends_with("\r\n") {
        Some(2)
    } else if text.ends_with('\n') {
        Some(1)
    } else {
        None
    }
}

fn ensure_line_break(output: &mut String, line_ending: &str) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push_str(line_ending);
    }
}

fn parse_settings_root(existing_settings: &str) -> Result<Map<String, Value>, SetupError> {
    if existing_settings.trim().is_empty() {
        return Ok(Map::new());
    }

    match serde_json::from_str(existing_settings)? {
        Value::Object(root) => Ok(root),
        _ => Err(SetupError::SettingsNotObject),
    }
}

fn env_object(root: &mut Map<String, Value>) -> Result<&mut Map<String, Value>, SetupError> {
    if !root.contains_key("env") {
        root.insert("env".to_owned(), Value::Object(Map::new()));
    }

    match root.get_mut("env") {
        Some(Value::Object(env)) => Ok(env),
        _ => Err(SetupError::EnvNotObject),
    }
}

fn managed_block_value(env_keys: &[&str]) -> Value {
    let env = env_keys
        .iter()
        .map(|key| Value::String((*key).to_owned()))
        .collect();
    Value::Object(Map::from_iter([("env".to_owned(), Value::Array(env))]))
}

fn managed_env_values(config: &SetupConfig) -> Vec<(&'static str, String)> {
    vec![
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".to_owned()),
        ("CLAUDE_CODE_ENHANCED_TELEMETRY_BETA", "1".to_owned()),
        ("CLAUDE_CODE_ENABLE_TRACE_BETA", "1".to_owned()),
        ("CLAUDE_CODE_ENABLE_CONTENT_GATES", "1".to_owned()),
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            config.endpoint.as_str().to_owned(),
        ),
        (
            "OTEL_EXPORTER_OTLP_HEADERS",
            format!(
                "Authorization={}",
                config.bearer_token.authorization_header()
            ),
        ),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf".to_owned()),
        ("OTEL_LOGS_EXPORTER", "otlp".to_owned()),
        (
            "OTEL_LOG_RAW_API_BODIES",
            format!("file:{}", config.raw_body_directory.as_path().display()),
        ),
        ("OTEL_LOG_TOOL_CONTENT", "1".to_owned()),
        ("OTEL_LOG_TOOL_DETAILS", "1".to_owned()),
        ("OTEL_LOG_USER_PROMPTS", "1".to_owned()),
        ("OTEL_METRICS_EXPORTER", "otlp".to_owned()),
        ("OTEL_TRACES_EXPORTER", "otlp".to_owned()),
    ]
}
