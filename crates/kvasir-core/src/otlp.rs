use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use serde_json::Value;

use crate::rpc::{ModelName, TimestampMillis};
use crate::usage::{
    CostUsageRecord, CostUsd, RepoBucket, RepoIdentity, RepoName, RepoPath, TokenCount,
    TokenMeasure, TokenUsageRecord, UsageRecords,
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
