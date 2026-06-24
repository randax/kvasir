use std::collections::BTreeMap;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{
    LogRecord, ResourceLogs as OtlpResourceLogs, ScopeLogs,
};
use opentelemetry_proto::tonic::metrics::v1::{
    HistogramDataPoint, Metric, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::trace::v1::{
    ResourceSpans as OtlpResourceSpans, ScopeSpans, Span as OtlpSpan,
};
use prost::Message;
use serde_json::Value;

use crate::rpc::{
    HarnessName, ModelName, PromptId, SessionId, SpanId, SpanName, TimestampMillis, ToolName,
    TraceId, TraceSpanKind, canonical_harness_name,
};
use crate::usage::{
    ContentEventKey, ContentKind, ContentRecord, ContentText, CostUsageRecord, CostUsd, RepoBucket,
    RepoIdentity, RepoName, RepoPath, TokenCount, TokenMeasure, TokenUsageEventKey,
    TokenUsageRecord, TokenUsageSignal, ToolCallCount, ToolCallEventKey, ToolCallRecord,
    TraceSpanRecord, UsageRecords,
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
    #[error("otlp payload is missing resource spans")]
    MissingResourceSpans,
    #[error("otlp payload is missing scope spans")]
    MissingScopeSpans,
    #[error("otlp payload is missing spans")]
    MissingSpans,
    #[error("otlp trace span is missing session id")]
    MissingSessionId,
    #[error("otlp trace span is missing prompt id")]
    MissingPromptId,
    #[error("otlp trace span is missing trace id")]
    MissingTraceId,
    #[error("otlp trace span has invalid trace id")]
    InvalidTraceId,
    #[error("otlp trace span is missing span id")]
    MissingSpanId,
    #[error("otlp trace span has invalid span id")]
    InvalidSpanId,
    #[error("otlp trace span is missing or has invalid kind")]
    InvalidTraceSpanKind,
}

pub fn parse_otlp_protobuf_usage_metrics(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let request = ExportMetricsServiceRequest::decode(bytes)?;
    if request.resource_metrics.is_empty() {
        return Err(OtlpError::MissingResourceMetrics);
    }
    let mut records = UsageRecords::default();
    let mut codex_point_ordinals = BTreeMap::new();
    for resource_metrics in request.resource_metrics {
        records.extend(records_from_resource_metrics(
            resource_metrics,
            &mut codex_point_ordinals,
        )?);
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
    let mut codex_point_ordinals = BTreeMap::new();
    for resource_metrics in resource_metrics {
        let resource_attributes = resource_metrics
            .get("resource")
            .and_then(|resource| resource.get("attributes"))
            .and_then(Value::as_array);
        let repo = repo_from_json_attributes(resource_attributes);
        let resource_harness = harness_from_json_attributes(resource_attributes);
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
                    Some(name) if is_token_metric_name(name) => {
                        let data_points = json_data_points(metric)?;
                        for data_point in data_points {
                            if let Some(record) = json_token_record(
                                metric,
                                data_point,
                                &mut codex_point_ordinals,
                                repo.clone(),
                            )? {
                                records.token_usage.push(record);
                            }
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
                    Some(name) if is_codex_tool_call_metric_name(name) => {
                        let data_points = json_tool_call_data_points(metric)?;
                        for data_point in data_points {
                            if let Some(record) = json_codex_tool_call_metric_record(
                                data_point,
                                &mut codex_point_ordinals,
                                repo.clone(),
                            )? {
                                records.tool_calls.push(record);
                            }
                        }
                    }
                    Some(name) if is_tool_call_sum_metric_name(name) => {
                        let data_points = json_tool_call_data_points(metric)?;
                        for data_point in data_points {
                            if let Some(record) = json_cumulative_tool_call_metric_record(
                                name,
                                data_point,
                                repo.clone(),
                                resource_harness.as_deref(),
                            )? {
                                records.tool_calls.push(record);
                            }
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
        let harness = harness_from_json_attributes(resource_attributes);
        let session_id = json_session_id(resource_attributes);
        let prompt_id = json_prompt_id(resource_attributes);
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
                if let Some(record) = json_token_usage_log_record(log_record, repo.clone())? {
                    records.token_usage.push(record);
                }
                if let Some(record) = json_tool_call_record(log_record, repo.clone())? {
                    records.tool_calls.push(record);
                }
                if let Some(record) = json_content_record(
                    log_record,
                    repo.clone(),
                    harness.as_deref(),
                    session_id.clone(),
                    prompt_id.clone(),
                )? {
                    records.content.push(record);
                }
            }
        }
    }
    Ok(records)
}

pub fn parse_otlp_protobuf_traces(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let request = ExportTraceServiceRequest::decode(bytes)?;
    if request.resource_spans.is_empty() {
        return Err(OtlpError::MissingResourceSpans);
    }
    let mut records = UsageRecords::default();
    for resource_spans in request.resource_spans {
        records.extend(records_from_resource_spans(resource_spans)?);
    }
    if records.is_empty() {
        return Err(OtlpError::MissingSpans);
    }
    Ok(records)
}

pub fn parse_otlp_json_traces(bytes: &[u8]) -> Result<UsageRecords, OtlpError> {
    let payload: Value = serde_json::from_slice(bytes)?;
    let mut records = UsageRecords::default();
    let resource_spans = payload
        .get("resourceSpans")
        .and_then(Value::as_array)
        .ok_or(OtlpError::MissingResourceSpans)?;
    for resource_spans in resource_spans {
        let resource_attributes = resource_spans
            .get("resource")
            .and_then(|resource| resource.get("attributes"))
            .and_then(Value::as_array);
        let session_id = json_session_id(resource_attributes);
        let prompt_id = json_prompt_id(resource_attributes);
        let repo = repo_from_json_attributes(resource_attributes);
        let harness = harness_from_json_attributes(resource_attributes);
        let scope_spans = resource_spans
            .get("scopeSpans")
            .and_then(Value::as_array)
            .ok_or(OtlpError::MissingScopeSpans)?;
        for scope_spans in scope_spans {
            let spans = scope_spans
                .get("spans")
                .and_then(Value::as_array)
                .ok_or(OtlpError::MissingSpans)?;
            for span in spans {
                records.extend(json_opencode_span_records(
                    span,
                    repo.clone(),
                    harness.as_deref(),
                )?);
                if let Some(record) = json_trace_span_record(
                    span,
                    session_id.clone(),
                    prompt_id.clone(),
                    harness.as_deref(),
                )? {
                    records.trace_spans.push(record);
                }
            }
        }
    }
    if records.is_empty() {
        return Err(OtlpError::MissingSpans);
    }
    Ok(records)
}

fn records_from_resource_metrics(
    resource_metrics: ResourceMetrics,
    codex_point_ordinals: &mut BTreeMap<String, usize>,
) -> Result<UsageRecords, OtlpError> {
    let repo = resource_metrics
        .resource
        .as_ref()
        .map(|resource| repo_from_proto_attributes(&resource.attributes))
        .unwrap_or_else(RepoBucket::no_repo);
    let resource_harness = resource_metrics
        .resource
        .as_ref()
        .and_then(|resource| harness_from_proto_attributes(&resource.attributes));

    if resource_metrics.scope_metrics.is_empty() {
        return Err(OtlpError::MissingScopeMetrics);
    }
    let mut records = UsageRecords::default();
    for scope_metrics in resource_metrics.scope_metrics {
        records.extend(records_from_scope_metrics(
            scope_metrics,
            repo.clone(),
            resource_harness.as_deref(),
            codex_point_ordinals,
        )?);
    }
    Ok(records)
}

fn records_from_scope_metrics(
    scope_metrics: ScopeMetrics,
    repo: RepoBucket,
    resource_harness: Option<&str>,
    codex_point_ordinals: &mut BTreeMap<String, usize>,
) -> Result<UsageRecords, OtlpError> {
    if scope_metrics.metrics.is_empty() {
        return Err(OtlpError::MissingMetrics);
    }
    let mut records = UsageRecords::default();
    for metric in scope_metrics.metrics {
        records.extend(records_from_metric(
            metric,
            repo.clone(),
            resource_harness,
            codex_point_ordinals,
        )?);
    }
    Ok(records)
}

fn records_from_metric(
    metric: Metric,
    repo: RepoBucket,
    resource_harness: Option<&str>,
    codex_point_ordinals: &mut BTreeMap<String, usize>,
) -> Result<UsageRecords, OtlpError> {
    let mut records = UsageRecords::default();
    let metric_name = metric.name.clone();
    match metric_name.as_str() {
        name if is_cumulative_token_metric_name(name) => {
            for data_point in proto_sum_data_points(metric)? {
                records
                    .token_usage
                    .push(record_from_proto_sum_data_point(data_point, repo.clone())?);
            }
        }
        "codex.turn.token_usage" => {
            for data_point in proto_codex_delta_histogram_data_points(metric)? {
                if let Some(record) = record_from_codex_proto_histogram(
                    data_point,
                    codex_point_ordinals,
                    repo.clone(),
                )? {
                    records.token_usage.push(record);
                }
            }
        }
        name if is_codex_tool_call_metric_name(name) => {
            for data_point in proto_codex_delta_histogram_data_points(metric)? {
                if let Some(record) = record_from_codex_proto_tool_call_histogram(
                    data_point,
                    codex_point_ordinals,
                    repo.clone(),
                )? {
                    records.tool_calls.push(record);
                }
            }
        }
        name if is_tool_call_sum_metric_name(name) => {
            for data_point in proto_cumulative_sum_data_points(metric)? {
                if let Some(record) = record_from_proto_tool_call_sum_data_point(
                    name,
                    data_point,
                    repo.clone(),
                    resource_harness,
                )? {
                    records.tool_calls.push(record);
                }
            }
        }
        "cost.usage" => {
            for data_point in proto_sum_data_points(metric)? {
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
    let harness = resource_logs
        .resource
        .as_ref()
        .and_then(|resource| harness_from_proto_attributes(&resource.attributes));
    let session_id = resource_logs
        .resource
        .as_ref()
        .and_then(|resource| proto_session_id(&resource.attributes));
    let prompt_id = resource_logs
        .resource
        .as_ref()
        .and_then(|resource| proto_prompt_id(&resource.attributes));

    let mut records = UsageRecords::default();
    for scope_logs in resource_logs.scope_logs {
        records.extend(records_from_scope_logs_for_logs(
            scope_logs,
            repo.clone(),
            harness.as_deref(),
            session_id.clone(),
            prompt_id.clone(),
        )?);
    }
    Ok(records)
}

fn records_from_scope_logs_for_logs(
    scope_logs: ScopeLogs,
    repo: RepoBucket,
    resource_harness: Option<&str>,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
) -> Result<UsageRecords, OtlpError> {
    let mut records = UsageRecords::default();
    for log_record in scope_logs.log_records {
        if let Some(record) = proto_token_usage_log_record(&log_record, repo.clone())? {
            records.token_usage.push(record);
        }
        if let Some(record) = proto_tool_call_record(log_record.clone(), repo.clone())? {
            records.tool_calls.push(record);
        }
        if let Some(record) = proto_content_record(
            &log_record,
            repo.clone(),
            resource_harness,
            resource_session_id.clone(),
            resource_prompt_id.clone(),
        )? {
            records.content.push(record);
        }
    }
    Ok(records)
}

fn records_from_resource_spans(
    resource_spans: OtlpResourceSpans,
) -> Result<UsageRecords, OtlpError> {
    let resource_attributes = resource_spans
        .resource
        .as_ref()
        .map(|resource| resource.attributes.as_slice())
        .unwrap_or(&[]);
    let session_id = proto_session_id(resource_attributes);
    let prompt_id = proto_prompt_id(resource_attributes);
    let repo = repo_from_proto_attributes(resource_attributes);
    let harness = harness_from_proto_attributes(resource_attributes);
    let mut records = UsageRecords::default();
    for scope_spans in resource_spans.scope_spans {
        records.extend(records_from_scope_spans_for_traces(
            scope_spans,
            session_id.clone(),
            prompt_id.clone(),
            repo.clone(),
            harness.as_deref(),
        )?);
    }
    Ok(records)
}

fn records_from_scope_spans_for_traces(
    scope_spans: ScopeSpans,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
    repo: RepoBucket,
    resource_harness: Option<&str>,
) -> Result<UsageRecords, OtlpError> {
    let mut records = UsageRecords::default();
    for span in scope_spans.spans {
        records.extend(proto_opencode_span_records(
            &span,
            repo.clone(),
            resource_harness,
        )?);
        if let Some(record) = proto_trace_span_record(
            span,
            resource_session_id.clone(),
            resource_prompt_id.clone(),
            resource_harness,
        )? {
            records.trace_spans.push(record);
        }
    }
    Ok(records)
}

fn proto_sum_data_points(
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

fn proto_cumulative_sum_data_points(
    metric: Metric,
) -> Result<Vec<opentelemetry_proto::tonic::metrics::v1::NumberDataPoint>, OtlpError> {
    let sum = match metric.data {
        Some(metric::Data::Sum(sum)) => sum,
        Some(_) | None => return Err(OtlpError::InvalidMetricKind),
    };
    if sum.aggregation_temporality != 2 || !sum.is_monotonic {
        return Err(OtlpError::InvalidMetricKind);
    }
    if sum.data_points.is_empty() {
        return Err(OtlpError::MissingDataPoints);
    }
    Ok(sum.data_points)
}

fn proto_codex_delta_histogram_data_points(
    metric: Metric,
) -> Result<Vec<HistogramDataPoint>, OtlpError> {
    let histogram = match metric.data {
        Some(metric::Data::Histogram(histogram)) => histogram,
        Some(_) | None => return Err(OtlpError::InvalidMetricKind),
    };
    if histogram.aggregation_temporality != 1 {
        return Err(OtlpError::InvalidMetricKind);
    }
    if histogram.data_points.is_empty() {
        return Err(OtlpError::MissingDataPoints);
    }
    Ok(histogram.data_points)
}

fn record_from_proto_sum_data_point(
    data_point: opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    repo: RepoBucket,
) -> Result<TokenUsageRecord, OtlpError> {
    let model = first_meaningful_proto_attribute(
        &data_point.attributes,
        &["model", "gen_ai.request.model"],
    )
    .ok_or(OtlpError::MissingModel)?;
    let measure = first_meaningful_proto_attribute(
        &data_point.attributes,
        &[
            "token.type",
            "type",
            "direction",
            "token_type",
            "gen_ai.token.type",
        ],
    )
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

fn record_from_codex_proto_histogram(
    data_point: HistogramDataPoint,
    point_ordinals: &mut BTreeMap<String, usize>,
    repo: RepoBucket,
) -> Result<Option<TokenUsageRecord>, OtlpError> {
    let token_type = proto_attribute(&data_point.attributes, "token_type")
        .or_else(|| proto_attribute(&data_point.attributes, "token.type"))
        .or_else(|| proto_attribute(&data_point.attributes, "type"))
        .ok_or(OtlpError::InvalidMeasure)?;
    let Some(measure) = codex_token_measure(&token_type) else {
        if is_ignored_codex_token_type(&token_type) {
            return Ok(None);
        }
        return Err(OtlpError::InvalidMeasure);
    };
    let model = proto_attribute(&data_point.attributes, "model").ok_or(OtlpError::MissingModel)?;
    let token_count = token_count_from_f64(data_point.sum.ok_or(OtlpError::MissingTokenCount)?)?;
    if data_point.start_time_unix_nano == 0 {
        return Err(OtlpError::MissingCounterStart);
    }
    let counter_start = TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
        .ok_or(OtlpError::MissingCounterStart)?;
    let raw_occurred_at_nanos = if data_point.time_unix_nano == 0 {
        data_point.start_time_unix_nano
    } else {
        data_point.time_unix_nano
    };
    let occurred_at = if data_point.time_unix_nano == 0 {
        TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
    } else {
        TimestampMillis::try_from_unix_nanos(data_point.time_unix_nano)
    }
    .ok_or(OtlpError::MissingTimestamp)?;
    let point_ordinal = codex_token_usage_point_ordinal(
        point_ordinals,
        &repo,
        &model,
        &token_type,
        data_point.start_time_unix_nano,
        raw_occurred_at_nanos,
        token_count,
    );
    let event_key = codex_token_usage_event_key(
        &repo,
        &model,
        &token_type,
        data_point.start_time_unix_nano,
        raw_occurred_at_nanos,
        point_ordinal,
        token_count,
    );
    Ok(Some(TokenUsageRecord::new_delta(
        event_key,
        occurred_at,
        counter_start,
        repo,
        ModelName::new(model),
        measure,
        token_count,
    )))
}

fn record_from_codex_proto_tool_call_histogram(
    data_point: HistogramDataPoint,
    point_ordinals: &mut BTreeMap<String, usize>,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    let tool_name = proto_codex_metric_tool_name(&data_point.attributes)?;
    let call_count = tool_call_count_from_f64(data_point.sum.ok_or(OtlpError::MissingTokenCount)?)?;
    if call_count.value() == 0 {
        return Ok(None);
    }
    let raw_occurred_at_nanos =
        proto_metric_occurred_at_nanos(data_point.time_unix_nano, data_point.start_time_unix_nano)?;
    let occurred_at = TimestampMillis::try_from_unix_nanos(raw_occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let harness = HarnessName::new("codex");
    let point_ordinal = codex_tool_call_point_ordinal(
        point_ordinals,
        &repo,
        tool_name.as_str(),
        raw_occurred_at_nanos,
        call_count,
    );
    let event_key = codex_tool_call_event_key(
        &repo,
        tool_name.as_str(),
        raw_occurred_at_nanos,
        point_ordinal,
        call_count,
    );
    Ok(Some(ToolCallRecord::new_counted(
        event_key,
        occurred_at,
        repo,
        harness,
        tool_name,
        call_count,
    )))
}

fn record_from_proto_tool_call_sum_data_point(
    metric_name: &str,
    data_point: opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    repo: RepoBucket,
    resource_harness: Option<&str>,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    let tool_name = proto_tool_name(&data_point.attributes)?;
    let call_count = proto_tool_call_count(&data_point)?;
    if call_count.value() == 0 {
        return Ok(None);
    }
    let raw_occurred_at_nanos =
        proto_metric_occurred_at_nanos(data_point.time_unix_nano, data_point.start_time_unix_nano)?;
    let occurred_at = TimestampMillis::try_from_unix_nanos(raw_occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = TimestampMillis::try_from_unix_nanos(data_point.start_time_unix_nano)
        .filter(|timestamp| data_point.start_time_unix_nano != 0 && timestamp.value() != 0)
        .ok_or(OtlpError::MissingCounterStart)?;
    let harness = tool_call_metric_harness(metric_name, resource_harness);
    let event_key = metric_tool_call_event_key(
        &repo,
        harness.as_str(),
        tool_name.as_str(),
        data_point.start_time_unix_nano,
        raw_occurred_at_nanos,
        call_count,
    );
    Ok(Some(ToolCallRecord::new_cumulative(
        event_key,
        occurred_at,
        counter_start,
        repo,
        harness,
        tool_name,
        call_count,
    )))
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
    metric_data(metric)?
        .and_then(Value::as_array)
        .filter(|data_points| !data_points.is_empty())
        .ok_or(OtlpError::MissingDataPoints)
}

fn metric_data(metric: &Value) -> Result<Option<&Value>, OtlpError> {
    match metric.get("name").and_then(Value::as_str) {
        Some(name) if is_cumulative_token_metric_name(name) || name == "cost.usage" => metric
            .get("sum")
            .map(|sum| sum.get("dataPoints"))
            .ok_or(OtlpError::InvalidMetricKind),
        Some("codex.turn.token_usage") => {
            let histogram = metric
                .get("histogram")
                .ok_or(OtlpError::InvalidMetricKind)?;
            if !json_codex_histogram_is_delta(histogram) {
                return Err(OtlpError::InvalidMetricKind);
            }
            Ok(histogram.get("dataPoints"))
        }
        _ => Err(OtlpError::InvalidMetricKind),
    }
}

fn json_codex_histogram_is_delta(histogram: &Value) -> bool {
    match histogram.get("aggregationTemporality") {
        Some(Value::Number(value)) => value.as_u64() == Some(1),
        Some(Value::String(value)) => value == "1" || value == "AGGREGATION_TEMPORALITY_DELTA",
        _ => false,
    }
}

fn json_tool_call_data_points(metric: &Value) -> Result<&Vec<Value>, OtlpError> {
    match metric.get("name").and_then(Value::as_str) {
        Some(name) if is_codex_tool_call_metric_name(name) => {
            let histogram = metric
                .get("histogram")
                .ok_or(OtlpError::InvalidMetricKind)?;
            if !json_codex_histogram_is_delta(histogram) {
                return Err(OtlpError::InvalidMetricKind);
            }
            histogram
                .get("dataPoints")
                .and_then(Value::as_array)
                .filter(|data_points| !data_points.is_empty())
                .ok_or(OtlpError::MissingDataPoints)
        }
        Some(name) if is_tool_call_sum_metric_name(name) => metric
            .get("sum")
            .ok_or(OtlpError::InvalidMetricKind)
            .and_then(|sum| {
                if !json_sum_is_cumulative(sum) {
                    return Err(OtlpError::InvalidMetricKind);
                }
                sum.get("dataPoints")
                    .and_then(Value::as_array)
                    .filter(|data_points| !data_points.is_empty())
                    .ok_or(OtlpError::MissingDataPoints)
            }),
        _ => Err(OtlpError::InvalidMetricKind),
    }
}

fn json_sum_is_cumulative(sum: &Value) -> bool {
    let cumulative = match sum.get("aggregationTemporality") {
        Some(Value::Number(value)) => value.as_u64() == Some(2),
        Some(Value::String(value)) => value == "2" || value == "AGGREGATION_TEMPORALITY_CUMULATIVE",
        _ => false,
    };
    let monotonic = match sum.get("isMonotonic") {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value == "true",
        _ => false,
    };
    cumulative && monotonic
}

fn json_token_record(
    metric: &Value,
    data_point: &Value,
    codex_point_ordinals: &mut BTreeMap<String, usize>,
    repo: RepoBucket,
) -> Result<Option<TokenUsageRecord>, OtlpError> {
    let attributes = data_point.get("attributes").and_then(Value::as_array);
    let model = first_meaningful_json_attribute(attributes, &["model", "gen_ai.request.model"])
        .ok_or(OtlpError::MissingModel)?;
    let token_type = first_meaningful_json_attribute(
        attributes,
        &[
            "token_type",
            "token.type",
            "type",
            "direction",
            "gen_ai.token.type",
        ],
    )
    .ok_or(OtlpError::InvalidMeasure)?;
    let Some(measure) = token_measure_for_metric(metric, &token_type) else {
        if is_codex_token_metric(metric) && is_ignored_codex_token_type(&token_type) {
            return Ok(None);
        }
        return Err(OtlpError::InvalidMeasure);
    };
    let token_count = json_token_count(data_point)?;
    let raw_occurred_at_nanos = json_number(data_point, "timeUnixNano")
        .or_else(|| json_number(data_point, "startTimeUnixNano"))
        .ok_or(OtlpError::MissingTimestamp)?;
    let occurred_at = json_timestamp(data_point, "timeUnixNano")
        .or_else(|| json_timestamp(data_point, "startTimeUnixNano"))
        .ok_or(OtlpError::MissingTimestamp)?;
    if is_codex_token_metric(metric) {
        let raw_counter_start_nanos = json_number(data_point, "startTimeUnixNano")
            .filter(|value| *value != 0)
            .ok_or(OtlpError::MissingCounterStart)?;
        let counter_start = TimestampMillis::try_from_unix_nanos(raw_counter_start_nanos)
            .ok_or(OtlpError::MissingCounterStart)?;
        let point_ordinal = codex_token_usage_point_ordinal(
            codex_point_ordinals,
            &repo,
            &model,
            &token_type,
            raw_counter_start_nanos,
            raw_occurred_at_nanos,
            token_count,
        );
        let event_key = codex_token_usage_event_key(
            &repo,
            &model,
            &token_type,
            raw_counter_start_nanos,
            raw_occurred_at_nanos,
            point_ordinal,
            token_count,
        );
        Ok(Some(TokenUsageRecord::new_delta(
            event_key,
            occurred_at,
            counter_start,
            repo,
            ModelName::new(model),
            measure,
            token_count,
        )))
    } else {
        let counter_start = json_timestamp(data_point, "startTimeUnixNano").unwrap_or(occurred_at);
        Ok(Some(TokenUsageRecord::new(
            occurred_at,
            counter_start,
            repo,
            ModelName::new(model),
            measure,
            token_count,
        )))
    }
}

fn json_codex_tool_call_metric_record(
    data_point: &Value,
    point_ordinals: &mut BTreeMap<String, usize>,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    let attributes = data_point.get("attributes").and_then(Value::as_array);
    let tool_name = json_codex_metric_tool_name(attributes)?;
    let call_count = json_tool_call_count(data_point)?;
    if call_count.value() == 0 {
        return Ok(None);
    }
    let raw_occurred_at_nanos = json_metric_occurred_at_nanos(data_point)?;
    let occurred_at = TimestampMillis::try_from_unix_nanos(raw_occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let point_ordinal = codex_tool_call_point_ordinal(
        point_ordinals,
        &repo,
        tool_name.as_str(),
        raw_occurred_at_nanos,
        call_count,
    );
    let event_key = codex_tool_call_event_key(
        &repo,
        tool_name.as_str(),
        raw_occurred_at_nanos,
        point_ordinal,
        call_count,
    );
    Ok(Some(ToolCallRecord::new_counted(
        event_key,
        occurred_at,
        repo,
        HarnessName::new("codex"),
        tool_name,
        call_count,
    )))
}

fn json_cumulative_tool_call_metric_record(
    metric_name: &str,
    data_point: &Value,
    repo: RepoBucket,
    resource_harness: Option<&str>,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    let attributes = data_point.get("attributes").and_then(Value::as_array);
    let tool_name = json_tool_name(attributes)?;
    let call_count = json_tool_call_count(data_point)?;
    if call_count.value() == 0 {
        return Ok(None);
    }
    let raw_occurred_at_nanos = json_metric_occurred_at_nanos(data_point)?;
    let occurred_at = TimestampMillis::try_from_unix_nanos(raw_occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let harness = tool_call_metric_harness(metric_name, resource_harness);

    let counter_start = json_counter_start(data_point, "startTimeUnixNano")
        .ok_or(OtlpError::MissingCounterStart)?;
    let raw_counter_start_nanos = json_number(data_point, "startTimeUnixNano")
        .filter(|value| *value != 0)
        .ok_or(OtlpError::MissingCounterStart)?;
    let event_key = metric_tool_call_event_key(
        &repo,
        harness.as_str(),
        tool_name.as_str(),
        raw_counter_start_nanos,
        raw_occurred_at_nanos,
        call_count,
    );
    Ok(Some(ToolCallRecord::new_cumulative(
        event_key,
        occurred_at,
        counter_start,
        repo,
        harness,
        tool_name,
        call_count,
    )))
}

fn json_metric_occurred_at_nanos(data_point: &Value) -> Result<u64, OtlpError> {
    json_number(data_point, "timeUnixNano")
        .filter(|value| *value != 0)
        .or_else(|| json_number(data_point, "startTimeUnixNano").filter(|value| *value != 0))
        .ok_or(OtlpError::MissingTimestamp)
}

fn proto_metric_occurred_at_nanos(
    time_unix_nano: u64,
    start_time_unix_nano: u64,
) -> Result<u64, OtlpError> {
    [time_unix_nano, start_time_unix_nano]
        .into_iter()
        .find(|value| *value != 0)
        .ok_or(OtlpError::MissingTimestamp)
}

fn token_measure_for_metric(metric: &Value, value: &str) -> Option<TokenMeasure> {
    if is_codex_token_metric(metric) {
        codex_token_measure(value)
    } else {
        TokenMeasure::from_attribute(value)
    }
}

fn is_token_metric_name(name: &str) -> bool {
    is_cumulative_token_metric_name(name) || name == "codex.turn.token_usage"
}

fn is_cumulative_token_metric_name(name: &str) -> bool {
    matches!(name, "token.usage" | "github.copilot.chat.tokens")
}

fn is_codex_tool_call_metric_name(name: &str) -> bool {
    matches!(name, "codex.turn.tool.call" | "codex.tool.call")
}

fn is_tool_call_sum_metric_name(name: &str) -> bool {
    matches!(
        name,
        "github.copilot.chat.tool_calls"
            | "github.copilot.chat.tool.calls"
            | "gen_ai.tool.calls"
            | "gen_ai.client.tool.calls"
    )
}

fn tool_call_metric_harness(metric_name: &str, resource_harness: Option<&str>) -> HarnessName {
    if is_codex_tool_call_metric_name(metric_name) {
        return HarnessName::new("codex");
    }
    if metric_name.starts_with("github.copilot.") {
        return HarnessName::new("github_copilot");
    }
    resource_harness
        .map(HarnessName::new)
        .unwrap_or_else(|| HarnessName::new("unknown"))
}

fn is_codex_token_metric(metric: &Value) -> bool {
    metric.get("name").and_then(Value::as_str) == Some("codex.turn.token_usage")
}

fn codex_token_measure(value: &str) -> Option<TokenMeasure> {
    match value {
        "input" => Some(TokenMeasure::Input),
        "output" => Some(TokenMeasure::Output),
        "cached_input" => Some(TokenMeasure::Cache),
        _ => None,
    }
}

fn is_ignored_codex_token_type(value: &str) -> bool {
    matches!(value, "total" | "reasoning_output" | "tool")
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

fn proto_token_usage_log_record(
    log_record: &LogRecord,
    repo: RepoBucket,
) -> Result<Option<TokenUsageRecord>, OtlpError> {
    if !is_token_usage_event(Some(log_record.event_name.as_str()), || {
        proto_attribute(&log_record.attributes, "event.name")
    }) {
        return Ok(None);
    }
    let model = first_meaningful_proto_attribute(
        &log_record.attributes,
        &["model", "gen_ai.request.model"],
    )
    .ok_or(OtlpError::MissingModel)?;
    let measure = first_meaningful_proto_attribute(
        &log_record.attributes,
        &[
            "token.type",
            "type",
            "direction",
            "token_type",
            "gen_ai.token.type",
        ],
    )
    .and_then(|value| TokenMeasure::from_attribute(&value))
    .ok_or(OtlpError::InvalidMeasure)?;
    let token_count = proto_log_token_count(log_record)?;
    let occurred_at_nanos = if log_record.time_unix_nano == 0 {
        log_record.observed_time_unix_nano
    } else {
        log_record.time_unix_nano
    };
    let occurred_at = TimestampMillis::try_from_unix_nanos(occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let model = ModelName::new(model);
    if let Some(counter_start) = proto_log_counter_start(log_record)? {
        return Ok(Some(TokenUsageRecord::new_from_signal(
            TokenUsageSignal::Logs,
            occurred_at,
            counter_start,
            repo,
            model,
            measure,
            token_count,
        )));
    }
    let event_key = log_token_usage_event_key(
        &repo,
        model.as_str(),
        measure,
        occurred_at_nanos,
        token_count,
    );
    Ok(Some(TokenUsageRecord::new_delta_from_signal(
        TokenUsageSignal::Logs,
        event_key,
        occurred_at,
        repo,
        model,
        measure,
        token_count,
    )))
}

fn json_token_usage_log_record(
    log_record: &Value,
    repo: RepoBucket,
) -> Result<Option<TokenUsageRecord>, OtlpError> {
    let attributes = log_record.get("attributes").and_then(Value::as_array);
    if !is_token_usage_event(log_record.get("eventName").and_then(Value::as_str), || {
        json_attribute(attributes, "event.name")
    }) {
        return Ok(None);
    }
    let model = first_meaningful_json_attribute(attributes, &["model", "gen_ai.request.model"])
        .ok_or(OtlpError::MissingModel)?;
    let measure = first_meaningful_json_attribute(
        attributes,
        &[
            "token.type",
            "type",
            "direction",
            "token_type",
            "gen_ai.token.type",
        ],
    )
    .and_then(|value| TokenMeasure::from_attribute(&value))
    .ok_or(OtlpError::InvalidMeasure)?;
    let token_count = json_log_token_count(log_record, attributes)?;
    let (occurred_at_nanos, occurred_at) = json_log_timestamp(log_record)?;
    let model = ModelName::new(model);
    if let Some(counter_start) = json_log_counter_start(log_record, attributes)? {
        return Ok(Some(TokenUsageRecord::new_from_signal(
            TokenUsageSignal::Logs,
            occurred_at,
            counter_start,
            repo,
            model,
            measure,
            token_count,
        )));
    }
    let event_key = log_token_usage_event_key(
        &repo,
        model.as_str(),
        measure,
        occurred_at_nanos,
        token_count,
    );
    Ok(Some(TokenUsageRecord::new_delta_from_signal(
        TokenUsageSignal::Logs,
        event_key,
        occurred_at,
        repo,
        model,
        measure,
        token_count,
    )))
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

fn proto_content_record(
    log_record: &LogRecord,
    repo: RepoBucket,
    resource_harness: Option<&str>,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
) -> Result<Option<ContentRecord>, OtlpError> {
    let Some(harness) = content_record_harness(resource_harness) else {
        return Ok(None);
    };
    if !is_content_event(Some(log_record.event_name.as_str()), || {
        proto_attribute(&log_record.attributes, "event.name")
    }) || !proto_bool_attribute(&log_record.attributes, "content.opt_in")
    {
        return Ok(None);
    }
    let Some(kind) = proto_attribute(&log_record.attributes, "content.type")
        .and_then(|value| ContentKind::from_attribute(&value))
    else {
        return Ok(None);
    };
    let Some(content) = log_record
        .body
        .as_ref()
        .and_then(proto_string_value)
        .and_then(ContentText::new)
    else {
        return Ok(None);
    };
    let Some(session_id) = resource_session_id.or_else(|| proto_session_id(&log_record.attributes))
    else {
        return Ok(None);
    };
    let Some(prompt_id) = resource_prompt_id.or_else(|| proto_prompt_id(&log_record.attributes))
    else {
        return Ok(None);
    };
    let occurred_at_nanos = if log_record.time_unix_nano == 0 {
        log_record.observed_time_unix_nano
    } else {
        log_record.time_unix_nano
    };
    let occurred_at = TimestampMillis::try_from_unix_nanos(occurred_at_nanos)
        .ok_or(OtlpError::MissingTimestamp)?;
    let event_key = content_event_key(
        &repo,
        harness.as_str(),
        &session_id,
        &prompt_id,
        kind,
        occurred_at_nanos,
        content.as_str(),
    );
    Ok(Some(ContentRecord {
        event_key,
        occurred_at,
        session_id,
        prompt_id,
        repo,
        harness: HarnessName::new(harness),
        kind,
        content,
    }))
}

fn json_content_record(
    log_record: &Value,
    repo: RepoBucket,
    resource_harness: Option<&str>,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
) -> Result<Option<ContentRecord>, OtlpError> {
    let attributes = log_record.get("attributes").and_then(Value::as_array);
    let Some(harness) = content_record_harness(resource_harness) else {
        return Ok(None);
    };
    if !is_content_event(log_record.get("eventName").and_then(Value::as_str), || {
        json_attribute(attributes, "event.name")
    }) || !json_bool_attribute(attributes, "content.opt_in")
    {
        return Ok(None);
    }
    let Some(kind) = json_attribute(attributes, "content.type")
        .and_then(|value| ContentKind::from_attribute(&value))
    else {
        return Ok(None);
    };
    let Some(content) = log_record
        .get("body")
        .and_then(json_string_any_value)
        .and_then(ContentText::new)
    else {
        return Ok(None);
    };
    let Some(session_id) = resource_session_id.or_else(|| json_session_id(attributes)) else {
        return Ok(None);
    };
    let Some(prompt_id) = resource_prompt_id.or_else(|| json_prompt_id(attributes)) else {
        return Ok(None);
    };
    let (occurred_at_nanos, occurred_at) = json_log_timestamp(log_record)?;
    let event_key = content_event_key(
        &repo,
        harness.as_str(),
        &session_id,
        &prompt_id,
        kind,
        occurred_at_nanos,
        content.as_str(),
    );
    Ok(Some(ContentRecord {
        event_key,
        occurred_at,
        session_id,
        prompt_id,
        repo,
        harness: HarnessName::new(harness),
        kind,
        content,
    }))
}

const TOOL_NAME_ATTRIBUTE_KEYS: &[&str] = &[
    "tool.name",
    "tool_name",
    "ai.toolCall.name",
    "gen_ai.tool.name",
    "gen_ai.tool_name",
    "gen_ai.client.tool.name",
    "gen_ai.client.tool_name",
];

const TRACE_SPAN_KIND_ATTRIBUTE_KEYS: &[&str] = &[
    "claude.span.kind",
    "codex.span.kind",
    "github.copilot.span.kind",
    "opencode.span.kind",
    "span.kind",
    "kvasir.span.kind",
];

fn proto_tool_name(attributes: &[KeyValue]) -> Result<ToolName, OtlpError> {
    let value = first_proto_attribute(attributes, TOOL_NAME_ATTRIBUTE_KEYS)
        .ok_or(OtlpError::MissingToolName)?;
    ToolName::try_new(value).ok_or(OtlpError::InvalidToolName)
}

fn json_tool_name(attributes: Option<&Vec<Value>>) -> Result<ToolName, OtlpError> {
    let value = first_json_attribute(attributes, TOOL_NAME_ATTRIBUTE_KEYS)
        .ok_or(OtlpError::MissingToolName)?;
    ToolName::try_new(value).ok_or(OtlpError::InvalidToolName)
}

fn proto_trace_tool_name(attributes: &[KeyValue]) -> Option<ToolName> {
    TOOL_NAME_ATTRIBUTE_KEYS
        .iter()
        .filter_map(|key| proto_attribute(attributes, key))
        .filter_map(|value| meaningful_attribute(Some(value)))
        .find_map(ToolName::try_new)
}

fn json_trace_tool_name(attributes: Option<&Vec<Value>>) -> Option<ToolName> {
    TOOL_NAME_ATTRIBUTE_KEYS
        .iter()
        .filter_map(|key| json_attribute(attributes, key))
        .filter_map(|value| meaningful_attribute(Some(value)))
        .find_map(ToolName::try_new)
}

fn proto_codex_metric_tool_name(attributes: &[KeyValue]) -> Result<ToolName, OtlpError> {
    first_proto_attribute(attributes, TOOL_NAME_ATTRIBUTE_KEYS)
        .map(|value| ToolName::try_new(value).ok_or(OtlpError::InvalidToolName))
        .unwrap_or_else(|| Ok(ToolName::unknown()))
}

fn json_codex_metric_tool_name(attributes: Option<&Vec<Value>>) -> Result<ToolName, OtlpError> {
    first_json_attribute(attributes, TOOL_NAME_ATTRIBUTE_KEYS)
        .map(|value| ToolName::try_new(value).ok_or(OtlpError::InvalidToolName))
        .unwrap_or_else(|| Ok(ToolName::unknown()))
}

fn proto_log_counter_start(log_record: &LogRecord) -> Result<Option<TimestampMillis>, OtlpError> {
    let raw_value = proto_u64_attribute(
        &log_record.attributes,
        &[
            "start_time_unix_nano",
            "counter_start_unix_nano",
            "startTimeUnixNano",
        ],
    )
    .or_else(|| {
        log_record
            .body
            .as_ref()
            .and_then(|body| proto_u64_map_value(body, "start_time_unix_nano"))
    })
    .or_else(|| {
        log_record
            .body
            .as_ref()
            .and_then(|body| proto_u64_map_value(body, "counter_start_unix_nano"))
    })
    .or_else(|| {
        log_record
            .body
            .as_ref()
            .and_then(|body| proto_u64_map_value(body, "startTimeUnixNano"))
    })
    .transpose()?;

    raw_value
        .filter(|value| *value != 0)
        .map(|value| TimestampMillis::try_from_unix_nanos(value).ok_or(OtlpError::MissingTimestamp))
        .transpose()
}

fn json_log_counter_start(
    log_record: &Value,
    attributes: Option<&Vec<Value>>,
) -> Result<Option<TimestampMillis>, OtlpError> {
    let raw_value = json_u64_attribute(
        attributes,
        &[
            "start_time_unix_nano",
            "counter_start_unix_nano",
            "startTimeUnixNano",
        ],
    )
    .or_else(|| {
        log_record
            .get("body")
            .and_then(|body| json_u64_map_value(body, "start_time_unix_nano"))
    })
    .or_else(|| {
        log_record
            .get("body")
            .and_then(|body| json_u64_map_value(body, "counter_start_unix_nano"))
    })
    .or_else(|| {
        log_record
            .get("body")
            .and_then(|body| json_u64_map_value(body, "startTimeUnixNano"))
    })
    .transpose()?;

    raw_value
        .filter(|value| *value != 0)
        .map(|value| TimestampMillis::try_from_unix_nanos(value).ok_or(OtlpError::MissingTimestamp))
        .transpose()
}

fn proto_log_token_count(log_record: &LogRecord) -> Result<TokenCount, OtlpError> {
    proto_token_count_attribute(&log_record.attributes, &["token.count", "token_count"])
        .or_else(|| log_record.body.as_ref().map(proto_token_count_value))
        .ok_or(OtlpError::MissingTokenCount)?
}

fn proto_token_count_attribute(
    attributes: &[KeyValue],
    keys: &[&str],
) -> Option<Result<TokenCount, OtlpError>> {
    keys.iter().find_map(|key| {
        attributes
            .iter()
            .find(|attribute| attribute.key == *key)
            .and_then(|attribute| attribute.value.as_ref())
            .map(proto_token_count_value)
    })
}

fn proto_token_count_value(value: &AnyValue) -> Result<TokenCount, OtlpError> {
    match value.value.as_ref().ok_or(OtlpError::MissingTokenCount)? {
        any_value::Value::IntValue(value) => {
            TokenCount::try_new(u64::try_from(*value).map_err(|_| OtlpError::NumberOutOfRange)?)
                .ok_or(OtlpError::NumberOutOfRange)
        }
        any_value::Value::DoubleValue(value) => token_count_from_f64(*value),
        any_value::Value::StringValue(value) => {
            let value = value
                .parse::<u64>()
                .map_err(|_| OtlpError::NumberOutOfRange)?;
            TokenCount::try_new(value).ok_or(OtlpError::NumberOutOfRange)
        }
        _ => Err(OtlpError::MissingTokenCount),
    }
}

fn proto_tool_call_count(
    data_point: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
) -> Result<ToolCallCount, OtlpError> {
    match data_point
        .value
        .as_ref()
        .ok_or(OtlpError::MissingTokenCount)?
    {
        number_data_point::Value::AsInt(value) => {
            ToolCallCount::try_new(u64::try_from(*value).map_err(|_| OtlpError::NumberOutOfRange)?)
                .ok_or(OtlpError::NumberOutOfRange)
        }
        number_data_point::Value::AsDouble(value) => tool_call_count_from_f64(*value),
    }
}

fn proto_u64_attribute(attributes: &[KeyValue], keys: &[&str]) -> Option<Result<u64, OtlpError>> {
    keys.iter().find_map(|key| {
        attributes
            .iter()
            .find(|attribute| attribute.key == *key)
            .and_then(|attribute| attribute.value.as_ref())
            .map(proto_u64_value)
    })
}

fn proto_u64_map_value(value: &AnyValue, key: &str) -> Option<Result<u64, OtlpError>> {
    let any_value::Value::KvlistValue(values) = value.value.as_ref()? else {
        return None;
    };
    values
        .values
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .map(proto_u64_value)
}

fn proto_u64_value(value: &AnyValue) -> Result<u64, OtlpError> {
    match value.value.as_ref().ok_or(OtlpError::NumberOutOfRange)? {
        any_value::Value::IntValue(value) => {
            u64::try_from(*value).map_err(|_| OtlpError::NumberOutOfRange)
        }
        any_value::Value::DoubleValue(value) => {
            valid_u64_from_f64(*value).ok_or(OtlpError::NumberOutOfRange)
        }
        any_value::Value::StringValue(value) => value
            .parse::<u64>()
            .map_err(|_| OtlpError::NumberOutOfRange),
        _ => Err(OtlpError::NumberOutOfRange),
    }
}

fn json_log_token_count(
    log_record: &Value,
    attributes: Option<&Vec<Value>>,
) -> Result<TokenCount, OtlpError> {
    json_token_count_attribute(attributes, &["token.count", "token_count"])
        .or_else(|| log_record.get("body").map(json_token_count_any_value))
        .ok_or(OtlpError::MissingTokenCount)?
}

fn json_token_count_attribute(
    attributes: Option<&Vec<Value>>,
    keys: &[&str],
) -> Option<Result<TokenCount, OtlpError>> {
    let attributes = attributes?;
    keys.iter().find_map(|key| {
        attributes
            .iter()
            .find(|attribute| attribute.get("key").and_then(Value::as_str) == Some(*key))
            .and_then(|attribute| attribute.get("value"))
            .map(json_token_count_any_value)
    })
}

fn json_token_count_any_value(value: &Value) -> Result<TokenCount, OtlpError> {
    let raw_value = value
        .get("intValue")
        .or_else(|| value.get("stringValue"))
        .and_then(json_u64_value);
    if let Some(value) = raw_value {
        return TokenCount::try_new(value).ok_or(OtlpError::NumberOutOfRange);
    }
    if let Some(value) = value.get("doubleValue").and_then(Value::as_f64) {
        return token_count_from_f64(value);
    }
    Err(OtlpError::MissingTokenCount)
}

fn json_u64_attribute(
    attributes: Option<&Vec<Value>>,
    keys: &[&str],
) -> Option<Result<u64, OtlpError>> {
    let attributes = attributes?;
    keys.iter().find_map(|key| {
        attributes
            .iter()
            .find(|attribute| attribute.get("key").and_then(Value::as_str) == Some(*key))
            .and_then(|attribute| attribute.get("value"))
            .map(json_u64_any_value)
    })
}

fn json_u64_map_value(value: &Value, key: &str) -> Option<Result<u64, OtlpError>> {
    value
        .get("kvlistValue")
        .and_then(|kvlist| kvlist.get("values"))
        .and_then(Value::as_array)?
        .iter()
        .find(|attribute| attribute.get("key").and_then(Value::as_str) == Some(key))
        .and_then(|attribute| attribute.get("value"))
        .map(json_u64_any_value)
}

fn json_u64_any_value(value: &Value) -> Result<u64, OtlpError> {
    value
        .get("intValue")
        .or_else(|| value.get("stringValue"))
        .and_then(json_u64_value)
        .or_else(|| {
            value
                .get("doubleValue")
                .and_then(Value::as_f64)
                .and_then(valid_u64_from_f64)
        })
        .ok_or(OtlpError::NumberOutOfRange)
}

fn is_token_usage_event(
    event_name: Option<&str>,
    attribute_event_name: impl FnOnce() -> Option<String>,
) -> bool {
    matches!(
        event_name
            .and_then(|value| meaningful_attribute(Some(value.to_owned())))
            .or_else(attribute_event_name)
            .as_deref(),
        Some("token_usage" | "token.usage")
    )
}

fn proto_trace_span_record(
    span: OtlpSpan,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
    resource_harness: Option<&str>,
) -> Result<Option<TraceSpanRecord>, OtlpError> {
    let Some(kind) = proto_trace_span_kind(&span.attributes)
        .or_else(|| opencode_inferred_proto_span_kind(resource_harness, &span))
    else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::InvalidTraceSpanKind);
    };
    let Some(session_id) = resource_session_id.or_else(|| proto_session_id(&span.attributes))
    else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::MissingSessionId);
    };
    let Some(prompt_id) = resource_prompt_id.or_else(|| proto_prompt_id(&span.attributes)) else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::MissingPromptId);
    };
    let trace_id = canonical_proto_trace_id(span.trace_id)?;
    let span_id = canonical_proto_span_id(span.span_id)?;
    let parent_span_id = canonical_proto_parent_span_id(span.parent_span_id)?.map(SpanId::new);
    let started_at = TimestampMillis::try_from_unix_nanos(span.start_time_unix_nano)
        .ok_or(OtlpError::MissingTimestamp)?;
    let ended_at = TimestampMillis::try_from_unix_nanos(span.end_time_unix_nano)
        .ok_or(OtlpError::MissingTimestamp)?;
    let tool_name = if kind == TraceSpanKind::ToolCall {
        proto_trace_tool_name(&span.attributes)
    } else {
        None
    };
    Ok(Some(TraceSpanRecord {
        harness: HarnessName::new(
            canonical_harness(resource_harness).unwrap_or("unknown".to_owned()),
        ),
        session_id,
        prompt_id,
        trace_id: TraceId::new(trace_id),
        span_id: SpanId::new(span_id),
        parent_span_id,
        kind,
        name: SpanName::new(span.name),
        started_at,
        ended_at,
        duration_ms: duration_ms(started_at, ended_at),
        tool_name,
    }))
}

fn proto_opencode_span_records(
    span: &OtlpSpan,
    repo: RepoBucket,
    resource_harness: Option<&str>,
) -> Result<UsageRecords, OtlpError> {
    if !is_opencode_span(resource_harness, &span.name, |key| {
        proto_attribute(&span.attributes, key)
    }) {
        return Ok(UsageRecords::default());
    }
    let mut records = UsageRecords::default();
    records.extend(proto_opencode_token_records(span, repo.clone())?);
    if let Some(record) = proto_opencode_tool_call_record(span, repo)? {
        records.tool_calls.push(record);
    }
    Ok(records)
}

fn json_opencode_span_records(
    span: &Value,
    repo: RepoBucket,
    resource_harness: Option<&str>,
) -> Result<UsageRecords, OtlpError> {
    let attributes = span.get("attributes").and_then(Value::as_array);
    let name = span.get("name").and_then(Value::as_str).unwrap_or_default();
    if !is_opencode_span(resource_harness, name, |key| {
        json_attribute(attributes, key)
    }) {
        return Ok(UsageRecords::default());
    }
    let mut records = UsageRecords::default();
    records.extend(json_opencode_token_records(span, attributes, repo.clone())?);
    if let Some(record) = json_opencode_tool_call_record(span, attributes, repo)? {
        records.tool_calls.push(record);
    }
    Ok(records)
}

fn proto_opencode_token_records(
    span: &OtlpSpan,
    repo: RepoBucket,
) -> Result<UsageRecords, OtlpError> {
    let Some(model) = opencode_proto_model(&span.attributes) else {
        return Ok(UsageRecords::default());
    };
    let occurred_at = TimestampMillis::try_from_unix_nanos(span.end_time_unix_nano)
        .ok_or(OtlpError::MissingTimestamp)?;
    let counter_start = TimestampMillis::try_from_unix_nanos(span.start_time_unix_nano)
        .ok_or(OtlpError::MissingTimestamp)?;
    let mut records = UsageRecords::default();
    for (measure, keys) in opencode_token_attribute_keys() {
        let Some(token_count) = proto_token_count_attribute(&span.attributes, keys).transpose()?
        else {
            continue;
        };
        let event_key = trace_token_usage_event_key(
            &repo,
            model.as_str(),
            measure,
            &canonical_proto_trace_id(span.trace_id.clone())?,
            &canonical_proto_span_id(span.span_id.clone())?,
            token_count,
        );
        records
            .token_usage
            .push(TokenUsageRecord::new_delta_from_signal(
                TokenUsageSignal::OpenCodeTraces,
                event_key,
                occurred_at,
                repo.clone(),
                model.clone(),
                measure,
                token_count,
            ));
        records
            .token_usage
            .last_mut()
            .expect("record was just pushed")
            .counter_start = counter_start;
    }
    Ok(records)
}

fn json_opencode_token_records(
    span: &Value,
    attributes: Option<&Vec<Value>>,
    repo: RepoBucket,
) -> Result<UsageRecords, OtlpError> {
    let Some(model) = opencode_json_model(attributes) else {
        return Ok(UsageRecords::default());
    };
    let occurred_at = json_timestamp(span, "endTimeUnixNano").ok_or(OtlpError::MissingTimestamp)?;
    let counter_start =
        json_timestamp(span, "startTimeUnixNano").ok_or(OtlpError::MissingTimestamp)?;
    let trace_id = canonical_json_trace_id(span.get("traceId").and_then(Value::as_str))?;
    let span_id = canonical_json_span_id(span.get("spanId").and_then(Value::as_str))?;
    let mut records = UsageRecords::default();
    for (measure, keys) in opencode_token_attribute_keys() {
        let Some(token_count) = json_token_count_attribute(attributes, keys).transpose()? else {
            continue;
        };
        let event_key = trace_token_usage_event_key(
            &repo,
            model.as_str(),
            measure,
            &trace_id,
            &span_id,
            token_count,
        );
        records
            .token_usage
            .push(TokenUsageRecord::new_delta_from_signal(
                TokenUsageSignal::OpenCodeTraces,
                event_key,
                occurred_at,
                repo.clone(),
                model.clone(),
                measure,
                token_count,
            ));
        records
            .token_usage
            .last_mut()
            .expect("record was just pushed")
            .counter_start = counter_start;
    }
    Ok(records)
}

fn proto_opencode_tool_call_record(
    span: &OtlpSpan,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    if opencode_inferred_proto_span_kind(Some("opencode"), span) != Some(TraceSpanKind::ToolCall) {
        return Ok(None);
    }
    let Ok(tool_name) = proto_tool_name(&span.attributes) else {
        return Ok(None);
    };
    let occurred_at = TimestampMillis::try_from_unix_nanos(span.end_time_unix_nano)
        .ok_or(OtlpError::MissingTimestamp)?;
    let event_key = trace_tool_call_event_key(
        &repo,
        &canonical_proto_trace_id(span.trace_id.clone())?,
        &canonical_proto_span_id(span.span_id.clone())?,
        tool_name.as_str(),
    );
    Ok(Some(ToolCallRecord::new(
        event_key,
        occurred_at,
        repo,
        HarnessName::new("opencode"),
        tool_name,
    )))
}

fn json_opencode_tool_call_record(
    span: &Value,
    attributes: Option<&Vec<Value>>,
    repo: RepoBucket,
) -> Result<Option<ToolCallRecord>, OtlpError> {
    if opencode_inferred_json_span_kind(Some("opencode"), span) != Some(TraceSpanKind::ToolCall) {
        return Ok(None);
    }
    let Ok(tool_name) = json_tool_name(attributes) else {
        return Ok(None);
    };
    let occurred_at = json_timestamp(span, "endTimeUnixNano").ok_or(OtlpError::MissingTimestamp)?;
    let event_key = trace_tool_call_event_key(
        &repo,
        &canonical_json_trace_id(span.get("traceId").and_then(Value::as_str))?,
        &canonical_json_span_id(span.get("spanId").and_then(Value::as_str))?,
        tool_name.as_str(),
    );
    Ok(Some(ToolCallRecord::new(
        event_key,
        occurred_at,
        repo,
        HarnessName::new("opencode"),
        tool_name,
    )))
}

fn json_trace_span_record(
    span: &Value,
    resource_session_id: Option<SessionId>,
    resource_prompt_id: Option<PromptId>,
    resource_harness: Option<&str>,
) -> Result<Option<TraceSpanRecord>, OtlpError> {
    let attributes = span.get("attributes").and_then(Value::as_array);
    let name = span.get("name").and_then(Value::as_str).unwrap_or_default();
    let Some(kind) = json_trace_span_kind(attributes)
        .or_else(|| opencode_inferred_json_span_kind(resource_harness, span))
    else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::InvalidTraceSpanKind);
    };
    let Some(session_id) = resource_session_id.or_else(|| json_session_id(attributes)) else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::MissingSessionId);
    };
    let Some(prompt_id) = resource_prompt_id.or_else(|| json_prompt_id(attributes)) else {
        if is_opencode_context(resource_harness) {
            return Ok(None);
        }
        return Err(OtlpError::MissingPromptId);
    };
    let trace_id = canonical_json_trace_id(span.get("traceId").and_then(Value::as_str))?;
    let span_id = canonical_json_span_id(span.get("spanId").and_then(Value::as_str))?;
    let parent_span_id = span
        .get("parentSpanId")
        .and_then(Value::as_str)
        .map(canonical_json_parent_span_id)
        .transpose()?
        .flatten()
        .map(SpanId::new);
    let started_at =
        json_timestamp(span, "startTimeUnixNano").ok_or(OtlpError::MissingTimestamp)?;
    let ended_at = json_timestamp(span, "endTimeUnixNano").ok_or(OtlpError::MissingTimestamp)?;
    let tool_name = if kind == TraceSpanKind::ToolCall {
        json_trace_tool_name(attributes)
    } else {
        None
    };
    Ok(Some(TraceSpanRecord {
        harness: HarnessName::new(
            canonical_harness(resource_harness).unwrap_or("unknown".to_owned()),
        ),
        session_id,
        prompt_id,
        trace_id: TraceId::new(trace_id),
        span_id: SpanId::new(span_id),
        parent_span_id,
        kind,
        name: SpanName::new(name),
        started_at,
        ended_at,
        duration_ms: duration_ms(started_at, ended_at),
        tool_name,
    }))
}

fn duration_ms(started_at: TimestampMillis, ended_at: TimestampMillis) -> u64 {
    u64::try_from(ended_at.value().saturating_sub(started_at.value())).unwrap_or(0)
}

fn harness_from_proto_attributes(attributes: &[KeyValue]) -> Option<String> {
    first_meaningful_proto_attribute(
        attributes,
        &[
            "kvasir.harness",
            "harness",
            "service.name",
            "service.namespace",
        ],
    )
}

fn harness_from_json_attributes(attributes: Option<&Vec<Value>>) -> Option<String> {
    first_meaningful_json_attribute(
        attributes,
        &[
            "kvasir.harness",
            "harness",
            "service.name",
            "service.namespace",
        ],
    )
}

fn is_opencode_span(
    resource_harness: Option<&str>,
    span_name: &str,
    attribute: impl Fn(&str) -> Option<String>,
) -> bool {
    // OpenCode's OTLP shape is experimental and not a stable public contract.
    // Keep detection attribute-based and conservative rather than depending on a
    // single span name emitted by today's AI SDK/framework stack.
    is_opencode_context(resource_harness)
        || span_name.starts_with("opencode.")
        || attribute("opencode.span.kind").is_some()
}

fn is_opencode_context(resource_harness: Option<&str>) -> bool {
    resource_harness.is_some_and(|value| value.eq_ignore_ascii_case("opencode"))
}

fn opencode_inferred_proto_span_kind(
    resource_harness: Option<&str>,
    span: &OtlpSpan,
) -> Option<TraceSpanKind> {
    if !is_opencode_span(resource_harness, &span.name, |key| {
        proto_attribute(&span.attributes, key)
    }) {
        return None;
    }
    inferred_opencode_span_kind(&span.name, |key| proto_attribute(&span.attributes, key))
}

fn opencode_inferred_json_span_kind(
    resource_harness: Option<&str>,
    span: &Value,
) -> Option<TraceSpanKind> {
    let attributes = span.get("attributes").and_then(Value::as_array);
    let name = span.get("name").and_then(Value::as_str).unwrap_or_default();
    if !is_opencode_span(resource_harness, name, |key| {
        json_attribute(attributes, key)
    }) {
        return None;
    }
    inferred_opencode_span_kind(name, |key| json_attribute(attributes, key))
}

fn inferred_opencode_span_kind(
    span_name: &str,
    attribute: impl Fn(&str) -> Option<String>,
) -> Option<TraceSpanKind> {
    first_meaningful_attribute_from(&attribute, &["ai.operationId", "gen_ai.operation.name"])
        .and_then(|operation| {
            let operation = operation.to_ascii_lowercase();
            if operation.contains("tool") {
                Some(TraceSpanKind::ToolCall)
            } else if operation.contains("generate")
                || operation.contains("chat")
                || operation.contains("completion")
            {
                Some(TraceSpanKind::LlmRequest)
            } else {
                None
            }
        })
        .or_else(|| {
            let name = span_name.to_ascii_lowercase();
            if name.contains("tool") || name.starts_with("execute ") {
                Some(TraceSpanKind::ToolCall)
            } else if name.contains("generate")
                || name.contains("chat")
                || name.contains("completion")
            {
                Some(TraceSpanKind::LlmRequest)
            } else if name.starts_with("opencode.") {
                Some(TraceSpanKind::Interaction)
            } else {
                None
            }
        })
}

fn first_meaningful_attribute_from(
    attribute: &impl Fn(&str) -> Option<String>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| meaningful_attribute(attribute(key)))
}

fn opencode_proto_model(attributes: &[KeyValue]) -> Option<ModelName> {
    first_meaningful_proto_attribute(
        attributes,
        &[
            "ai.model.id",
            "model",
            "gen_ai.request.model",
            "gen_ai.response.model",
        ],
    )
    .map(ModelName::new)
}

fn opencode_json_model(attributes: Option<&Vec<Value>>) -> Option<ModelName> {
    first_meaningful_json_attribute(
        attributes,
        &[
            "ai.model.id",
            "model",
            "gen_ai.request.model",
            "gen_ai.response.model",
        ],
    )
    .map(ModelName::new)
}

fn opencode_token_attribute_keys() -> [(TokenMeasure, &'static [&'static str]); 3] {
    [
        (
            TokenMeasure::Input,
            &[
                "ai.usage.promptTokens",
                "ai.usage.inputTokens",
                "gen_ai.usage.input_tokens",
                "gen_ai.usage.prompt_tokens",
            ],
        ),
        (
            TokenMeasure::Output,
            &[
                "ai.usage.completionTokens",
                "ai.usage.outputTokens",
                "gen_ai.usage.output_tokens",
                "gen_ai.usage.completion_tokens",
            ],
        ),
        (
            TokenMeasure::Cache,
            &[
                "ai.usage.cachedInputTokens",
                "ai.usage.cacheReadInputTokens",
                "gen_ai.usage.cached_input_tokens",
                "gen_ai.usage.cache_read_input_tokens",
            ],
        ),
    ]
}

fn proto_session_id(attributes: &[KeyValue]) -> Option<SessionId> {
    first_proto_attribute(attributes, &["session.id", "session_id", "conversation.id"])
        .and_then(|value| meaningful_attribute(Some(value)))
        .map(SessionId::new)
}

fn proto_prompt_id(attributes: &[KeyValue]) -> Option<PromptId> {
    first_proto_attribute(attributes, &["prompt.id", "prompt_id"])
        .and_then(|value| meaningful_attribute(Some(value)))
        .map(PromptId::new)
}

fn json_session_id(attributes: Option<&Vec<Value>>) -> Option<SessionId> {
    first_json_attribute(attributes, &["session.id", "session_id", "conversation.id"])
        .and_then(|value| meaningful_attribute(Some(value)))
        .map(SessionId::new)
}

fn json_prompt_id(attributes: Option<&Vec<Value>>) -> Option<PromptId> {
    first_json_attribute(attributes, &["prompt.id", "prompt_id"])
        .and_then(|value| meaningful_attribute(Some(value)))
        .map(PromptId::new)
}

fn proto_trace_span_kind(attributes: &[KeyValue]) -> Option<TraceSpanKind> {
    TRACE_SPAN_KIND_ATTRIBUTE_KEYS
        .iter()
        .filter_map(|key| proto_attribute(attributes, key))
        .filter_map(|value| meaningful_attribute(Some(value)))
        .find_map(|value| TraceSpanKind::from_attribute(&value))
}

fn json_trace_span_kind(attributes: Option<&Vec<Value>>) -> Option<TraceSpanKind> {
    TRACE_SPAN_KIND_ATTRIBUTE_KEYS
        .iter()
        .filter_map(|key| json_attribute(attributes, key))
        .filter_map(|value| meaningful_attribute(Some(value)))
        .find_map(|value| TraceSpanKind::from_attribute(&value))
}

fn first_proto_attribute(attributes: &[KeyValue], keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| proto_attribute(attributes, key))
}

fn first_meaningful_proto_attribute(attributes: &[KeyValue], keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| meaningful_attribute(proto_attribute(attributes, key)))
}

fn first_json_attribute(attributes: Option<&Vec<Value>>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| json_attribute(attributes, key))
}

fn first_meaningful_json_attribute(
    attributes: Option<&Vec<Value>>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| meaningful_attribute(json_attribute(attributes, key)))
}

