use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{
    LogRecord, ResourceLogs as OtlpResourceLogs, ScopeLogs,
};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use serde_json::Value;

use crate::rpc::{HarnessName, ModelName, TimestampMillis, ToolName};
use crate::usage::{
    CostUsageRecord, CostUsd, RepoBucket, RepoIdentity, RepoName, RepoPath, TokenCount,
    TokenMeasure, TokenUsageRecord, ToolCallEventKey, ToolCallRecord, UsageRecords,
};

#[derive(Debug, thiserror::Error)]
pub enum OtlpError {
    #[error("invalid otlp protobuf")]
    InvalidProtobuf(#[from] prost::DecodeError),
    #[error("invalid otlp json")]
    InvalidJson(#[from] serde_json::Error),
    #[error("otlp number is out of range")]
    NumberOutOfRange,
    #[error("otlp token datapoint is missing model")]
    MissingModel,
    #[error("otlp token datapoint is missing or has invalid token measure")]
    InvalidMeasure,
    #[error("otlp token datapoint is missing token count")]
    MissingTokenCount,
    #[error("otlp cost datapoint is missing cost")]
    MissingCost,
    #[error("otlp cost datapoint has invalid cost")]
    InvalidCost,
    #[error("otlp token datapoint has non-integral token count")]
    NonIntegralTokenCount,
    #[error("otlp token datapoint is missing timestamp")]
    MissingTimestamp,
    #[error("otlp cumulative datapoint is missing counter start timestamp")]
    MissingCounterStart,
    #[error("otlp payload is missing resource metrics")]
    MissingResourceMetrics,
    #[error("otlp payload is missing scope metrics")]
    MissingScopeMetrics,
    #[error("otlp payload is missing metrics")]
    MissingMetrics,
    #[error("otlp token metric is missing datapoints")]
    MissingDataPoints,
    #[error("otlp token metric has invalid metric kind")]
    InvalidMetricKind,
    #[error("otlp payload contains no token usage metrics")]
    NoTokenUsageMetrics,
    #[error("otlp payload is missing resource logs")]
    MissingResourceLogs,
    #[error("otlp payload is missing scope logs")]
    MissingScopeLogs,
    #[error("otlp payload is missing log records")]
    MissingLogRecords,
    #[error("otlp log record is missing tool name")]
    MissingToolName,
    #[error("otlp log record has invalid tool name")]
    InvalidToolName,
    #[error("otlp payload contains no tool call logs")]
    NoToolCallLogs,
}

pub fn parse_otlp_protobuf_usage_metrics(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let request = ExportMetricsServiceRequest::decode(bytes)?;
    if request.resource_metrics.is_empty() {
        return Err(OtlpError::MissingResourceMetrics);
    }
    let mut records = UsageRecords::default();
    for resource_metrics in request.resource_metrics {
        records.extend(records_from_resource_metrics(resource_metrics)?);
    }
    if records.is_empty() {
        return Err(OtlpError::NoTokenUsageMetrics);
    }
    Ok(records)
}

pub fn parse_otlp_json_usage_metrics(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let payload: Value = serde_json::from_slice(bytes)?;
    let mut records = UsageRecords::default();
    let resource_metrics = payload
        .get("resourceMetrics")
        .and_then(Value::as_array)
        .ok_or(OtlpError::MissingResourceMetrics)?;
    for resource_metrics in resource_metrics {
        let resource_attributes = resource_metrics
            .get("resource")
            .and_then(|resource| resource.get("attributes"))
            .and_then(Value::as_array);
        let repo = repo_from_json_attributes(resource_attributes);
        let scope_metrics = resource_metrics
            .get("scopeMetrics")
            .and_then(Value::as_array)
            .ok_or(OtlpError::MissingScopeMetrics)?;
        for scope_metric in scope_metrics {
            let metrics = scope_metric
                .get("metrics")
                .and_then(Value::as_array)
                .ok_or(OtlpError::MissingMetrics)?;
            for metric in metrics {
                match metric.get("name").and_then(Value::as_str) {
                    Some("token.usage") => {
                        let data_points = json_data_points(metric)?;
                        for data_point in data_points {
                            records
                                .token_usage
                                .push(json_token_record(data_point, repo.clone())?);
                        }
                    }
                    Some("cost.usage") => {
                        let data_points = json_data_points(metric)?;
                        for data_point in data_points {
                            records
                                .cost_usage
                                .push(json_cost_record(data_point, repo.clone())?);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if records.is_empty() {
        return Err(OtlpError::NoTokenUsageMetrics);
    }
    Ok(records)
}

pub fn parse_otlp_protobuf_usage_logs(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let request = ExportLogsServiceRequest::decode(bytes)?;
    if request.resource_logs.is_empty() {
        return Ok(UsageRecords::default());
    }
    let mut records = UsageRecords::default();
    for resource_logs in request.resource_logs {
        records.extend(records_from_resource_logs(resource_logs)?);
    }
    Ok(records)
}

pub fn parse_otlp_json_usage_logs(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let payload: Value = serde_json::from_slice(bytes)?;
    let mut records = UsageRecords::default();
    let resource_logs = payload
        .get("resourceLogs")
        .and_then(Value::as_array)
        .ok_or(OtlpError::MissingResourceLogs)?;
    for resource_logs in resource_logs {
        let resource_attributes = resource_logs
            .get("resource")
            .and_then(|resource| resource.get("attributes"))
            .and_then(Value::as_array);
        let repo = repo_from_json_attributes(resource_attributes);
        let scope_logs = resource_logs
            .get("scopeLogs")
            .and_then(Value::as_array)
            .ok_or(OtlpError::MissingScopeLogs)?;
        for scope_logs in scope_logs {
            let log_records = scope_logs
                .get("logRecords")
                .and_then(Value::as_array)
                .ok_or(OtlpError::MissingLogRecords)?;
            for log_record in log_records {
                if let Some(record) = json_tool_call_record(log_record, repo.clone())? {
                    records.tool_calls.push(record);
                }
            }
        }
    }
    Ok(records)
}

fn records_from_resource_metrics(
    resource_metrics: ResourceMetrics,
) -> Result<UsageRecords, OtlpError> {
    let repo = resource_metrics
        .resource
        .as_ref()
        .map(|resource| repo_from_proto_attributes(&resource.attributes))
        .unwrap_or_else(RepoBucket::no_repo);

    if resource_metrics.scope_metrics.is_empty() {
        return Err(OtlpError::MissingScopeMetrics);
    }
    let mut records = UsageRecords::default();
    for scope_metrics in resource_metrics.scope_metrics {
        records.extend(records_from_scope_metrics(scope_metrics, repo.clone())?);
    }
    Ok(records)
}

fn records_from_scope_metrics(
    scope_metrics: ScopeMetrics,
    repo: RepoBucket,
) -> Result<UsageRecords, OtlpError> {
    if scope_metrics.metrics.is_empty() {
        return Err(OtlpError::MissingMetrics);
    }
    let mut records = UsageRecords::default();
    for metric in scope_metrics.metrics {
        records.extend(records_from_metric(metric, repo.clone())?);
    }
    Ok(records)
}

fn records_from_metric(metric: Metric, repo: RepoBucket) -> Result<UsageRecords, OtlpError> {
    let mut records = UsageRecords::default();
    match metric.name.as_str() {
        "token.usage" => {
            for data_point in proto_data_points(metric)? {
                records
                    .token_usage
                    .push(record_from_proto_data_point(data_point, repo.clone())?);
            }
        }
        "cost.usage" => {
            for data_point in proto_data_points(metric)? {
                records
                    .cost_usage
                    .push(cost_record_from_proto_data_point(data_point, repo.clone())?);
            }
        }
        _ => {}
    }
    Ok(records)
}

fn records_from_resource_logs(resource_logs: OtlpResourceLogs) -> Result<UsageRecords, OtlpError> {
    let repo = resource_logs
        .resource
        .as_ref()
        .map(|resource| repo_from_proto_attributes(&resource.attributes))
        .unwrap_or_else(RepoBucket::no_repo);

    let mut records = UsageRecords::default();
    for scope_logs in resource_logs.scope_logs {
        records.extend(records_from_scope_logs_for_logs(scope_logs, repo.clone())?);
    }
    Ok(records)
}

fn records_from_scope_logs_for_logs(
    scope_logs: ScopeLogs,
    repo: RepoBucket,
) -> Result<UsageRecords, OtlpError> {
    let mut records = UsageRecords::default();
    for log_record in scope_logs.log_records {
        if let Some(record) = proto_tool_call_record(log_record, repo.clone())? {
            records.tool_calls.push(record);
        }
    }
    Ok(records)
}

fn proto_data_points(
    metric: Metric,
) -> Result<Vec<opentelemetry_proto::tonic::metrics::v1::NumberDataPoint>, OtlpError> {
    let sum = match metric.data {
        Some(metric::Data::Sum(sum)) => sum,
        Some(_) | None => return Err(OtlpError::InvalidMetricKind),
    };
    if sum.data_points.is_empty() {
        return Err(OtlpError::MissingDataPoints);
    }
    Ok(sum.data_points)
}

fn record_from_proto_data_point(
    data_point: opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    repo: RepoBucket,
) -> Result<TokenUsageRecord, OtlpError> {
    let model = proto_attribute(&data_point.attributes, "model").ok_or(OtlpError::MissingModel)?;
    let measure = proto_attribute(&data_point.attributes, "token.type")
        .or_else(|| proto_attribute(&data_point.attributes, "type"))
        .and_then(|value| TokenMeasure::from_attribute(&value))
        .ok_or(OtlpError::InvalidMeasure)?;
    let token_count = match data_point.value.ok_or(OtlpError::MissingTokenCount)? {
        number_data_point::Value::AsInt(value) => {
            TokenCount::try_new(u64::try_from(value).map_err(|_| OtlpError::NumberOutOfRange)?)
                .ok_or(OtlpError::NumberOutOfRange)?
        }
        number_data_point::Value::AsDouble(value) => token_count_from_f64(value)?,
    };
    let occurred_at = if data_point.time_unix_nano == 0 {
        TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
    } else {
        TimestampMillis::try_from_unix_nanos(data_point.time_unix_nano)
    }
    .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = if data_point.start_time_unix_nano == 0 {
        occurred_at
    } else {
        TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
            .ok_or(OtlpError::MissingTimestamp)?
    };
    Ok(TokenUsageRecord::new(
        occurred_at,
        counter_start,
        repo,
        ModelName::new(model),
        measure,
        token_count,
    ))
}

fn cost_record_from_proto_data_point(
    data_point: opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    repo: RepoBucket,
) -> Result<CostUsageRecord, OtlpError> {
    let model = proto_attribute(&data_point.attributes, "model").ok_or(OtlpError::MissingModel)?;
    let cost_usd = match data_point.value.ok_or(OtlpError::MissingCost)? {
        number_data_point::Value::AsInt(value) => {
            CostUsd::from_whole_usd(u64::try_from(value).map_err(|_| OtlpError::InvalidCost)?)
                .ok_or(OtlpError::InvalidCost)?
        }
        number_data_point::Value::AsDouble(value) => {
            CostUsd::from_f64(value).ok_or(OtlpError::InvalidCost)?
        }
    };
    let occurred_at = if data_point.time_unix_nano == 0 {
        TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
    } else {
        TimestampMillis::try_from_unix_nanos(data_point.time_unix_nano)
    }
    .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
        .filter(|timestamp| data_point.start_time_unix_nano != 0 && timestamp.value() != 0)
        .ok_or(OtlpError::MissingCounterStart)?;
    Ok(CostUsageRecord::new(
        occurred_at,
        counter_start,
        repo,
        ModelName::new(model),
        cost_usd,
    ))
}

fn json_data_points(metric: &Value) -> Result<&Vec<Value>, OtlpError> {
    metric
        .get("sum")
        .and_then(|sum| sum.get("dataPoints"))
        .and_then(Value::as_array)
        .filter(|data_points| !data_points.is_empty())
        .ok_or(OtlpError::MissingDataPoints)
}

fn json_token_record(data_point: &Value, repo: RepoBucket) -> Result<TokenUsageRecord, OtlpError> {
    let attributes = data_point.get("attributes").and_then(Value::as_array);
    let model = json_attribute(attributes, "model").ok_or(OtlpError::MissingModel)?;
    let measure = json_attribute(attributes, "token.type")
        .or_else(|| json_attribute(attributes, "type"))
        .and_then(|value| TokenMeasure::from_attribute(&value))
        .ok_or(OtlpError::InvalidMeasure)?;
    let token_count = json_token_count(data_point)?;
    let occurred_at = json_timestamp(data_point, "timeUnixNano")
        .or_else(|| json_timestamp(data_point, "startTimeUnixNano"))
        .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = json_timestamp(data_point, "startTimeUnixNano").unwrap_or(occurred_at);
    Ok(TokenUsageRecord::new(
        occurred_at,
        counter_start,
        repo,
        ModelName::new(model),
        measure,
        token_count,
    ))
}

fn json_cost_record(data_point: &Value, repo: RepoBucket) -> Result<CostUsageRecord, OtlpError> {
    let attributes = data_point.get("attributes").and_then(Value::as_array);
    let model = json_attribute(attributes, "model").ok_or(OtlpError::MissingModel)?;
    let cost_usd = json_cost(data_point)?;
    let occurred_at = json_timestamp(data_point, "timeUnixNano")
        .or_else(|| json_timestamp(data_point, "startTimeUnixNano"))
        .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = json_counter_start(data_point, "startTimeUnixNano")
        .ok_or(OtlpError::MissingCounterStart)?;
    Ok(CostUsageRecord::new(
        occurred_at,
        counter_start,
        repo,
        ModelName::new(model),
        cost_usd,
    ))
}

fn proto_tool_call_record(
    log_record: LogRecord,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    if !is_tool_result_event(Some(log_record.event_name.as_str()), || {
        proto_attribute(&log_record.attributes, "event.name")
    }) {
        return Ok(None);
    }
    let tool_name = proto_tool_name(&log_record.attributes)?;
    let occurred_at_nanos = if log_record.time_unix_nano == 0 {
        log_record.observed_time_unix_nano
    } else {
        log_record.time_unix_nano
    };
    let occurred_at = TimestampMillis::try_from_unix_nanos(occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let harness = HarnessName::new("claude_code");
    let event_key = tool_call_event_key(
        &repo,
        harness.as_str(),
        tool_name.as_str(),
        occurred_at_nanos,
    );
    Ok(Some(ToolCallRecord::new(
        event_key,
        occurred_at,
        repo,
        harness,
        tool_name,
    )))
}

fn json_tool_call_record(
    log_record: &Value,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    let attributes = log_record.get("attributes").and_then(Value::as_array);
    if !is_tool_result_event(log_record.get("eventName").and_then(Value::as_str), || {
        json_attribute(attributes, "event.name")
    }) {
        return Ok(None);
    }
    let tool_name = json_tool_name(attributes)?;
    let (occurred_at_nanos, occurred_at) = json_log_timestamp(log_record)?;
    let harness = HarnessName::new("claude_code");
    let event_key = tool_call_event_key(
        &repo,
        harness.as_str(),
        tool_name.as_str(),
        occurred_at_nanos,
    );
    Ok(Some(ToolCallRecord::new(
        event_key,
        occurred_at,
        repo,
        harness,
        tool_name,
    )))
}

fn proto_tool_name(attributes: &[KeyValue]) -> Result<ToolName, OtlpError> {
    let value = proto_attribute(attributes, "tool.name")
        .or_else(|| proto_attribute(attributes, "tool_name"))
        .ok_or(OtlpError::MissingToolName)?;
    ToolName::try_new(value).ok_or(OtlpError::InvalidToolName)
}

fn json_tool_name(attributes: Option<&Vec<Value>>) -> Result<ToolName, OtlpError> {
    let value = json_attribute(attributes, "tool.name")
        .or_else(|| json_attribute(attributes, "tool_name"))
        .ok_or(OtlpError::MissingToolName)?;
    ToolName::try_new(value).ok_or(OtlpError::InvalidToolName)
}

fn tool_call_event_key(
    repo: &RepoBucket,
    harness: &str,
    tool_name: &str,
    occurred_at_nanos: u64,
) -> ToolCallEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-log-tool-result");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("harness=");
    canonical.push_str(harness);
    canonical.push('\n');
    canonical.push_str("tool_name=");
    canonical.push_str(tool_name);
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    ToolCallEventKey::new(canonical)
}

fn append_repo_key(output: &mut String, repo: &RepoBucket) {
    match repo {
        RepoBucket::NoRepo => output.push_str("repo_bucket=no_repo\nrepo_name=\nrepo_path=\n"),
        RepoBucket::Repo(identity) => {
            output.push_str("repo_bucket=repo\nrepo_name=");
            output.push_str(
                identity
                    .name
                    .as_ref()
                    .map(RepoName::as_str)
                    .unwrap_or_default(),
            );
            output.push_str("\nrepo_path=");
            output.push_str(
                identity
                    .path
                    .as_ref()
                    .map(RepoPath::as_str)
                    .unwrap_or_default(),
            );
            output.push('\n');
        }
    }
}

fn is_tool_result_event(
    event_name: Option<&str>,
    attribute_event_name: impl FnOnce() -> Option<String>,
) -> bool {
    matches_tool_result(event_name) || matches_tool_result(attribute_event_name().as_deref())
}

fn matches_tool_result(value: Option<&str>) -> bool {
    value
        .map(|value| value == "tool_result" || value.ends_with(".tool_result"))
        .unwrap_or(false)
}

fn repo_from_proto_attributes(attributes: &[KeyValue]) -> RepoBucket {
    repo_from_optional_attributes(
        proto_attribute(attributes, "repo.name"),
        proto_attribute(attributes, "repo.path"),
    )
}

fn repo_from_json_attributes(attributes: Option<&Vec<Value>>) -> RepoBucket {
    repo_from_optional_attributes(
        json_attribute(attributes, "repo.name"),
        json_attribute(attributes, "repo.path"),
    )
}

fn repo_from_optional_attributes(name: Option<String>, path: Option<String>) -> RepoBucket {
    let name = meaningful_attribute(name).map(RepoName::new);
    let path = meaningful_attribute(path).map(RepoPath::new);
    RepoIdentity::from_parts(name, path)
        .map(RepoBucket::repo)
        .unwrap_or_else(RepoBucket::no_repo)
}

fn meaningful_attribute(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn proto_attribute(attributes: &[KeyValue], key: &str) -> Option<String> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(proto_string_value)
}

fn proto_string_value(value: &AnyValue) -> Option<String> {
    match value.value.as_ref()? {
        any_value::Value::StringValue(value) => Some(value.clone()),
        _ => None,
    }
}

fn json_attribute(attributes: Option<&Vec<Value>>, key: &str) -> Option<String> {
    attributes?
        .iter()
        .find(|attribute| attribute.get("key").and_then(Value::as_str) == Some(key))
        .and_then(|attribute| attribute.get("value"))
        .and_then(|value| value.get("stringValue"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn json_number(value: &Value, key: &str) -> Option<u64> {
    let value = value.get(key)?;
    json_u64_value(value)
}

fn json_timestamp(value: &Value, key: &str) -> Option<TimestampMillis> {
    json_number(value, key).and_then(TimestampMillis::try_from_unix_nanos)
}

fn json_log_timestamp(value: &Value) -> Result<(u64, TimestampMillis), OtlpError> {
    ["timeUnixNano", "observedTimeUnixNano"]
        .into_iter()
        .find_map(|key| {
            let raw_value = json_number(value, key)?;
            TimestampMillis::try_from_unix_nanos(raw_value).map(|timestamp| (raw_value, timestamp))
        })
        .ok_or(OtlpError::MissingTimestamp)
}

fn json_counter_start(value: &Value, key: &str) -> Option<TimestampMillis> {
    let raw_value = json_number(value, key)?;
    if raw_value == 0 {
        return None;
    }
    TimestampMillis::try_from_unix_nanos(raw_value).filter(|timestamp| timestamp.value() != 0)
}

fn json_token_count(value: &Value) -> Result<TokenCount, OtlpError> {
    let raw_value = match (value.get("asInt"), value.get("asDouble")) {
        (Some(value), _) => json_u64_value(value).ok_or(OtlpError::NumberOutOfRange)?,
        (None, Some(value)) => {
            let Some(value) = value.as_f64() else {
                return Err(OtlpError::NumberOutOfRange);
            };
            return token_count_from_f64(value);
        }
        (None, None) => return Err(OtlpError::MissingTokenCount),
    };
    TokenCount::try_new(raw_value).ok_or(OtlpError::NumberOutOfRange)
}

fn json_cost(value: &Value) -> Result<CostUsd, OtlpError> {
    let raw_value = match (value.get("asDouble"), value.get("asInt")) {
        (Some(value), _) => json_cost_value(value).ok_or(OtlpError::InvalidCost)?,
        (None, Some(value)) => json_cost_value(value).ok_or(OtlpError::InvalidCost)?,
        (None, None) => return Err(OtlpError::MissingCost),
    };
    Ok(raw_value)
}

fn token_count_from_f64(value: f64) -> Result<TokenCount, OtlpError> {
    let Some(value) = valid_u64_from_f64(value) else {
        return Err(
            if value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 {
                OtlpError::NonIntegralTokenCount
            } else {
                OtlpError::NumberOutOfRange
            },
        );
    };
    TokenCount::try_new(value).ok_or(OtlpError::NumberOutOfRange)
}

fn valid_u64_from_f64(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 || value > u64::MAX as f64 || value.fract() != 0.0 {
        return None;
    }
    Some(value as u64)
}

fn json_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        .or_else(|| value.as_f64().and_then(valid_u64_from_f64))
}

fn json_cost_value(value: &Value) -> Option<CostUsd> {
    match value {
        Value::Number(number) => CostUsd::from_decimal_str(&number.to_string())
            .or_else(|| number.as_f64().and_then(CostUsd::from_f64)),
        Value::String(text) => CostUsd::from_decimal_str(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
    use opentelemetry_proto::tonic::metrics::v1::{
        Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric::Data,
        number_data_point::Value,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    use super::*;

    #[test]
    fn protobuf_rejects_missing_required_token_datapoint_fields() {
        let payload = protobuf_payload(vec![NumberDataPoint {
            attributes: vec![string_attribute("token.type", "input")],
            start_time_unix_nano: 1_781_956_700_000_000_000,
            time_unix_nano: 1_781_956_800_000_000_000,
            exemplars: Vec::new(),
            flags: 0,
            value: Some(Value::AsInt(100)),
        }]);

        assert!(matches!(
            parse_token_usage_protobuf(&payload),
            Err(OtlpError::MissingModel)
        ));
    }

    #[test]
    fn protobuf_rejects_empty_metric_requests() {
        let payload = ExportMetricsServiceRequest {
            resource_metrics: Vec::new(),
        }
        .encode_to_vec();

        assert!(matches!(
            parse_token_usage_protobuf(&payload),
            Err(OtlpError::MissingResourceMetrics)
        ));
    }

    #[test]
    fn protobuf_rejects_fractional_double_token_counts() {
        let payload = protobuf_payload(vec![NumberDataPoint {
            attributes: vec![
                string_attribute("model", "claude-opus-4-20250514"),
                string_attribute("token.type", "input"),
            ],
            start_time_unix_nano: 1_781_956_700_000_000_000,
            time_unix_nano: 1_781_956_800_000_000_000,
            exemplars: Vec::new(),
            flags: 0,
            value: Some(Value::AsDouble(1.9)),
        }]);

        assert!(matches!(
            parse_token_usage_protobuf(&payload),
            Err(OtlpError::NonIntegralTokenCount)
        ));
    }

    #[test]
    fn protobuf_rejects_mixed_batches_with_invalid_token_metric_kind() {
        let payload = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: None,
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![
                        Metric {
                            name: "token.usage".to_owned(),
                            description: String::new(),
                            unit: "{token}".to_owned(),
                            metadata: Vec::new(),
                            data: Some(Data::Sum(Sum {
                                data_points: vec![NumberDataPoint {
                                    attributes: vec![
                                        string_attribute("model", "claude-opus-4-20250514"),
                                        string_attribute("token.type", "input"),
                                    ],
                                    start_time_unix_nano: 1_781_956_700_000_000_000,
                                    time_unix_nano: 1_781_956_800_000_000_000,
                                    exemplars: Vec::new(),
                                    flags: 0,
                                    value: Some(Value::AsInt(100)),
                                }],
                                aggregation_temporality: 2,
                                is_monotonic: true,
                            })),
                        },
                        Metric {
                            name: "token.usage".to_owned(),
                            description: String::new(),
                            unit: "{token}".to_owned(),
                            metadata: Vec::new(),
                            data: None,
                        },
                    ],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec();

        assert!(matches!(
            parse_token_usage_protobuf(&payload),
            Err(OtlpError::InvalidMetricKind)
        ));
    }

    #[test]
    fn json_rejects_fractional_double_token_counts() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 1.9,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                    { "key": "token.type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_token_usage_json(payload),
            Err(OtlpError::NonIntegralTokenCount)
        ));
    }

    #[test]
    fn json_rejects_payloads_without_token_usage_metrics() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "not.token.usage",
                        "sum": { "dataPoints": [] }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_token_usage_json(payload),
            Err(OtlpError::NoTokenUsageMetrics)
        ));
    }

    #[test]
    fn json_rejects_empty_token_usage_metrics_in_mixed_batches() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [
                        {
                            "name": "token.usage",
                            "sum": {
                                "dataPoints": [{
                                    "timeUnixNano": "1781956800000000000",
                                    "asInt": "100",
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } },
                                        { "key": "token.type", "value": { "stringValue": "input" } }
                                    ]
                                }]
                            }
                        },
                        {
                            "name": "token.usage",
                            "sum": { "dataPoints": [] }
                        }
                    ]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_token_usage_json(payload),
            Err(OtlpError::MissingDataPoints)
        ));
    }

    #[test]
    fn json_tool_result_requires_explicit_tool_name_attribute() {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "name", "value": { "stringValue": "Read" } }
                        ]
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_logs(payload),
            Err(OtlpError::MissingToolName)
        ));
    }

    #[test]
    fn json_tool_result_rejects_content_like_tool_names() {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "Read /Users/oyr/secret.txt" } }
                        ]
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_logs(payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn json_tool_result_rejects_unknown_tool_names() {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "InventedTool" } }
                        ]
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_logs(payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn json_tool_result_accepts_mcp_tool_names() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "mcp__linear__issue_view" } }
                        ]
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert_eq!(
            records.tool_calls[0].tool_name,
            ToolName::new("mcp__linear__issue_view")
        );

        Ok(())
    }

    #[test]
    fn json_tool_result_event_key_reuses_the_selected_timestamp()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "observedTimeUnixNano": "1781956900000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "Read" } }
                        ]
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert_eq!(records.tool_calls[0].occurred_at.value(), 1_781_956_800_000);
        assert!(
            records.tool_calls[0]
                .event_key
                .as_str()
                .contains("occurred_at_nanos=1781956800000000000\n")
        );

        Ok(())
    }

    #[test]
    fn json_usage_logs_accept_empty_batches_as_noop() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": []
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert!(records.is_empty());

        Ok(())
    }

    #[test]
    fn json_usage_logs_skip_empty_scope_collections() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": []
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert!(records.is_empty());

        Ok(())
    }

    #[test]
    fn json_usage_logs_skip_empty_log_record_collections() -> Result<(), Box<dyn std::error::Error>>
    {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": []
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert!(records.is_empty());

        Ok(())
    }

    #[test]
    fn protobuf_usage_logs_accept_empty_batches_as_noop() -> Result<(), Box<dyn std::error::Error>>
    {
        let payload = ExportLogsServiceRequest {
            resource_logs: Vec::new(),
        }
        .encode_to_vec();

        let records = parse_otlp_protobuf_usage_logs(&payload)?;

        assert!(records.is_empty());

        Ok(())
    }

    #[test]
    fn protobuf_usage_logs_skip_empty_scope_and_log_record_collections()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = ExportLogsServiceRequest {
            resource_logs: vec![
                OtlpResourceLogs {
                    resource: None,
                    scope_logs: Vec::new(),
                    schema_url: String::new(),
                },
                OtlpResourceLogs {
                    resource: None,
                    scope_logs: vec![ScopeLogs {
                        scope: None,
                        log_records: Vec::new(),
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                },
            ],
        }
        .encode_to_vec();

        let records = parse_otlp_protobuf_usage_logs(&payload)?;

        assert!(records.is_empty());

        Ok(())
    }

    #[test]
    fn protobuf_usage_logs_normalize_claude_tool_results_with_repo_and_harness()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_logs_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/Users/oyr/projects/kvasir"),
            ],
            vec![
                LogRecord {
                    time_unix_nano: 1_781_956_800_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: None,
                    attributes: vec![string_attribute("tool.name", "Read")],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "tool_result".to_owned(),
                },
                LogRecord {
                    time_unix_nano: 1_781_956_900_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: None,
                    attributes: vec![string_attribute("tool.name", "Ignored")],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "user_prompt".to_owned(),
                },
            ],
        );

        let records = parse_otlp_protobuf_usage_logs(&payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(
            records.tool_calls[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/Users/oyr/projects/kvasir"),
            ))
        );
        assert_eq!(
            records.tool_calls[0].harness,
            HarnessName::new("claude_code")
        );
        assert_eq!(records.tool_calls[0].tool_name, ToolName::new("Read"));

        Ok(())
    }

    #[test]
    fn json_partial_repo_resource_attributes_preserve_repo_name() {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
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
            }]
        }"#;

        let records = parse_token_usage_json(payload).expect("valid token usage");

        assert_eq!(
            records[0].repo,
            RepoBucket::repo(
                RepoIdentity::from_parts(Some(RepoName::new("kvasir")), None).unwrap()
            )
        );
    }

    #[test]
    fn protobuf_partial_repo_resource_attributes_preserve_repo_path() {
        let payload = protobuf_payload_with_resource_attributes(
            vec![string_attribute("repo.path", "/repos/kvasir")],
            vec![NumberDataPoint {
                attributes: vec![
                    string_attribute("model", "claude-opus-4-20250514"),
                    string_attribute("token.type", "input"),
                ],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(100)),
            }],
        );

        let records = parse_token_usage_protobuf(&payload).expect("valid token usage");

        assert_eq!(
            records[0].repo,
            RepoBucket::repo(
                RepoIdentity::from_parts(None, Some(RepoPath::new("/repos/kvasir"))).unwrap()
            )
        );
    }

    #[test]
    fn json_blank_repo_resource_attributes_use_no_repo_bucket() {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": " " } },
                        { "key": "repo.path", "value": { "stringValue": "" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
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
            }]
        }"#;

        let records = parse_token_usage_json(payload).expect("valid token usage");

        assert_eq!(records[0].repo, RepoBucket::no_repo());
    }

    #[test]
    fn protobuf_blank_repo_resource_attributes_use_no_repo_bucket() {
        let payload = protobuf_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "\t"),
                string_attribute("repo.path", " "),
            ],
            vec![NumberDataPoint {
                attributes: vec![
                    string_attribute("model", "claude-opus-4-20250514"),
                    string_attribute("token.type", "input"),
                ],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(100)),
            }],
        );

        let records = parse_token_usage_protobuf(&payload).expect("valid token usage");

        assert_eq!(records[0].repo, RepoBucket::no_repo());
    }

    #[test]
    fn json_repo_resource_attributes_are_trimmed() {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": " kvasir " } },
                        { "key": "repo.path", "value": { "stringValue": "\t/repos/kvasir\n" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "sum": {
                            "dataPoints": [{
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
            }]
        }"#;

        let records = parse_token_usage_json(payload).expect("valid token usage");

        assert_eq!(
            records[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir")
            ))
        );
    }

    #[test]
    fn protobuf_repo_resource_attributes_are_trimmed() {
        let payload = protobuf_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "\nkvasir"),
                string_attribute("repo.path", "/repos/kvasir "),
            ],
            vec![NumberDataPoint {
                attributes: vec![
                    string_attribute("model", "claude-opus-4-20250514"),
                    string_attribute("token.type", "input"),
                ],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(100)),
            }],
        );

        let records = parse_token_usage_protobuf(&payload).expect("valid token usage");

        assert_eq!(
            records[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir")
            ))
        );
    }

    #[test]
    fn json_usage_parser_accepts_cost_only_payloads() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 0.1,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload).expect("valid cost usage");

        assert!(records.token_usage.is_empty());
        assert_eq!(records.cost_usage.len(), 1);
        assert_eq!(records.cost_usage[0].repo, RepoBucket::no_repo());
        assert_eq!(
            records.cost_usage[0].cost_usd,
            CostUsd::from_decimal_str("0.1").unwrap()
        );
    }

    #[test]
    fn json_usage_parser_preserves_scientific_notation_cost() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781956800000000000",
                                    "asDouble": 1.23e-7,
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                    ]
                                },
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781956900000000000",
                                    "asDouble": "2.5e-7",
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                    ]
                                }
                            ]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload).expect("valid cost usage");

        assert_eq!(records.cost_usage.len(), 2);
        assert_eq!(
            records.cost_usage[0].cost_usd,
            CostUsd::from_nanos(123).unwrap()
        );
        assert_eq!(
            records.cost_usage[1].cost_usd,
            CostUsd::from_nanos(250).unwrap()
        );
    }

    #[test]
    fn json_usage_parser_rejects_cost_without_counter_start() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 0.1,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::MissingCounterStart)
        ));
    }

    #[test]
    fn json_usage_parser_rejects_zero_cost_counter_start() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "cost.usage",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "0",
                                "timeUnixNano": "1781956800000000000",
                                "asDouble": 0.1,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "claude-opus-4-20250514" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::MissingCounterStart)
        ));
    }

    #[test]
    fn protobuf_usage_parser_preserves_native_cost_with_repo_attributes() {
        let payload = protobuf_cost_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![NumberDataPoint {
                attributes: vec![string_attribute("model", "claude-opus-4-20250514")],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsDouble(0.375)),
            }],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload).expect("valid cost usage");

        assert!(records.token_usage.is_empty());
        assert_eq!(records.cost_usage.len(), 1);
        assert_eq!(
            records.cost_usage[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir")
            ))
        );
        assert_eq!(
            records.cost_usage[0].cost_usd,
            CostUsd::from_decimal_str("0.375").unwrap()
        );
    }

    #[test]
    fn protobuf_usage_parser_rejects_cost_without_counter_start() {
        let payload = protobuf_cost_payload_with_resource_attributes(
            Vec::new(),
            vec![NumberDataPoint {
                attributes: vec![string_attribute("model", "claude-opus-4-20250514")],
                start_time_unix_nano: 0,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsDouble(0.375)),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingCounterStart)
        ));
    }

    fn protobuf_payload(data_points: Vec<NumberDataPoint>) -> Vec<u8> {
        protobuf_payload_with_resource_attributes(Vec::new(), data_points)
    }

    fn protobuf_payload_with_resource_attributes(
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<NumberDataPoint>,
    ) -> Vec<u8> {
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: resource_attributes,
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "token.usage".to_owned(),
                        description: String::new(),
                        unit: "{token}".to_owned(),
                        metadata: Vec::new(),
                        data: Some(Data::Sum(Sum {
                            data_points,
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn protobuf_cost_payload_with_resource_attributes(
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<NumberDataPoint>,
    ) -> Vec<u8> {
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: resource_attributes,
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "cost.usage".to_owned(),
                        description: String::new(),
                        unit: "USD".to_owned(),
                        metadata: Vec::new(),
                        data: Some(Data::Sum(Sum {
                            data_points,
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn protobuf_logs_payload_with_resource_attributes(
        resource_attributes: Vec<KeyValue>,
        log_records: Vec<LogRecord>,
    ) -> Vec<u8> {
        ExportLogsServiceRequest {
            resource_logs: vec![OtlpResourceLogs {
                resource: Some(Resource {
                    attributes: resource_attributes,
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn parse_token_usage_json(bytes: &[u8]) -> Result<Vec<TokenUsageRecord>, OtlpError> {
        let records = parse_otlp_json_usage_metrics(bytes)?;
        if records.token_usage.is_empty() {
            return Err(OtlpError::NoTokenUsageMetrics);
        }
        Ok(records.token_usage)
    }

    fn parse_token_usage_protobuf(bytes: &[u8]) -> Result<Vec<TokenUsageRecord>, OtlpError> {
        let records = parse_otlp_protobuf_usage_metrics(bytes)?;
        if records.token_usage.is_empty() {
            return Err(OtlpError::NoTokenUsageMetrics);
        }
        Ok(records.token_usage)
    }

    fn string_attribute(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_owned(),
            key_strindex: 0,
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(value.to_owned())),
            }),
        }
    }
}
