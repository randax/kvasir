use serde::{Deserialize, Serialize};

use crate::rpc::{ModelName, TimestampMillis};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIdentity {
    pub name: RepoName,
    pub path: RepoPath,
}

impl RepoIdentity {
    pub fn new(name: RepoName, path: RepoPath) -> Self {
        Self { name, path }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenMeasure {
    Input,
    Output,
    Cache,
}

impl TokenMeasure {
    pub fn from_attribute(value: &str) -> Option<Self> {
        match value {
            "input" | "input_tokens" => Some(Self::Input),
            "output" | "output_tokens" => Some(Self::Output),
            "cache" | "cache_tokens" | "cache_read" | "cache_creation" => Some(Self::Cache),
            _ => None,
        }
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Cache => "cache",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount(u64);

impl TokenCount {
    pub fn new(value: u64) -> Self {
        Self::try_new(value).expect("token count must fit SQLite integer storage")
    }

    pub fn try_new(value: u64) -> Option<Self> {
        i64::try_from(value).ok().map(|_| Self(value))
    }

    pub fn value(self) -> u64 {
        self.0
    }

    pub fn storage_value(self) -> i64 {
        i64::try_from(self.0).expect("token count is validated before storage")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub occurred_at: TimestampMillis,
    pub counter_start: TimestampMillis,
    pub repo: RepoIdentity,
    pub model: ModelName,
    pub measure: TokenMeasure,
    pub token_count: TokenCount,
}

impl TokenUsageRecord {
    pub fn new(
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoIdentity,
        model: ModelName,
        measure: TokenMeasure,
        token_count: TokenCount,
    ) -> Self {
        Self {
            occurred_at,
            counter_start,
            repo,
            model,
            measure,
            token_count,
        }
    }
}