fn canonical_proto_trace_id(bytes: Vec<u8>) -> Result<String, OtlpError> {
    if bytes.is_empty() {
        return Err(OtlpError::MissingTraceId);
    }
    fixed_width_proto_hex_id(bytes, 16).ok_or(OtlpError::InvalidTraceId)
}

fn canonical_proto_span_id(bytes: Vec<u8>) -> Result<String, OtlpError> {
    if bytes.is_empty() {
        return Err(OtlpError::MissingSpanId);
    }
    fixed_width_proto_hex_id(bytes, 8).ok_or(OtlpError::InvalidSpanId)
}

fn canonical_proto_parent_span_id(bytes: Vec<u8>) -> Result<Option<String>, OtlpError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    fixed_width_proto_hex_id(bytes, 8)
        .map(Some)
        .ok_or(OtlpError::InvalidSpanId)
}

fn fixed_width_proto_hex_id(bytes: Vec<u8>, expected_len: usize) -> Option<String> {
    if bytes.len() != expected_len || bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(nibble_to_hex(byte >> 4));
        encoded.push(nibble_to_hex(byte & 0x0f));
    }
    Some(encoded)
}

fn canonical_json_trace_id(value: Option<&str>) -> Result<String, OtlpError> {
    fixed_width_json_hex_id(
        value,
        32,
        OtlpError::MissingTraceId,
        OtlpError::InvalidTraceId,
    )
}

