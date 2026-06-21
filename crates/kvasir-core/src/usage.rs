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
    pub name: Option<RepoName>,
    pub path: Option<RepoPath>,
}

impl RepoIdentity {
    pub fn new(name: RepoName, path: RepoPath) -> Self {
        Self {
            name: Some(name),
            path: Some(path),
        }
    }

    pub fn from_parts(name: Option<RepoName>, path: Option<RepoPath>) -> Option<Self> {
        if name.is_none() && path.is_none() {
            None
        } else {
            Some(Self { name, path })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "identity", rename_all = "snake_case")]
pub enum RepoBucket {
    NoRepo,
    Repo(RepoIdentity),
}

impl RepoBucket {
    pub fn repo(identity: RepoIdentity) -> Self {
        Self::Repo(identity)
    }

    pub fn no_repo() -> Self {
        Self::NoRepo
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
    pub repo: RepoBucket,
    pub model: ModelName,
    pub measure: TokenMeasure,
    pub token_count: TokenCount,
}

const COST_NANOS_PER_USD: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsd {
    nanos: u64,
}

impl CostUsd {
    pub fn from_nanos(nanos: u64) -> Option<Self> {
        i64::try_from(nanos).ok().map(|_| Self { nanos })
    }

    pub fn from_whole_usd(value: u64) -> Option<Self> {
        value
            .checked_mul(COST_NANOS_PER_USD)
            .and_then(Self::from_nanos)
    }

    pub fn from_decimal_str(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.starts_with('-') {
            return None;
        }
        let (mantissa, exponent) = split_decimal_exponent(value)?;
        let mut digits = String::with_capacity(mantissa.len());
        let mut fractional_digits = 0_i32;
        let mut saw_decimal = false;
        for character in mantissa.chars() {
            match character {
                '.' if saw_decimal => return None,
                '.' => saw_decimal = true,
                digit if digit.is_ascii_digit() => {
                    digits.push(digit);
                    if saw_decimal {
                        fractional_digits += 1;
                    }
                }
                _ => return None,
            }
        }
        if digits.is_empty() {
            return None;
        }
        let nanos_scale = 9_i32 + exponent - fractional_digits;
        if nanos_scale < 0 {
            return None;
        }
        let mut nanos = digits.parse::<u64>().ok()?;
        for _ in 0..nanos_scale {
            nanos = nanos.checked_mul(10)?;
        }
        Self::from_nanos(nanos)
    }

    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        let nanos = (value * COST_NANOS_PER_USD as f64).round();
        if nanos < 0.0 || nanos > i64::MAX as f64 {
            return None;
        }
        Self::from_nanos(nanos as u64)
    }

    pub fn from_storage_value(value: i64) -> Option<Self> {
        u64::try_from(value).ok().and_then(Self::from_nanos)
    }

    pub fn as_nanos(self) -> u64 {
        self.nanos
    }

    pub fn storage_value(self) -> i64 {
        i64::try_from(self.nanos).expect("cost is validated before storage")
    }
}

fn split_decimal_exponent(value: &str) -> Option<(&str, i32)> {
    let Some((mantissa, exponent)) = value.split_once(['e', 'E']) else {
        return Some((value, 0));
    };
    if mantissa.is_empty() || exponent.is_empty() {
        return None;
    }
    Some((mantissa, exponent.parse::<i32>().ok()?))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostUsageRecord {
    pub occurred_at: TimestampMillis,
    pub counter_start: TimestampMillis,
    pub repo: RepoBucket,
    pub model: ModelName,
    pub cost_usd: CostUsd,
}

impl CostUsageRecord {
    pub fn new(
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
        model: ModelName,
        cost_usd: CostUsd,
    ) -> Self {
        Self {
            occurred_at,
            counter_start,
            repo,
            model,
            cost_usd,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageRecords {
    pub token_usage: Vec<TokenUsageRecord>,
    pub cost_usage: Vec<CostUsageRecord>,
}

impl UsageRecords {
    pub fn from_token_usage(token_usage: Vec<TokenUsageRecord>) -> Self {
        Self {
            token_usage,
            cost_usage: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.token_usage.is_empty() && self.cost_usage.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.token_usage.extend(other.token_usage);
        self.cost_usage.extend(other.cost_usage);
    }
}

impl TokenUsageRecord {
    pub fn new(
        occurred_at: TimestampMillis,
        counter_start: TimestampMillis,
        repo: RepoBucket,
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
