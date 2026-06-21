use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use serde_json::Value;

use crate::rpc::{ModelName, TimestampMillis};
use crate::usage::{RepoIdentity, RepoName, RepoPath, TokenCount, TokenMeasure, TokenUsageRecord};

#[derive(Debug, thiserror::Error)]
pub enum OtlpError {
    #[error("invalid otlp protobuf")]
    InvalidProtobuf(#[from] prost::DecodeError),
    #[error("invalid otlp json")]
    InvalidJson(#[from] serde_json::Error),
    #[error("otlp number is out of range")]
    NumberOutOfRange,
}

pub fn parse_otlp_protobuf_metrics(bytes: &[u8]) -> Result<Vec<TokenUsageRecord>, OtlpError> {
    let request = ExportMetricsServiceRequest::decode(bytes)?;
    let mut records = Vec::new();
    for resource_metrics in request.resource_metrics {
        records.extend(records_from_resource_metrics(resource_metrics));
    }
    Ok(records)
}

pub fn parse_otlp_json_metrics(bytes: &[u8]) -> Result<Vec<TokenUsageRecord>, OtlpError> {
    let payload: Value = serde_json::from_slice(bytes)?;
    let mut records = Vec::new();
    for resource_metrics in payload
        .get("resourceMetrics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let resource_attributes = resource_metrics
            .get("resource")
            .and_then(|resource| resource.get("attributes"))
            .and_then(Value::as_array);
        let repo = repo_from_json_attributes(resource_attributes);
        let Some(scope_metrics) = resource_metrics
            .get("scopeMetrics")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for scope_metric in scope_metrics {
            let Some(metrics) = scope_metric.get("metrics").and_then(Value::as_array) else {
                continue;
            };
            for metric in metrics {
                if metric.get("name").and_then(Value::as_str) != Some("token.usage") {
                    continue;
                }
                let Some(data_points) = metric
                    .get("sum")
                    .and_then(|sum| sum.get("dataPoints"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for data_point in data_points {
                    let attributes = data_point.get("attributes").and_then(Value::as_array);
                    let Some(model) = json_attribute(attributes, "model") else {
                        continue;
                    };
                    let Some(measure) = json_attribute(attributes, "token.type")
                        .or_else(|| json_attribute(attributes, "type"))
                        .and_then(|value| TokenMeasure::from_attribute(&value))
                    else {
                        continue;
                    };
                    let Some(token_count) = json_token_count(data_point)? else {
                        continue;
                    };
                    let Some(occurred_at) = json_timestamp(data_point, "timeUnixNano")
                        .or_else(|| json_timestamp(data_point, "startTimeUnixNano"))
                    else {
                        continue;
                    };
                    let counter_start =
                        json_timestamp(data_point, "startTimeUnixNano").unwrap_or(occurred_at);
                    records.push(TokenUsageRecord::new(
                        occurred_at,
                        counter_start,
                        repo.clone(),
                        ModelName::new(model),
                        measure,
                        token_count,
                    ));
                }
            }
        }
    }
    Ok(records)
}

fn records_from_resource_metrics(resource_metrics: ResourceMetrics) -> Vec<TokenUsageRecord> {
    let repo = resource_metrics
        .resource
        .as_ref()
        .map(|resource| repo_from_proto_attributes(&resource.attributes))
        .unwrap_or_else(no_repo);

    resource_metrics
        .scope_metrics
        .into_iter()
        .flat_map(|scope_metrics| records_from_scope_metrics(scope_metrics, repo.clone()))
        .collect()
}

fn records_from_scope_metrics(
    scope_metrics: ScopeMetrics,
    repo: RepoIdentity,
) -> Vec<TokenUsageRecord> {
    scope_metrics
        .metrics
        .into_iter()
        .flat_map(|metric| records_from_metric(metric, repo.clone()))
        .collect()
}

fn records_from_metric(metric: Metric, repo: RepoIdentity) -> Vec<TokenUsageRecord> {
    if metric.name != "token.usage" {
        return Vec::new();
    }

    let Some(metric::Data::Sum(sum)) = metric.data else {
        return Vec::new();
    };

    sum.data_points
        .into_iter()
        .filter_map(|data_point| {
            let model = proto_attribute(&data_point.attributes, "model")?;
            let measure = proto_attribute(&data_point.attributes, "token.type")
                .or_else(|| proto_attribute(&data_point.attributes, "type"))
                .and_then(|value| TokenMeasure::from_attribute(&value))?;
            let token_count = match data_point.value? {
                number_data_point::Value::AsInt(value) => {
                    TokenCount::try_new(u64::try_from(value).ok()?)?
                }
                number_data_point::Value::AsDouble(value) if value >= 0.0 => {
                    TokenCount::try_new(value as u64)?
                }
                number_data_point::Value::AsDouble(_) => return None,
            };
            let occurred_at = if data_point.time_unix_nano == 0 {
                TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)?
            } else {
                TimestampMillis::try_from_unix_nanos(data_point.time_unix_nano)?
            };
            let counter_start = if data_point.start_time_unix_nano == 0 {
                occurred_at
            } else {
                TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)?
            };
            Some(TokenUsageRecord::new(
                occurred_at,
                counter_start,
                repo.clone(),
                ModelName::new(model),
                measure,
                token_count,
            ))
        })
        .collect()
}

fn repo_from_proto_attributes(attributes: &[KeyValue]) -> RepoIdentity {
    RepoIdentity::new(
        RepoName::new(proto_attribute(attributes, "repo.name").unwrap_or_else(|| "no-repo".into())),
        RepoPath::new(proto_attribute(attributes, "repo.path").unwrap_or_else(|| "no-repo".into())),
    )
}

fn repo_from_json_attributes(attributes: Option<&Vec<Value>>) -> RepoIdentity {
    RepoIdentity::new(
        RepoName::new(json_attribute(attributes, "repo.name").unwrap_or_else(|| "no-repo".into())),
        RepoPath::new(json_attribute(attributes, "repo.path").unwrap_or_else(|| "no-repo".into())),
    )
}

fn no_repo() -> RepoIdentity {
    RepoIdentity::new(RepoName::new("no-repo"), RepoPath::new("no-repo"))
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
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| *number >= 0.0)
                .map(|number| number as u64)
        })
}

fn json_timestamp(value: &Value, key: &str) -> Option<TimestampMillis> {
    json_number(value, key).and_then(TimestampMillis::try_from_unix_nanos)
}

fn json_token_count(value: &Value) -> Result<Option<TokenCount>, OtlpError> {
    let Some(raw_value) = json_number(value, "asInt").or_else(|| json_number(value, "asDouble"))
    else {
        return Ok(None);
    };
    TokenCount::try_new(raw_value)
        .map(Some)
        .ok_or(OtlpError::NumberOutOfRange)
}