fn canonical_json_span_id(value: Option<&str>) -> Result<String, OtlpError> {
    fixed_width_json_hex_id(
        value,
        16,
        OtlpError::MissingSpanId,
        OtlpError::InvalidSpanId,
    )
}

fn canonical_json_parent_span_id(value: &str) -> Result<Option<String>, OtlpError> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    fixed_width_json_hex_id(
        Some(value),
        16,
        OtlpError::MissingSpanId,
        OtlpError::InvalidSpanId,
    )
    .map(Some)
}

fn fixed_width_json_hex_id(
    value: Option<&str>,
    expected_len: usize,
    missing: OtlpError,
    invalid: OtlpError,
) -> Result<String, OtlpError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(missing);
    };
    if value.len() != expected_len
        || !value.chars().all(|character| character.is_ascii_hexdigit())
        || value.bytes().all(|byte| byte == b'0')
    {
        return Err(invalid);
    }
    Ok(value.to_ascii_lowercase())
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + value - 10),
        _ => unreachable!("nibble is four bits"),
    }
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

fn metric_tool_call_event_key(
    repo: &RepoBucket,
    harness: &str,
    tool_name: &str,
    counter_start_nanos: u64,
    occurred_at_nanos: u64,
    call_count: ToolCallCount,
) -> ToolCallEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-metric-tool-call");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("harness=");
    canonical.push_str(harness);
    canonical.push('\n');
    canonical.push_str("tool_name=");
    canonical.push_str(tool_name);
    canonical.push('\n');
    canonical.push_str("counter_start_nanos=");
    canonical.push_str(&counter_start_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("call_count=");
    canonical.push_str(&call_count.value().to_string());
    canonical.push('\n');
    ToolCallEventKey::new(canonical)
}

