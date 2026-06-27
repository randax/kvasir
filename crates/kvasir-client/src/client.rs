use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

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

#[derive(uniffi::Object)]
pub struct KvasirOverviewRefreshSubscription {
    socket_path: PathBuf,
    usage_subscription: Mutex<Option<Arc<KvasirUsageUpdateSubscription>>>,
    skip_next_initial: Mutex<bool>,
    next_lock: Mutex<()>,
    close_mutex: Mutex<()>,
    close_signal: Condvar,
    closed: AtomicBool,
}

const OVERVIEW_REFRESH_RECONNECT_DELAY: Duration = Duration::from_secs(1);

#[uniffi::export]
impl KvasirClient {
    #[uniffi::constructor]
    pub fn connect(socket_path: KvasirSocketPath) -> Result<Self, KvasirClientError> {
        Ok(Self {
            socket_path: PathBuf::from(socket_path.into_string()),
        })
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
        subscribe_usage_updates_at_path(&self.socket_path)
    }

    pub fn subscribe_overview_refreshes(
        &self,
    ) -> Result<KvasirOverviewRefreshSubscription, KvasirClientError> {
        Ok(KvasirOverviewRefreshSubscription::new(
            self.socket_path.clone(),
        ))
    }
}

impl KvasirClient {
    pub(crate) fn content_replay(
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
}

fn subscribe_usage_updates_at_path(
    socket_path: &std::path::Path,
) -> Result<KvasirUsageUpdateSubscription, KvasirClientError> {
    let stream = connect_with_retries(socket_path)?;
    subscribe_usage_updates_on_stream(stream)
}

fn subscribe_usage_updates_once_at_path(
    socket_path: &std::path::Path,
) -> Result<KvasirUsageUpdateSubscription, KvasirClientError> {
    let stream = UnixStream::connect(socket_path).map_err(|_| KvasirClientError::SocketIo)?;
    subscribe_usage_updates_on_stream(stream)
}

fn subscribe_usage_updates_on_stream(
    mut stream: UnixStream,
) -> Result<KvasirUsageUpdateSubscription, KvasirClientError> {
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

#[uniffi::export]
impl KvasirOverviewRefreshSubscription {
    #[uniffi::constructor]
    pub fn connect(socket_path: KvasirSocketPath) -> Result<Self, KvasirClientError> {
        Ok(Self::new(PathBuf::from(socket_path.into_string())))
    }

    pub fn next(&self) -> Result<(), KvasirClientError> {
        let _next = self
            .next_lock
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(KvasirClientError::SocketIo);
            }

            let subscription = self.current_usage_subscription()?;
            if self.closed.load(Ordering::SeqCst) {
                subscription.close()?;
                self.clear_usage_subscription()?;
                return Err(KvasirClientError::SocketIo);
            }

            match subscription.next() {
                Ok(KvasirUsageUpdateKind::Initial) if self.should_skip_initial()? => {}
                Ok(KvasirUsageUpdateKind::Initial | KvasirUsageUpdateKind::Changed) => {
                    self.mark_bootstrap_seen()?;
                    return Ok(());
                }
                Err(KvasirClientError::SocketIo | KvasirClientError::RpcSerialization) => {
                    self.clear_usage_subscription()?;
                    self.wait_for_reconnect_delay()?;
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub fn close(&self) -> Result<(), KvasirClientError> {
        {
            let _close = self
                .close_mutex
                .lock()
                .map_err(|_| KvasirClientError::SocketIo)?;
            self.closed.store(true, Ordering::SeqCst);
            self.close_signal.notify_all();
        }
        self.clear_usage_subscription()
    }
}

impl KvasirOverviewRefreshSubscription {
    fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            usage_subscription: Mutex::new(None),
            skip_next_initial: Mutex::new(true),
            next_lock: Mutex::new(()),
            close_mutex: Mutex::new(()),
            close_signal: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn current_usage_subscription(
        &self,
    ) -> Result<Arc<KvasirUsageUpdateSubscription>, KvasirClientError> {
        if let Some(subscription) = self
            .usage_subscription
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?
            .as_ref()
            .cloned()
        {
            return Ok(subscription);
        }

        let subscription = Arc::new(self.connect_usage_subscription()?);
        let mut usage_subscription = self
            .usage_subscription
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        if self.closed.load(Ordering::SeqCst) {
            subscription.close()?;
            return Err(KvasirClientError::SocketIo);
        }
        if let Some(existing) = usage_subscription.as_ref().cloned() {
            subscription.close()?;
            return Ok(existing);
        }
        *usage_subscription = Some(Arc::clone(&subscription));
        Ok(subscription)
    }

    fn connect_usage_subscription(
        &self,
    ) -> Result<KvasirUsageUpdateSubscription, KvasirClientError> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(KvasirClientError::SocketIo);
            }
            match subscribe_usage_updates_once_at_path(&self.socket_path) {
                Ok(subscription) => return Ok(subscription),
                Err(KvasirClientError::SocketIo | KvasirClientError::RpcSerialization) => {
                    self.wait_for_reconnect_delay()?;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn wait_for_reconnect_delay(&self) -> Result<(), KvasirClientError> {
        let guard = self
            .close_mutex
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(KvasirClientError::SocketIo);
        }
        let (_guard, _timeout) = self
            .close_signal
            .wait_timeout_while(guard, OVERVIEW_REFRESH_RECONNECT_DELAY, |_| {
                !self.closed.load(Ordering::SeqCst)
            })
            .map_err(|_| KvasirClientError::SocketIo)?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(KvasirClientError::SocketIo);
        }
        Ok(())
    }

    fn clear_usage_subscription(&self) -> Result<(), KvasirClientError> {
        if let Some(subscription) = self
            .usage_subscription
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?
            .take()
        {
            subscription.close()?;
        }
        Ok(())
    }

    fn should_skip_initial(&self) -> Result<bool, KvasirClientError> {
        let mut skip_next_initial = self
            .skip_next_initial
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)?;
        if *skip_next_initial {
            *skip_next_initial = false;
            return Ok(true);
        }
        Ok(false)
    }

    fn mark_bootstrap_seen(&self) -> Result<(), KvasirClientError> {
        *self
            .skip_next_initial
            .lock()
            .map_err(|_| KvasirClientError::SocketIo)? = false;
        Ok(())
    }
}
