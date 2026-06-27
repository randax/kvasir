use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;

use kvasir_core::rpc::{RpcRequest, RpcResponse, RpcStreamEvent};

use crate::error::KvasirClientError;
use crate::transport::{connect_with_retries, read_rpc_stream_event, send_rpc_request};
use crate::types::{
    KvasirContentQuery, KvasirContentReplay, KvasirCostRollup, KvasirOverviewRollup,
    KvasirOverviewSnapshot, KvasirRollupQuery, KvasirSocketPath, KvasirTokenRollup,
    KvasirTokenRollupUpdate, KvasirToolCallRollup, KvasirTrace, KvasirTraceQuery,
    KvasirUsageUpdateKind,
};

#[derive(Debug, uniffi::Object)]
pub struct KvasirClient {
    socket_path: PathBuf,
}

#[derive(uniffi::Object)]
pub struct KvasirTokenRollupSubscription {
    reader: Mutex<BufReader<UnixStream>>,
}

#[derive(uniffi::Object)]
pub struct KvasirUsageUpdateSubscription {
    reader: Mutex<Option<BufReader<UnixStream>>>,
    shutdown_stream: Mutex<Option<UnixStream>>,
}

#[uniffi::export]
impl KvasirClient {
    #[uniffi::constructor]
    pub fn connect(socket_path: KvasirSocketPath) -> Result<Self, KvasirClientError> {
        let socket_path = PathBuf::from(socket_path.into_string());
        connect_with_retries(&socket_path)?;
        Ok(Self { socket_path })
    }