fn codex_tool_call_event_key(
    repo: &RepoBucket,
    tool_name: &str,
    occurred_at_nanos: u64,
    point_ordinal: usize,
    call_count: ToolCallCount,
) -> ToolCallEventKey {
    let mut canonical =
        codex_tool_call_point_fingerprint(repo, tool_name, occurred_at_nanos, call_count);
    canonical.push_str("point_ordinal=");
    canonical.push_str(&point_ordinal.to_string());
    canonical.push('\n');
    ToolCallEventKey::new(canonical)
}

fn codex_tool_call_point_ordinal(
    point_ordinals: &mut BTreeMap<String, usize>,
    repo: &RepoBucket,
    tool_name: &str,
    occurred_at_nanos: u64,
    call_count: ToolCallCount,
) -> usize {
    let fingerprint =
        codex_tool_call_point_fingerprint(repo, tool_name, occurred_at_nanos, call_count);
    let ordinal = point_ordinals.entry(fingerprint).or_default();
    let current = *ordinal;
    *ordinal += 1;
    current
}

fn codex_tool_call_point_fingerprint(
    repo: &RepoBucket,
    tool_name: &str,
    occurred_at_nanos: u64,
    call_count: ToolCallCount,
) -> String {
    let mut canonical = String::new();
    canonical.push_str("otlp-metric-codex-tool-call");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("tool_name=");
    canonical.push_str(tool_name);
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("call_count=");
    canonical.push_str(&call_count.value().to_string());
    canonical.push('\n');
    canonical
}

