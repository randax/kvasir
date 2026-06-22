use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::rpc::BearerToken;

const MANAGED_BLOCK_KEY: &str = "kvasirManaged";

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