    pub fn token_rollups(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<Vec<KvasirTokenRollup>, KvasirClientError> {
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::TokenRollup {
                query: query.try_into()?,
            },
        )?;
        match response {
            RpcResponse::TokenRollup { rollups } => rollups
                .into_iter()
                .map(KvasirTokenRollup::try_from)
                .collect::<Result<Vec<_>, _>>(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn overview_rollups(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<KvasirOverviewRollup, KvasirClientError> {
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::OverviewRollup {
                query: query.try_into()?,
            },
        )?;
        match response {
            RpcResponse::OverviewRollup { rollup } => rollup.try_into(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn overview_snapshot(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<KvasirOverviewSnapshot, KvasirClientError> {
        let selected_repo = query.repo.clone();
        let selected_harness = query
            .prompt
            .as_ref()
            .map(|prompt| prompt.session.harness.clone())
            .or_else(|| {
                query
                    .session
                    .as_ref()
                    .map(|session| session.harness.clone())
            })
            .or_else(|| query.harness.clone());
        let selected_model = query.model.clone();
        let selected_session = query.session.clone();
        let selected_prompt = query.prompt.clone();
        self.overview_rollups(query).map(|rollup| {
            KvasirOverviewSnapshot::from_rollup(
                rollup,
                selected_repo,
                selected_harness,
                selected_model,
                selected_session,
                selected_prompt,
            )
        })
    }

    pub fn cost_rollups(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<Vec<KvasirCostRollup>, KvasirClientError> {
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::CostRollup {
                query: query.try_into()?,
            },
        )?;
        match response {
            RpcResponse::CostRollup { rollups } => rollups
                .into_iter()
                .map(KvasirCostRollup::try_from)
                .collect::<Result<Vec<_>, _>>(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn tool_call_rollups(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<Vec<KvasirToolCallRollup>, KvasirClientError> {
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::ToolCallRollup {
                query: query.try_into()?,
            },
        )?;
        match response {
            RpcResponse::ToolCallRollup { rollups } => rollups
                .into_iter()
                .map(KvasirToolCallRollup::try_from)
                .collect::<Result<Vec<_>, _>>(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn trace(&self, query: KvasirTraceQuery) -> Result<Vec<KvasirTrace>, KvasirClientError> {
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::Trace {
                query: query.into(),
            },
        )?;
        match response {
            RpcResponse::Trace { traces } => traces
                .into_iter()
                .map(KvasirTrace::try_from)
                .collect::<Result<Vec<_>, _>>(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn content_replay(
        &self,
        query: KvasirContentQuery,
    ) -> Result<KvasirContentReplay, KvasirClientError> {
        let (query, bearer_token) = query.into();
        let response = send_rpc_request(
            &self.socket_path,
            RpcRequest::Content {
                query,
                bearer_token,
            },
        )?;
        match response {
            RpcResponse::Content { replay } => replay.try_into(),
            RpcResponse::Error { error } => Err(error.into()),
            _ => Err(KvasirClientError::WrongResponseType),
        }
    }

    pub fn subscribe_token_rollups(
        &self,
        query: KvasirRollupQuery,
    ) -> Result<KvasirTokenRollupSubscription, KvasirClientError> {
        let mut stream = connect_with_retries(&self.socket_path)?;
        let mut request_bytes = serde_json::to_vec(&RpcRequest::SubscribeTokenRollup {
            query: query.try_into()?,
        })
        .map_err(|_| KvasirClientError::RpcSerialization)?;
        request_bytes.push(b'\n');
        std::io::Write::write_all(&mut stream, &request_bytes)
            .map_err(|_| KvasirClientError::SocketIo)?;
        Ok(KvasirTokenRollupSubscription {
            reader: Mutex::new(BufReader::new(stream)),
        })
    }

    pub fn subscribe_usage_updates(
        &self,
    ) -> Result<KvasirUsageUpdateSubscription, KvasirClientError> {
        let mut stream = connect_with_retries(&self.socket_path)?;
        let mut request_bytes = serde_json::to_vec(&RpcRequest::SubscribeUsageUpdates)
            .map_err(|_| KvasirClientError::RpcSerialization)?;
        request_bytes.push(b'\n');
        std::io::Write::write_all(&mut stream, &request_bytes)
            .map_err(|_| KvasirClientError::SocketIo)?;
        let shutdown_stream = stream
            .try_clone()
            .map_err(|_| KvasirClientError::SocketIo)?;
        Ok(KvasirUsageUpdateSubscription {
            reader: Mutex::new(Some(BufReader::new(stream))),
            shutdown_stream: Mutex::new(Some(shutdown_stream)),
        })
    }
}

#[uniffi::export]
impl KvasirTokenRollupSubscription {
    pub fn next(&self) -> Result<KvasirTokenRollupUpdate, KvasirClientError> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        let event = read_rpc_stream_event(&mut *reader)?;
        match event {
            RpcStreamEvent::TokenRollup { rollups } => Ok(KvasirTokenRollupUpdate {
                rollups: rollups
                    .into_iter()
                    .map(KvasirTokenRollup::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            RpcStreamEvent::UsageUpdate { .. } => Err(KvasirClientError::WrongResponseType),
            RpcStreamEvent::Error { error } => Err(error.into()),
        }
    }
}

#[uniffi::export]
impl KvasirUsageUpdateSubscription {
    pub fn next(&self) -> Result<KvasirUsageUpdateKind, KvasirClientError> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        let reader = reader.as_mut().ok_or(KvasirClientError::SocketIo)?;
        let event = read_rpc_stream_event(reader)?;
        match event {
            RpcStreamEvent::UsageUpdate { kind } => Ok(match kind {
                kvasir_core::rpc::UsageUpdateKind::Initial => KvasirUsageUpdateKind::Initial,
                kvasir_core::rpc::UsageUpdateKind::Changed => KvasirUsageUpdateKind::Changed,
            }),
            RpcStreamEvent::TokenRollup { .. } => Err(KvasirClientError::WrongResponseType),
            RpcStreamEvent::Error { error } => Err(error.into()),
        }
    }

    pub fn close(&self) -> Result<(), KvasirClientError> {
        let mut stream = self
            .shutdown_stream
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        if let Some(stream) = stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        Ok(())
    }
}