fn log_token_usage_event_key(
    repo: &RepoBucket,
    model: &str,
    measure: TokenMeasure,
    occurred_at_nanos: u64,
    token_count: TokenCount,
) -> TokenUsageEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-log-token-usage");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("model=");
    canonical.push_str(model);
    canonical.push('\n');
    canonical.push_str("measure=");
    canonical.push_str(measure.storage_name());
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("token_count=");
    canonical.push_str(&token_count.value().to_string());
    canonical.push('\n');
    TokenUsageEventKey::new(canonical)
}

fn content_event_key(
    repo: &RepoBucket,
    harness: &str,
    session_id: &SessionId,
    prompt_id: &PromptId,
    kind: ContentKind,
    occurred_at_nanos: u64,
    content: &str,
) -> ContentEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-log-content");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("harness=");
    canonical.push_str(harness);
    canonical.push('\n');
    canonical.push_str("session_id=");
    canonical.push_str(session_id.as_str());
    canonical.push('\n');
    canonical.push_str("prompt_id=");
    canonical.push_str(prompt_id.as_str());
    canonical.push('\n');
    canonical.push_str("kind=");
    canonical.push_str(kind.storage_name());
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("content_len=");
    canonical.push_str(&content.len().to_string());
    canonical.push('\n');
    canonical.push_str("content_fingerprint=");
    canonical.push_str(&content_fingerprint(content));
    canonical.push('\n');
    ContentEventKey::new(canonical)
}

fn content_fingerprint(content: &str) -> String {
    let mut forward_hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in content.as_bytes() {
        forward_hash ^= u64::from(*byte);
        forward_hash = forward_hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut reverse_hash = 0xaf63_dc4c_8601_ec8c_u64;
    for byte in content.as_bytes().iter().rev() {
        reverse_hash ^= u64::from(*byte);
        reverse_hash = reverse_hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{forward_hash:016x}{reverse_hash:016x}")
}

fn trace_token_usage_event_key(
    repo: &RepoBucket,
    model: &str,
    measure: TokenMeasure,
    trace_id: &str,
    span_id: &str,
    token_count: TokenCount,
) -> TokenUsageEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-trace-token-usage");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("model=");
    canonical.push_str(model);
    canonical.push('\n');
    canonical.push_str("measure=");
    canonical.push_str(measure.storage_name());
    canonical.push('\n');
    canonical.push_str("trace_id=");
    canonical.push_str(trace_id);
    canonical.push('\n');
    canonical.push_str("span_id=");
    canonical.push_str(span_id);
    canonical.push('\n');
    canonical.push_str("token_count=");
    canonical.push_str(&token_count.value().to_string());
    canonical.push('\n');
    TokenUsageEventKey::new(canonical)
}

fn trace_tool_call_event_key(
    repo: &RepoBucket,
    trace_id: &str,
    span_id: &str,
    tool_name: &str,
) -> ToolCallEventKey {
    let mut canonical = String::new();
    canonical.push_str("otlp-trace-tool-call");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("trace_id=");
    canonical.push_str(trace_id);
    canonical.push('\n');
    canonical.push_str("span_id=");
    canonical.push_str(span_id);
    canonical.push('\n');
    canonical.push_str("tool_name=");
    canonical.push_str(tool_name);
    canonical.push('\n');
    ToolCallEventKey::new(canonical)
}

fn codex_token_usage_event_key(
    repo: &RepoBucket,
    model: &str,
    token_type: &str,
    counter_start_nanos: u64,
    occurred_at_nanos: u64,
    point_ordinal: usize,
    token_count: TokenCount,
) -> TokenUsageEventKey {
    let mut canonical = codex_token_usage_point_fingerprint(
        repo,
        model,
        token_type,
        counter_start_nanos,
        occurred_at_nanos,
        token_count,
    );
    canonical.push_str("point_ordinal=");
    canonical.push_str(&point_ordinal.to_string());
    canonical.push('\n');
    TokenUsageEventKey::new(canonical)
}

fn codex_token_usage_point_ordinal(
    point_ordinals: &mut BTreeMap<String, usize>,
    repo: &RepoBucket,
    model: &str,
    token_type: &str,
    counter_start_nanos: u64,
    occurred_at_nanos: u64,
    token_count: TokenCount,
) -> usize {
    let fingerprint = codex_token_usage_point_fingerprint(
        repo,
        model,
        token_type,
        counter_start_nanos,
        occurred_at_nanos,
        token_count,
    );
    let ordinal = point_ordinals.entry(fingerprint).or_default();
    let current = *ordinal;
    *ordinal += 1;
    current
}

fn codex_token_usage_point_fingerprint(
    repo: &RepoBucket,
    model: &str,
    token_type: &str,
    counter_start_nanos: u64,
    occurred_at_nanos: u64,
    token_count: TokenCount,
) -> String {
    let mut canonical = String::new();
    canonical.push_str("otlp-metric-codex-turn-token-usage");
    canonical.push('\n');
    append_repo_key(&mut canonical, repo);
    canonical.push_str("model=");
    canonical.push_str(model);
    canonical.push('\n');
    canonical.push_str("token_type=");
    canonical.push_str(token_type);
    canonical.push('\n');
    canonical.push_str("counter_start_nanos=");
    canonical.push_str(&counter_start_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("occurred_at_nanos=");
    canonical.push_str(&occurred_at_nanos.to_string());
    canonical.push('\n');
    canonical.push_str("token_count=");
    canonical.push_str(&token_count.value().to_string());
    canonical.push('\n');
    canonical
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

fn canonical_harness(resource_harness: Option<&str>) -> Option<String> {
    resource_harness
        .and_then(|value| meaningful_attribute(Some(value.to_owned())))
        .map(|value| canonical_harness_name(&value))
}

fn content_record_harness(resource_harness: Option<&str>) -> Option<String> {
    canonical_harness(resource_harness).filter(|harness| content_harness_provides_logs(harness))
}

fn content_harness_provides_logs(harness: &str) -> bool {
    matches!(
        harness,
        "claude" | "claude_code" | "codex" | "github_copilot" | "opencode"
    )
}

fn is_content_event(
    event_name: Option<&str>,
    attribute_event_name: impl FnOnce() -> Option<String>,
) -> bool {
    event_name
        .and_then(|value| meaningful_attribute(Some(value.to_owned())))
        .or_else(|| meaningful_attribute(attribute_event_name()))
        .is_some_and(|value| value == "content" || value.ends_with(".content"))
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

fn proto_bool_attribute(attributes: &[KeyValue], key: &str) -> bool {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| match value.value.as_ref()? {
            any_value::Value::BoolValue(value) => Some(*value),
            any_value::Value::StringValue(value) => value.parse::<bool>().ok(),
            _ => None,
        })
        .unwrap_or(false)
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

fn json_bool_attribute(attributes: Option<&Vec<Value>>, key: &str) -> bool {
    attributes
        .and_then(|attributes| {
            attributes
                .iter()
                .find(|attribute| attribute.get("key").and_then(Value::as_str) == Some(key))
        })
        .and_then(|attribute| attribute.get("value"))
        .and_then(|value| {
            value.get("boolValue").and_then(Value::as_bool).or_else(|| {
                value
                    .get("stringValue")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<bool>().ok())
            })
        })
        .unwrap_or(false)
}

fn json_string_any_value(value: &Value) -> Option<String> {
    value
        .get("stringValue")
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
    let raw_value = match (value.get("asInt"), value.get("asDouble"), value.get("sum")) {
        (Some(value), _, _) => json_u64_value(value).ok_or(OtlpError::NumberOutOfRange)?,
        (None, Some(value), _) | (None, None, Some(value)) => {
            let Some(value) = value.as_f64() else {
                return Err(OtlpError::NumberOutOfRange);
            };
            return token_count_from_f64(value);
        }
        (None, None, None) => return Err(OtlpError::MissingTokenCount),
    };
    TokenCount::try_new(raw_value).ok_or(OtlpError::NumberOutOfRange)
}

fn json_tool_call_count(value: &Value) -> Result<ToolCallCount, OtlpError> {
    let raw_value = match (value.get("asInt"), value.get("asDouble"), value.get("sum")) {
        (Some(value), _, _) => json_u64_value(value).ok_or(OtlpError::NumberOutOfRange)?,
        (None, Some(value), _) | (None, None, Some(value)) => {
            json_u64_value(value).ok_or(OtlpError::NumberOutOfRange)?
        }
        (None, None, None) => return Err(OtlpError::MissingTokenCount),
    };
    ToolCallCount::try_new(raw_value).ok_or(OtlpError::NumberOutOfRange)
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

fn tool_call_count_from_f64(value: f64) -> Result<ToolCallCount, OtlpError> {
    let Some(value) = valid_u64_from_f64(value) else {
        return Err(
            if value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 {
                OtlpError::NonIntegralTokenCount
            } else {
                OtlpError::NumberOutOfRange
            },
        );
    };
    ToolCallCount::try_new(value).ok_or(OtlpError::NumberOutOfRange)
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
        Histogram, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric::Data,
        number_data_point::Value,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    use prost::Message;

    use super::*;
    use crate::usage::{ContentKind, TokenUsageKind};

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
    fn json_traces_reject_invalid_trace_and_span_ids() {
        assert!(matches!(
            parse_otlp_json_traces(
                trace_json_payload("aaaa", "1111111111111111", "interaction").as_bytes()
            ),
            Err(OtlpError::InvalidTraceId)
        ));
        assert!(matches!(
            parse_otlp_json_traces(
                trace_json_payload(
                    "00000000000000000000000000000000",
                    "1111111111111111",
                    "interaction"
                )
                .as_bytes()
            ),
            Err(OtlpError::InvalidTraceId)
        ));
        assert!(matches!(
            parse_otlp_json_traces(
                trace_json_payload(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "0000000000000000",
                    "interaction"
                )
                .as_bytes()
            ),
            Err(OtlpError::InvalidSpanId)
        ));
    }

    #[test]
    fn protobuf_trace_id_helpers_reject_invalid_otlp_widths_and_zeroes() {
        assert!(matches!(
            canonical_proto_trace_id(vec![1; 15]),
            Err(OtlpError::InvalidTraceId)
        ));
        assert!(matches!(
            canonical_proto_trace_id(vec![0; 16]),
            Err(OtlpError::InvalidTraceId)
        ));
        assert!(matches!(
            canonical_proto_span_id(vec![1; 7]),
            Err(OtlpError::InvalidSpanId)
        ));
        assert!(matches!(
            canonical_proto_span_id(vec![0; 8]),
            Err(OtlpError::InvalidSpanId)
        ));
    }

    #[test]
    fn json_traces_accept_span_level_session_prompt_and_canonicalize_ids() {
        let payload = r#"{
            "resourceSpans": [{
                "scopeSpans": [{
                    "spans": [{
                        "traceId": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "spanId": "1111111111111111",
                        "name": "claude.interaction",
                        "startTimeUnixNano": "1781956800000000000",
                        "endTimeUnixNano": "1781956802750000000",
                        "attributes": [
                            { "key": "conversation.id", "value": { "stringValue": " session-12 " } },
                            { "key": "prompt_id", "value": { "stringValue": " prompt-7 " } },
                            { "key": "claude.span.kind", "value": { "stringValue": "interaction" } }
                        ]
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_traces(payload.as_bytes()).expect("valid trace payload");

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].session_id.as_str(), "session-12");
        assert_eq!(records.trace_spans[0].prompt_id.as_str(), "prompt-7");
        assert_eq!(
            records.trace_spans[0].trace_id.as_str(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn json_opencode_trace_tokens_survive_missing_trace_session_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = json_opencode_trace_payload_with_resource_ids(None, Some("prompt-7"));

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 0);
        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.tool_calls.len(), 1);
        Ok(())
    }

    #[test]
    fn json_opencode_trace_tokens_survive_missing_trace_prompt_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = json_opencode_trace_payload_with_resource_ids(Some("session-12"), None);

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 0);
        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.tool_calls.len(), 1);
        Ok(())
    }

    #[test]
    fn protobuf_opencode_trace_tokens_survive_missing_trace_session_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_opencode_trace_payload_with_resource_ids(None, Some("prompt-7"));

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 0);
        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.tool_calls.len(), 1);
        Ok(())
    }

    #[test]
    fn protobuf_opencode_trace_tokens_survive_missing_trace_prompt_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_opencode_trace_payload_with_resource_ids(Some("session-12"), None);

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 0);
        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.tool_calls.len(), 1);
        Ok(())
    }

    #[test]
    fn protobuf_codex_span_kind_normalizes_to_canonical_trace_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_span_kind_key(
            "codex",
            "codex.span.kind",
            "tool",
            "tool.name",
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn protobuf_copilot_span_kind_normalizes_to_canonical_trace_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_span_kind_key(
            "github-copilot",
            "github.copilot.span.kind",
            "tool_call",
            "gen_ai.tool.name",
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn json_codex_span_kind_alias_normalizes_to_canonical_trace_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload =
            trace_json_payload_with_span_kind_key("codex.span.kind", "llm-request", "tool.name");

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::LlmRequest);
        assert_eq!(records.trace_spans[0].tool_name, None);
        Ok(())
    }

    #[test]
    fn protobuf_copilot_span_kind_alias_normalizes_to_canonical_trace_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_span_kind_key(
            "github-copilot",
            "github.copilot.span.kind",
            "tool-call",
            "gen_ai.tool.name",
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn json_codex_span_kind_rejects_unknown_value() {
        let payload =
            trace_json_payload_with_span_kind_key("codex.span.kind", "invented", "tool.name");

        assert!(matches!(
            parse_otlp_json_traces(payload.as_bytes()),
            Err(OtlpError::InvalidTraceSpanKind)
        ));
    }

    #[test]
    fn json_trace_span_kind_uses_later_valid_alias_when_first_alias_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = trace_json_payload_with_extra_attributes(
            r#"
                { "key": "codex.span.kind", "value": { "stringValue": "invented" } },
                { "key": "span.kind", "value": { "stringValue": "tool-call" } },
                { "key": "tool.name", "value": { "stringValue": "Read" } }
            "#,
        );

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn json_trace_tool_name_uses_later_valid_alias_when_first_alias_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = trace_json_payload_with_extra_attributes(
            r#"
                { "key": "codex.span.kind", "value": { "stringValue": "tool" } },
                { "key": "tool.name", "value": { "stringValue": "Unknown" } },
                { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
            "#,
        );

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn json_trace_tool_span_with_no_valid_tool_name_keeps_span_without_tool_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = trace_json_payload_with_span_kind_key("codex.span.kind", "tool", "tool.name")
            .replace(r#""Read""#, r#""Unknown""#);

        let records = parse_otlp_json_traces(payload.as_bytes())?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(records.trace_spans[0].tool_name, None);
        Ok(())
    }

    #[test]
    fn protobuf_trace_span_kind_uses_later_valid_alias_when_first_alias_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_attributes(
            "github-copilot",
            vec![
                string_attribute("github.copilot.span.kind", "invented"),
                string_attribute("span.kind", "tool-call"),
                string_attribute("gen_ai.tool.name", "Read"),
            ],
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn protobuf_trace_tool_name_uses_later_valid_alias_when_first_alias_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_attributes(
            "github-copilot",
            vec![
                string_attribute("github.copilot.span.kind", "tool-call"),
                string_attribute("tool.name", "Unknown"),
                string_attribute("gen_ai.tool.name", "Read"),
            ],
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(
            records.trace_spans[0].tool_name,
            Some(ToolName::new("Read"))
        );
        Ok(())
    }

    #[test]
    fn protobuf_trace_tool_span_with_no_valid_tool_name_keeps_span_without_tool_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_trace_payload_with_attributes(
            "github-copilot",
            vec![
                string_attribute("github.copilot.span.kind", "tool-call"),
                string_attribute("tool.name", "Unknown"),
            ],
        );

        let records = parse_otlp_protobuf_traces(&payload)?;

        assert_eq!(records.trace_spans.len(), 1);
        assert_eq!(records.trace_spans[0].kind, TraceSpanKind::ToolCall);
        assert_eq!(records.trace_spans[0].tool_name, None);
        Ok(())
    }

    #[test]
    fn protobuf_copilot_span_kind_rejects_unknown_value() {
        let payload = protobuf_trace_payload_with_span_kind_key(
            "github-copilot",
            "github.copilot.span.kind",
            "invented",
            "gen_ai.tool.name",
        );

        assert!(matches!(
            parse_otlp_protobuf_traces(&payload),
            Err(OtlpError::InvalidTraceSpanKind)
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
    fn json_codex_turn_token_usage_metrics_normalize_to_token_records() {
        let records = parse_token_usage_json(include_bytes!(
            "../tests/fixtures/codex_turn_token_usage_otlp.json"
        ))
        .expect("valid Codex token usage");
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));

        assert_eq!(
            records,
            vec![
                TokenUsageRecord::new_delta(
                    codex_token_usage_event_key(
                        &repo,
                        "gpt-5.4",
                        "input",
                        1_781_956_799_000_000_000,
                        1_781_956_800_000_000_000,
                        0,
                        TokenCount::new(1200),
                    ),
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_799_000),
                    repo.clone(),
                    ModelName::new("gpt-5.4"),
                    TokenMeasure::Input,
                    TokenCount::new(1200),
                ),
                TokenUsageRecord::new_delta(
                    codex_token_usage_event_key(
                        &repo,
                        "gpt-5.4",
                        "output",
                        1_781_956_799_000_000_000,
                        1_781_956_800_000_000_000,
                        0,
                        TokenCount::new(450),
                    ),
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_799_000),
                    repo.clone(),
                    ModelName::new("gpt-5.4"),
                    TokenMeasure::Output,
                    TokenCount::new(450),
                ),
                TokenUsageRecord::new_delta(
                    codex_token_usage_event_key(
                        &repo,
                        "gpt-5.4",
                        "cached_input",
                        1_781_956_799_000_000_000,
                        1_781_956_800_000_000_000,
                        0,
                        TokenCount::new(80),
                    ),
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_799_000),
                    repo,
                    ModelName::new("gpt-5.4"),
                    TokenMeasure::Cache,
                    TokenCount::new(80),
                ),
            ]
        );
    }

    #[test]
    fn json_copilot_token_metrics_normalize_to_cumulative_token_records() {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tokens",
                        "sum": {
                            "dataPoints": [
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781956800000000000",
                                    "asInt": "1200",
                                    "attributes": [
                                        { "key": "model", "value": { "stringValue": " " } },
                                        { "key": "gen_ai.request.model", "value": { "stringValue": "gpt-4.1" } },
                                        { "key": "token.type", "value": { "stringValue": "" } },
                                        { "key": "direction", "value": { "stringValue": "input" } }
                                    ]
                                },
                                {
                                    "startTimeUnixNano": "1781956700000000000",
                                    "timeUnixNano": "1781956800000000000",
                                    "asInt": "450",
                                    "attributes": [
                                        { "key": "gen_ai.request.model", "value": { "stringValue": "gpt-4.1" } },
                                        { "key": "gen_ai.token.type", "value": { "stringValue": "output" } }
                                    ]
                                }
                            ]
                        }
                    }]
                }]
            }]
        }"#;
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));

        assert_eq!(
            parse_token_usage_json(payload).expect("valid Copilot token usage"),
            vec![
                TokenUsageRecord::new(
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_700_000),
                    repo.clone(),
                    ModelName::new("gpt-4.1"),
                    TokenMeasure::Input,
                    TokenCount::new(1200),
                ),
                TokenUsageRecord::new(
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_700_000),
                    repo,
                    ModelName::new("gpt-4.1"),
                    TokenMeasure::Output,
                    TokenCount::new(450),
                ),
            ]
        );
    }

    #[test]
    fn json_copilot_tool_call_metrics_normalize_to_tool_call_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "github-copilot" } },
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload)?;

        assert!(records.token_usage.is_empty());
        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(
            records.tool_calls[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir"),
            ))
        );
        assert_eq!(
            records.tool_calls[0].harness,
            HarnessName::new("github_copilot")
        );
        assert_eq!(records.tool_calls[0].tool_name, ToolName::new("Read"));
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(3));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Cumulative {
                counter_start: TimestampMillis::new_for_test(1_781_956_700_000)
            }
        );

        Ok(())
    }

    #[test]
    fn json_copilot_tool_call_metrics_fall_back_to_start_when_time_is_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "0",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].occurred_at.value(), 1_781_956_700_000);
        assert!(
            records.tool_calls[0]
                .event_key
                .as_str()
                .contains("occurred_at_nanos=1781956700000000000\n")
        );

        Ok(())
    }

    #[test]
    fn json_copilot_tool_call_metrics_reject_zero_time_and_start_as_missing_timestamp() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "0",
                                "timeUnixNano": "0",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::MissingTimestamp)
        ));
    }

    #[test]
    fn json_copilot_tool_call_metrics_require_cumulative_counter_start() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
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
    fn json_copilot_tool_call_metrics_require_cumulative_sum_metadata() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidMetricKind)
        ));
    }

    #[test]
    fn json_copilot_tool_call_metrics_require_tool_name() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3"
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::MissingToolName)
        ));
    }

    #[test]
    fn json_copilot_tool_call_metrics_reject_unknown_tool_name() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "github.copilot.chat.tool_calls",
                        "sum": {
                            "aggregationTemporality": 2,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956700000000000",
                                "timeUnixNano": "1781956800000000000",
                                "asInt": "3",
                                "attributes": [
                                    { "key": "gen_ai.tool.name", "value": { "stringValue": "Unknown" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn json_codex_tool_call_metrics_without_tool_name_normalize_to_unknown_tool_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "codex" } },
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.tool.call",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 2
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].harness, HarnessName::new("codex"));
        assert_eq!(records.tool_calls[0].tool_name, ToolName::unknown());
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(2));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Delta
        );

        Ok(())
    }

    #[test]
    fn json_codex_tool_call_metrics_fall_back_to_start_when_time_is_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.tool.call",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "0",
                                "count": "1",
                                "sum": 2,
                                "attributes": [
                                    { "key": "tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_metrics(payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].occurred_at.value(), 1_781_956_799_000);
        assert!(
            records.tool_calls[0]
                .event_key
                .as_str()
                .contains("occurred_at_nanos=1781956799000000000\n")
        );

        Ok(())
    }

    #[test]
    fn json_codex_tool_call_metrics_reject_zero_time_and_start_as_missing_timestamp() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.tool.call",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "0",
                                "timeUnixNano": "0",
                                "count": "1",
                                "sum": 2,
                                "attributes": [
                                    { "key": "tool.name", "value": { "stringValue": "Read" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::MissingTimestamp)
        ));
    }

    #[test]
    fn json_codex_tool_call_metrics_reject_explicit_unknown_tool_name() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.tool.call",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 2,
                                "attributes": [
                                    { "key": "tool.name", "value": { "stringValue": "Unknown" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn protobuf_codex_turn_token_usage_metrics_normalize_to_token_records() {
        let payload = protobuf_codex_histogram_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![
                codex_histogram_point("gpt-5.4", "input", 1200.0),
                codex_histogram_point("gpt-5.4", "output", 450.0),
                codex_histogram_point("gpt-5.4", "cached_input", 80.0),
                codex_histogram_point("gpt-5.4", "total", 1730.0),
                codex_histogram_point("gpt-5.4", "reasoning_output", 120.0),
                codex_histogram_point("gpt-5.4", "tool", 25.0),
            ],
        );

        let records = parse_token_usage_protobuf(&payload).expect("valid Codex token usage");

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].measure, TokenMeasure::Input);
        assert_eq!(records[0].token_count, TokenCount::new(1200));
        assert_eq!(records[1].measure, TokenMeasure::Output);
        assert_eq!(records[1].token_count, TokenCount::new(450));
        assert_eq!(records[2].measure, TokenMeasure::Cache);
        assert_eq!(records[2].token_count, TokenCount::new(80));
        assert!(records.iter().all(|record| {
            record.repo
                == RepoBucket::repo(RepoIdentity::new(
                    RepoName::new("kvasir"),
                    RepoPath::new("/repos/kvasir"),
                ))
                && record.model == ModelName::new("gpt-5.4")
                && record.occurred_at == TimestampMillis::new_for_test(1_781_956_800_000)
                && record.counter_start == TimestampMillis::new_for_test(1_781_956_799_000)
        }));
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_normalize_to_tool_call_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![codex_tool_call_histogram_point("Read", 2.0)],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert!(records.token_usage.is_empty());
        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(
            records.tool_calls[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir"),
            ))
        );
        assert_eq!(records.tool_calls[0].harness, HarnessName::new("codex"));
        assert_eq!(records.tool_calls[0].tool_name, ToolName::new("Read"));
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(2));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Delta
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_alias_metrics_normalize_to_tool_call_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_tool_call_histogram_payload_with_metric_name(
            "codex.tool.call",
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![codex_tool_call_histogram_point("Read", 2.0)],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].harness, HarnessName::new("codex"));
        assert_eq!(records.tool_calls[0].tool_name, ToolName::new("Read"));
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(2));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Delta
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_keep_identical_points_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![
                codex_tool_call_histogram_point("Read", 2.0),
                codex_tool_call_histogram_point("Read", 2.0),
            ],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 2);
        assert_ne!(
            records.tool_calls[0].event_key,
            records.tool_calls[1].event_key
        );
        assert_eq!(
            records
                .tool_calls
                .iter()
                .map(|record| record.call_count.value())
                .sum::<u64>(),
            4
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_split_metrics_keep_identical_points_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_split_tool_call_histogram_payload(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            codex_tool_call_histogram_point("Read", 2.0),
            codex_tool_call_histogram_point("Read", 2.0),
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 2);
        assert_ne!(
            records.tool_calls[0].event_key,
            records.tool_calls[1].event_key
        );
        assert_eq!(
            records
                .tool_calls
                .iter()
                .map(|record| record.call_count.value())
                .sum::<u64>(),
            4
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_without_tool_name_normalize_to_unknown_tool_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![HistogramDataPoint {
                attributes: Vec::new(),
                start_time_unix_nano: 1_781_956_799_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                count: 1,
                sum: Some(2.0),
                bucket_counts: Vec::new(),
                explicit_bounds: Vec::new(),
                exemplars: Vec::new(),
                flags: 0,
                min: None,
                max: None,
            }],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].harness, HarnessName::new("codex"));
        assert_eq!(records.tool_calls[0].tool_name, ToolName::unknown());
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(2));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Delta
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_fall_back_to_start_when_time_is_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            Vec::new(),
            vec![HistogramDataPoint {
                attributes: vec![string_attribute("tool.name", "Read")],
                start_time_unix_nano: 1_781_956_799_000_000_000,
                time_unix_nano: 0,
                count: 1,
                sum: Some(2.0),
                bucket_counts: Vec::new(),
                explicit_bounds: Vec::new(),
                exemplars: Vec::new(),
                flags: 0,
                min: None,
                max: None,
            }],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].occurred_at.value(), 1_781_956_799_000);
        assert!(
            records.tool_calls[0]
                .event_key
                .as_str()
                .contains("occurred_at_nanos=1781956799000000000\n")
        );

        Ok(())
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_reject_zero_time_and_start_as_missing_timestamp() {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            Vec::new(),
            vec![HistogramDataPoint {
                attributes: vec![string_attribute("tool.name", "Read")],
                start_time_unix_nano: 0,
                time_unix_nano: 0,
                count: 1,
                sum: Some(2.0),
                bucket_counts: Vec::new(),
                explicit_bounds: Vec::new(),
                exemplars: Vec::new(),
                flags: 0,
                min: None,
                max: None,
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingTimestamp)
        ));
    }

    #[test]
    fn protobuf_copilot_token_metrics_normalize_to_cumulative_token_records() {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tokens",
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![
                NumberDataPoint {
                    attributes: vec![
                        string_attribute("model", " "),
                        string_attribute("gen_ai.request.model", "gpt-4.1"),
                        string_attribute("token.type", ""),
                        string_attribute("direction", "input"),
                    ],
                    start_time_unix_nano: 1_781_956_700_000_000_000,
                    time_unix_nano: 1_781_956_800_000_000_000,
                    exemplars: Vec::new(),
                    flags: 0,
                    value: Some(Value::AsInt(1200)),
                },
                NumberDataPoint {
                    attributes: vec![
                        string_attribute("gen_ai.request.model", "gpt-4.1"),
                        string_attribute("gen_ai.token.type", "output"),
                    ],
                    start_time_unix_nano: 1_781_956_700_000_000_000,
                    time_unix_nano: 1_781_956_800_000_000_000,
                    exemplars: Vec::new(),
                    flags: 0,
                    value: Some(Value::AsInt(450)),
                },
            ],
        );
        let repo = RepoBucket::repo(RepoIdentity::new(
            RepoName::new("kvasir"),
            RepoPath::new("/repos/kvasir"),
        ));

        assert_eq!(
            parse_token_usage_protobuf(&payload).expect("valid Copilot token usage"),
            vec![
                TokenUsageRecord::new(
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_700_000),
                    repo.clone(),
                    ModelName::new("gpt-4.1"),
                    TokenMeasure::Input,
                    TokenCount::new(1200),
                ),
                TokenUsageRecord::new(
                    TimestampMillis::new_for_test(1_781_956_800_000),
                    TimestampMillis::new_for_test(1_781_956_700_000),
                    repo,
                    ModelName::new("gpt-4.1"),
                    TokenMeasure::Output,
                    TokenCount::new(450),
                ),
            ]
        );
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_normalize_to_tool_call_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            vec![
                string_attribute("service.name", "github-copilot"),
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![NumberDataPoint {
                attributes: vec![string_attribute("gen_ai.client.tool.name", "Read")],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert!(records.token_usage.is_empty());
        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(
            records.tool_calls[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir"),
            ))
        );
        assert_eq!(
            records.tool_calls[0].harness,
            HarnessName::new("github_copilot")
        );
        assert_eq!(records.tool_calls[0].tool_name, ToolName::new("Read"));
        assert_eq!(records.tool_calls[0].call_count, ToolCallCount::new(3));
        assert_eq!(
            records.tool_calls[0].kind,
            crate::usage::ToolCallKind::Cumulative {
                counter_start: TimestampMillis::new_for_test(1_781_956_700_000)
            }
        );

        Ok(())
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_fall_back_to_start_when_time_is_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            Vec::new(),
            vec![NumberDataPoint {
                attributes: vec![string_attribute("gen_ai.tool.name", "Read")],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 0,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        let records = parse_otlp_protobuf_usage_metrics(&payload)?;

        assert_eq!(records.tool_calls.len(), 1);
        assert_eq!(records.tool_calls[0].occurred_at.value(), 1_781_956_700_000);
        assert!(
            records.tool_calls[0]
                .event_key
                .as_str()
                .contains("occurred_at_nanos=1781956700000000000\n")
        );

        Ok(())
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_reject_zero_time_and_start_as_missing_timestamp() {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            Vec::new(),
            vec![NumberDataPoint {
                attributes: vec![string_attribute("gen_ai.tool.name", "Read")],
                start_time_unix_nano: 0,
                time_unix_nano: 0,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingTimestamp)
        ));
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_require_cumulative_counter_start() {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            Vec::new(),
            vec![NumberDataPoint {
                attributes: vec![string_attribute("gen_ai.tool.name", "Read")],
                start_time_unix_nano: 0,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingCounterStart)
        ));
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_require_tool_name() {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            Vec::new(),
            vec![NumberDataPoint {
                attributes: Vec::new(),
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingToolName)
        ));
    }

    #[test]
    fn protobuf_copilot_tool_call_metrics_reject_unknown_tool_name() {
        let payload = protobuf_payload_with_metric_name_and_resource_attributes(
            "github.copilot.chat.tool_calls",
            Vec::new(),
            vec![NumberDataPoint {
                attributes: vec![string_attribute("gen_ai.tool.name", "Unknown")],
                start_time_unix_nano: 1_781_956_700_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                exemplars: Vec::new(),
                flags: 0,
                value: Some(Value::AsInt(3)),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn protobuf_codex_tool_call_metrics_reject_explicit_unknown_tool_name() {
        let payload = protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
            Vec::new(),
            vec![HistogramDataPoint {
                attributes: vec![string_attribute("tool.name", "Unknown")],
                start_time_unix_nano: 1_781_956_799_000_000_000,
                time_unix_nano: 1_781_956_800_000_000_000,
                count: 1,
                sum: Some(2.0),
                bucket_counts: Vec::new(),
                explicit_bounds: Vec::new(),
                exemplars: Vec::new(),
                flags: 0,
                min: None,
                max: None,
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn json_codex_token_usage_rejects_unknown_token_types() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.token_usage",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 12,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token_type", "value": { "stringValue": "new_billable_bucket" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidMeasure)
        ));
    }

    #[test]
    fn protobuf_codex_token_usage_rejects_unknown_token_types() {
        let payload = protobuf_codex_histogram_payload_with_resource_attributes(
            Vec::new(),
            vec![codex_histogram_point(
                "gpt-5.4",
                "new_billable_bucket",
                12.0,
            )],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::InvalidMeasure)
        ));
    }

    #[test]
    fn json_codex_token_usage_rejects_fractional_histogram_sum() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.token_usage",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "1781956799000000000",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 12.5,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token_type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::NonIntegralTokenCount)
        ));
    }

    #[test]
    fn json_codex_token_usage_rejects_non_delta_histograms() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.token_usage",
                        "histogram": {
                            "aggregationTemporality": 2,
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 12,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token_type", "value": { "stringValue": "input" } }
                                ]
                            }]
                        }
                    }]
                }]
            }]
        }"#;

        assert!(matches!(
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidMetricKind)
        ));
    }

    #[test]
    fn json_codex_token_usage_rejects_missing_counter_start() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.token_usage",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 12,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token_type", "value": { "stringValue": "input" } }
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
    fn json_codex_token_usage_rejects_zero_counter_start() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "codex.turn.token_usage",
                        "histogram": {
                            "aggregationTemporality": 1,
                            "dataPoints": [{
                                "startTimeUnixNano": "0",
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 12,
                                "attributes": [
                                    { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                                    { "key": "token_type", "value": { "stringValue": "input" } }
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
    fn protobuf_codex_token_usage_rejects_missing_histogram_sum() {
        let mut data_point = codex_histogram_point("gpt-5.4", "input", 12.0);
        data_point.sum = None;
        let payload =
            protobuf_codex_histogram_payload_with_resource_attributes(Vec::new(), vec![data_point]);

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingTokenCount)
        ));
    }

    #[test]
    fn protobuf_codex_token_usage_rejects_missing_counter_start() {
        let mut data_point = codex_histogram_point("gpt-5.4", "input", 12.0);
        data_point.start_time_unix_nano = 0;
        let payload =
            protobuf_codex_histogram_payload_with_resource_attributes(Vec::new(), vec![data_point]);

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::MissingCounterStart)
        ));
    }

    #[test]
    fn protobuf_codex_token_usage_rejects_non_delta_histograms() {
        let payload = protobuf_codex_histogram_payload_with_temporality(
            Vec::new(),
            vec![codex_histogram_point("gpt-5.4", "input", 12.0)],
            2,
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
            Err(OtlpError::InvalidMetricKind)
        ));
    }

    #[test]
    fn protobuf_codex_token_usage_rejects_fractional_histogram_sum() {
        let payload = protobuf_codex_histogram_payload_with_resource_attributes(
            Vec::new(),
            vec![codex_histogram_point("gpt-5.4", "input", 12.5)],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_metrics(&payload),
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
    fn json_rejects_legacy_token_usage_with_histogram_metric_kind() {
        let payload = br#"{
            "resourceMetrics": [{
                "scopeMetrics": [{
                    "metrics": [{
                        "name": "token.usage",
                        "histogram": {
                            "dataPoints": [{
                                "timeUnixNano": "1781956800000000000",
                                "count": "1",
                                "sum": 100,
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
            parse_otlp_json_usage_metrics(payload),
            Err(OtlpError::InvalidMetricKind)
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
    fn json_tool_result_rejects_unknown_sentinel_tool_name() {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "tool_result",
                        "attributes": [
                            { "key": "tool.name", "value": { "stringValue": "Unknown" } }
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
    fn json_usage_logs_normalize_token_usage_records() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "resource": {
                    "attributes": [
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "token_usage",
                        "body": { "intValue": "1200" },
                        "attributes": [
                            { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                            { "key": "token.type", "value": { "stringValue": "input" } }
                        ]
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.token_usage[0].signal, TokenUsageSignal::Logs);
        assert_eq!(records.token_usage[0].token_count, TokenCount::new(1200));
        assert!(matches!(
            records.token_usage[0].kind,
            TokenUsageKind::Delta { .. }
        ));
        assert_eq!(
            records.token_usage[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir")
            ))
        );

        Ok(())
    }

    #[test]
    fn json_opencode_content_logs_require_opt_in_and_normalize_content()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "OpenCode" } },
                        { "key": "repo.name", "value": { "stringValue": "kvasir" } },
                        { "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } },
                        { "key": "session.id", "value": { "stringValue": "session-12" } },
                        { "key": "prompt.id", "value": { "stringValue": "prompt-7" } }
                    ]
                },
                "scopeLogs": [{
                    "logRecords": [
                        {
                            "timeUnixNano": "1781956800000000000",
                            "body": { "stringValue": "stored assistant text" },
                            "attributes": [
                                { "key": "event.name", "value": { "stringValue": " opencode.content " } },
                                { "key": "content.opt_in", "value": { "boolValue": true } },
                                { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                            ]
                        },
                        {
                            "timeUnixNano": "1781956801000000000",
                            "eventName": "opencode.content",
                            "body": { "stringValue": "not opted in" },
                            "attributes": [
                                { "key": "content.type", "value": { "stringValue": "assistant_message" } }
                            ]
                        }
                    ]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert_eq!(records.content.len(), 1);
        assert_eq!(records.content[0].harness, HarnessName::new("opencode"));
        assert_eq!(records.content[0].session_id, SessionId::new("session-12"));
        assert_eq!(records.content[0].prompt_id, PromptId::new("prompt-7"));
        assert_eq!(records.content[0].kind, ContentKind::AssistantMessage);
        assert_eq!(records.content[0].content.as_str(), "stored assistant text");
        assert_eq!(
            records.content[0].repo,
            RepoBucket::repo(RepoIdentity::new(
                RepoName::new("kvasir"),
                RepoPath::new("/repos/kvasir"),
            ))
        );

        Ok(())
    }

    #[test]
    fn json_usage_logs_use_counter_start_when_present() -> Result<(), Box<dyn std::error::Error>> {
        let payload = br#"{
            "resourceLogs": [{
                "scopeLogs": [{
                    "logRecords": [{
                        "timeUnixNano": "1781956800000000000",
                        "eventName": "token_usage",
                        "body": { "intValue": "1200" },
                        "attributes": [
                            { "key": "model", "value": { "stringValue": "gpt-5.4" } },
                            { "key": "token.type", "value": { "stringValue": "input" } },
                            { "key": "counter_start_unix_nano", "value": { "stringValue": "1781956799000000000" } }
                        ]
                    }]
                }]
            }]
        }"#;

        let records = parse_otlp_json_usage_logs(payload)?;

        assert_eq!(
            records.token_usage[0].counter_start,
            TimestampMillis::new_for_test(1_781_956_799_000)
        );
        assert_eq!(records.token_usage[0].kind, TokenUsageKind::Cumulative);

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
    fn protobuf_usage_logs_normalize_token_usage_records() -> Result<(), Box<dyn std::error::Error>>
    {
        let payload = protobuf_logs_payload_with_resource_attributes(
            vec![
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
            ],
            vec![LogRecord {
                time_unix_nano: 1_781_956_800_000_000_000,
                observed_time_unix_nano: 0,
                severity_number: 0,
                severity_text: String::new(),
                body: Some(AnyValue {
                    value: Some(any_value::Value::IntValue(1200)),
                }),
                attributes: vec![
                    string_attribute("model", "gpt-5.4"),
                    string_attribute("token.type", "input"),
                ],
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: Vec::new(),
                span_id: Vec::new(),
                event_name: "token_usage".to_owned(),
            }],
        );

        let records = parse_otlp_protobuf_usage_logs(&payload)?;

        assert_eq!(records.token_usage.len(), 1);
        assert_eq!(records.token_usage[0].signal, TokenUsageSignal::Logs);
        assert_eq!(records.token_usage[0].token_count, TokenCount::new(1200));
        assert!(matches!(
            records.token_usage[0].kind,
            TokenUsageKind::Delta { .. }
        ));

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
    fn protobuf_tool_result_rejects_unknown_sentinel_tool_name() {
        let payload = protobuf_logs_payload_with_resource_attributes(
            Vec::new(),
            vec![LogRecord {
                time_unix_nano: 1_781_956_800_000_000_000,
                observed_time_unix_nano: 0,
                severity_number: 0,
                severity_text: String::new(),
                body: None,
                attributes: vec![string_attribute("tool.name", "Unknown")],
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: Vec::new(),
                span_id: Vec::new(),
                event_name: "tool_result".to_owned(),
            }],
        );

        assert!(matches!(
            parse_otlp_protobuf_usage_logs(&payload),
            Err(OtlpError::InvalidToolName)
        ));
    }

    #[test]
    fn protobuf_opencode_content_logs_require_opt_in_and_reject_raw_api_payloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = protobuf_logs_payload_with_resource_attributes(
            vec![
                string_attribute("service.name", "opencode"),
                string_attribute("repo.name", "kvasir"),
                string_attribute("repo.path", "/repos/kvasir"),
                string_attribute("session.id", "session-12"),
                string_attribute("prompt.id", "prompt-7"),
            ],
            vec![
                LogRecord {
                    time_unix_nano: 1_781_956_800_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(
                            "stored assistant text".to_owned(),
                        )),
                    }),
                    attributes: vec![
                        string_attribute("event.name", " opencode.content "),
                        bool_attribute("content.opt_in", true),
                        string_attribute("content.type", "assistant_message"),
                    ],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: String::new(),
                },
                LogRecord {
                    time_unix_nano: 1_781_956_801_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("not opted in".to_owned())),
                    }),
                    attributes: vec![string_attribute("content.type", "assistant_message")],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "opencode.content".to_owned(),
                },
                LogRecord {
                    time_unix_nano: 1_781_956_802_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(
                            "authorization: bearer secret".to_owned(),
                        )),
                    }),
                    attributes: vec![
                        bool_attribute("content.opt_in", true),
                        string_attribute("content.type", "raw_api_request"),
                    ],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "opencode.content".to_owned(),
                },
            ],
        );

        let records = parse_otlp_protobuf_usage_logs(&payload)?;

        assert_eq!(records.content.len(), 1);
        assert_eq!(records.content[0].harness, HarnessName::new("opencode"));
        assert_eq!(records.content[0].session_id, SessionId::new("session-12"));
        assert_eq!(records.content[0].prompt_id, PromptId::new("prompt-7"));
        assert_eq!(records.content[0].kind, ContentKind::AssistantMessage);
        assert_eq!(records.content[0].content.as_str(), "stored assistant text");

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
        protobuf_payload_with_metric_name_and_resource_attributes(
            "token.usage",
            resource_attributes,
            data_points,
        )
    }

    fn protobuf_payload_with_metric_name_and_resource_attributes(
        metric_name: &str,
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
                        name: metric_name.to_owned(),
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

    fn protobuf_codex_histogram_payload_with_resource_attributes(
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<HistogramDataPoint>,
    ) -> Vec<u8> {
        protobuf_codex_histogram_payload_with_temporality(resource_attributes, data_points, 1)
    }

    fn protobuf_codex_histogram_payload_with_temporality(
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<HistogramDataPoint>,
        aggregation_temporality: i32,
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
                        name: "codex.turn.token_usage".to_owned(),
                        description: String::new(),
                        unit: "{token}".to_owned(),
                        metadata: Vec::new(),
                        data: Some(Data::Histogram(Histogram {
                            data_points,
                            aggregation_temporality,
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn protobuf_codex_tool_call_histogram_payload_with_resource_attributes(
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<HistogramDataPoint>,
    ) -> Vec<u8> {
        protobuf_codex_tool_call_histogram_payload_with_metric_name(
            "codex.turn.tool.call",
            resource_attributes,
            data_points,
        )
    }

    fn protobuf_codex_tool_call_histogram_payload_with_metric_name(
        metric_name: &str,
        resource_attributes: Vec<KeyValue>,
        data_points: Vec<HistogramDataPoint>,
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
                        name: metric_name.to_owned(),
                        description: String::new(),
                        unit: "{call}".to_owned(),
                        metadata: Vec::new(),
                        data: Some(Data::Histogram(Histogram {
                            data_points,
                            aggregation_temporality: 1,
                        })),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn protobuf_codex_split_tool_call_histogram_payload(
        resource_attributes: Vec<KeyValue>,
        first: HistogramDataPoint,
        second: HistogramDataPoint,
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
                    metrics: vec![
                        Metric {
                            name: "codex.turn.tool.call".to_owned(),
                            description: String::new(),
                            unit: "{call}".to_owned(),
                            metadata: Vec::new(),
                            data: Some(Data::Histogram(Histogram {
                                data_points: vec![first],
                                aggregation_temporality: 1,
                            })),
                        },
                        Metric {
                            name: "codex.turn.tool.call".to_owned(),
                            description: String::new(),
                            unit: "{call}".to_owned(),
                            metadata: Vec::new(),
                            data: Some(Data::Histogram(Histogram {
                                data_points: vec![second],
                                aggregation_temporality: 1,
                            })),
                        },
                    ],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn codex_histogram_point(model: &str, token_type: &str, sum: f64) -> HistogramDataPoint {
        HistogramDataPoint {
            attributes: vec![
                string_attribute("model", model),
                string_attribute("token_type", token_type),
            ],
            start_time_unix_nano: 1_781_956_799_000_000_000,
            time_unix_nano: 1_781_956_800_000_000_000,
            count: 1,
            sum: Some(sum),
            bucket_counts: Vec::new(),
            explicit_bounds: Vec::new(),
            exemplars: Vec::new(),
            flags: 0,
            min: None,
            max: None,
        }
    }

    fn codex_tool_call_histogram_point(tool_name: &str, sum: f64) -> HistogramDataPoint {
        HistogramDataPoint {
            attributes: vec![string_attribute("tool.name", tool_name)],
            start_time_unix_nano: 1_781_956_799_000_000_000,
            time_unix_nano: 1_781_956_800_000_000_000,
            count: 1,
            sum: Some(sum),
            bucket_counts: Vec::new(),
            explicit_bounds: Vec::new(),
            exemplars: Vec::new(),
            flags: 0,
            min: None,
            max: None,
        }
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

    fn bool_attribute(key: &str, value: bool) -> KeyValue {
        KeyValue {
            key: key.to_owned(),
            key_strindex: 0,
            value: Some(AnyValue {
                value: Some(any_value::Value::BoolValue(value)),
            }),
        }
    }

    fn int_attribute(key: &str, value: i64) -> KeyValue {
        KeyValue {
            key: key.to_owned(),
            key_strindex: 0,
            value: Some(AnyValue {
                value: Some(any_value::Value::IntValue(value)),
            }),
        }
    }

    fn trace_json_payload(trace_id: &str, span_id: &str, kind: &str) -> String {
        format!(
            r#"{{
                "resourceSpans": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "session.id", "value": {{ "stringValue": "session-12" }} }},
                            {{ "key": "prompt.id", "value": {{ "stringValue": "prompt-7" }} }}
                        ]
                    }},
                    "scopeSpans": [{{
                        "spans": [{{
                            "traceId": "{trace_id}",
                            "spanId": "{span_id}",
                            "name": "claude.interaction",
                            "startTimeUnixNano": "1781956800000000000",
                            "endTimeUnixNano": "1781956802750000000",
                            "attributes": [
                                {{ "key": "claude.span.kind", "value": {{ "stringValue": "{kind}" }} }}
                            ]
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn trace_json_payload_with_span_kind_key(
        span_kind_key: &str,
        span_kind: &str,
        tool_name_key: &str,
    ) -> String {
        format!(
            r#"{{
                "resourceSpans": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "codex" }} }},
                            {{ "key": "session.id", "value": {{ "stringValue": "session-12" }} }},
                            {{ "key": "prompt.id", "value": {{ "stringValue": "prompt-7" }} }}
                        ]
                    }},
                    "scopeSpans": [{{
                        "spans": [{{
                            "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                            "spanId": "1111111111111111",
                            "name": "codex.span",
                            "startTimeUnixNano": "1781956800000000000",
                            "endTimeUnixNano": "1781956800100000000",
                            "attributes": [
                                {{ "key": "{span_kind_key}", "value": {{ "stringValue": "{span_kind}" }} }},
                                {{ "key": "{tool_name_key}", "value": {{ "stringValue": "Read" }} }}
                            ]
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn trace_json_payload_with_extra_attributes(attributes: &str) -> String {
        format!(
            r#"{{
                "resourceSpans": [{{
                    "resource": {{
                        "attributes": [
                            {{ "key": "service.name", "value": {{ "stringValue": "codex" }} }},
                            {{ "key": "session.id", "value": {{ "stringValue": "session-12" }} }},
                            {{ "key": "prompt.id", "value": {{ "stringValue": "prompt-7" }} }}
                        ]
                    }},
                    "scopeSpans": [{{
                        "spans": [{{
                            "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                            "spanId": "1111111111111111",
                            "name": "codex.span",
                            "startTimeUnixNano": "1781956800000000000",
                            "endTimeUnixNano": "1781956800100000000",
                            "attributes": [
                                {attributes}
                            ]
                        }}]
                    }}]
                }}]
            }}"#
        )
    }

    fn json_opencode_trace_payload_with_resource_ids(
        session_id: Option<&str>,
        prompt_id: Option<&str>,
    ) -> String {
        let mut attributes = vec![
            r#"{ "key": "service.name", "value": { "stringValue": "opencode" } }"#.to_owned(),
            r#"{ "key": "repo.name", "value": { "stringValue": "kvasir" } }"#.to_owned(),
            r#"{ "key": "repo.path", "value": { "stringValue": "/repos/kvasir" } }"#.to_owned(),
        ];
        if let Some(session_id) = session_id {
            attributes.push(format!(
                r#"{{ "key": "session.id", "value": {{ "stringValue": "{session_id}" }} }}"#
            ));
        }
        if let Some(prompt_id) = prompt_id {
            attributes.push(format!(
                r#"{{ "key": "prompt.id", "value": {{ "stringValue": "{prompt_id}" }} }}"#
            ));
        }
        format!(
            r#"{{
                "resourceSpans": [{{
                    "resource": {{
                        "attributes": [{}]
                    }},
                    "scopeSpans": [{{
                        "spans": [
                            {{
                                "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                                "spanId": "1111111111111111",
                                "name": "opencode.generate",
                                "startTimeUnixNano": "1781956800000000000",
                                "endTimeUnixNano": "1781956801800000000",
                                "attributes": [
                                    {{ "key": "ai.operationId", "value": {{ "stringValue": "chat" }} }},
                                    {{ "key": "ai.model.id", "value": {{ "stringValue": "gpt-4.1" }} }},
                                    {{ "key": "ai.usage.promptTokens", "value": {{ "intValue": "100" }} }}
                                ]
                            }},
                            {{
                                "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                                "spanId": "2222222222222222",
                                "parentSpanId": "1111111111111111",
                                "name": "opencode.tool",
                                "startTimeUnixNano": "1781956801800000000",
                                "endTimeUnixNano": "1781956801900000000",
                                "attributes": [
                                    {{ "key": "ai.operationId", "value": {{ "stringValue": "tool" }} }},
                                    {{ "key": "tool.name", "value": {{ "stringValue": "Read" }} }}
                                ]
                            }}
                        ]
                    }}]
                }}]
            }}"#,
            attributes.join(",")
        )
    }

    fn protobuf_trace_payload_with_span_kind_key(
        harness: &str,
        span_kind_key: &str,
        span_kind: &str,
        tool_name_key: &str,
    ) -> Vec<u8> {
        protobuf_trace_payload_with_attributes(
            harness,
            vec![
                string_attribute(span_kind_key, span_kind),
                string_attribute(tool_name_key, "Read"),
            ],
        )
    }

    fn protobuf_trace_payload_with_attributes(harness: &str, attributes: Vec<KeyValue>) -> Vec<u8> {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![
                        string_attribute("service.name", harness),
                        string_attribute("session.id", "session-12"),
                        string_attribute("prompt.id", "prompt-7"),
                    ],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![Span {
                        trace_id: vec![0xaa; 16],
                        span_id: vec![0x11; 8],
                        trace_state: String::new(),
                        parent_span_id: Vec::new(),
                        flags: 0,
                        name: format!("{harness}.span"),
                        kind: 0,
                        start_time_unix_nano: 1_781_956_800_000_000_000,
                        end_time_unix_nano: 1_781_956_800_100_000_000,
                        attributes,
                        dropped_attributes_count: 0,
                        events: Vec::new(),
                        dropped_events_count: 0,
                        links: Vec::new(),
                        dropped_links_count: 0,
                        status: None,
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn protobuf_opencode_trace_payload_with_resource_ids(
        session_id: Option<&str>,
        prompt_id: Option<&str>,
    ) -> Vec<u8> {
        let mut resource_attributes = vec![
            string_attribute("service.name", "opencode"),
            string_attribute("repo.name", "kvasir"),
            string_attribute("repo.path", "/repos/kvasir"),
        ];
        if let Some(session_id) = session_id {
            resource_attributes.push(string_attribute("session.id", session_id));
        }
        if let Some(prompt_id) = prompt_id {
            resource_attributes.push(string_attribute("prompt.id", prompt_id));
        }
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: resource_attributes,
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![
                        Span {
                            trace_id: vec![0xaa; 16],
                            span_id: vec![0x11; 8],
                            trace_state: String::new(),
                            parent_span_id: Vec::new(),
                            flags: 0,
                            name: "opencode.generate".to_owned(),
                            kind: 0,
                            start_time_unix_nano: 1_781_956_800_000_000_000,
                            end_time_unix_nano: 1_781_956_801_800_000_000,
                            attributes: vec![
                                string_attribute("ai.operationId", "chat"),
                                string_attribute("ai.model.id", "gpt-4.1"),
                                int_attribute("ai.usage.promptTokens", 100),
                            ],
                            dropped_attributes_count: 0,
                            events: Vec::new(),
                            dropped_events_count: 0,
                            links: Vec::new(),
                            dropped_links_count: 0,
                            status: None,
                        },
                        Span {
                            trace_id: vec![0xaa; 16],
                            span_id: vec![0x22; 8],
                            trace_state: String::new(),
                            parent_span_id: vec![0x11; 8],
                            flags: 0,
                            name: "opencode.tool".to_owned(),
                            kind: 0,
                            start_time_unix_nano: 1_781_956_801_800_000_000,
                            end_time_unix_nano: 1_781_956_801_900_000_000,
                            attributes: vec![
                                string_attribute("ai.operationId", "tool"),
                                string_attribute("tool.name", "Read"),
                            ],
                            dropped_attributes_count: 0,
                            events: Vec::new(),
                            dropped_events_count: 0,
                            links: Vec::new(),
                            dropped_links_count: 0,
                            status: None,
                        },
                    ],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }
}
