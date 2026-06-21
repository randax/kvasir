use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use chrono::Datelike;
use kvasir_core::rpc::{
    CostRollup as CoreCostRollup, CostRollupQuery, RollupQuery, RpcError, RpcRequest, RpcResponse,
    RpcStreamEvent, TimestampMillis, TokenRollup as CoreTokenRollup,
};
use kvasir_core::{RepoBucket, RepoIdentity, RepoName, RepoPath};

uniffi::setup_scaffolding!();

const MAX_RPC_RESPONSE_BYTES: u64 = 16 * 1024;
const CONNECT_RETRY_ATTEMPTS: usize = 80;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KvasirClientError {
    #[error("invalid query")]
    InvalidQuery,
    #[error("socket io failed")]
    SocketIo,
    #[error("rpc serialization failed")]
    RpcSerialization,
    #[error("rpc response is too large")]
    RpcResponseTooLarge,
    #[error("rpc returned a daemon error")]
    DaemonError,
    #[error("rpc returned the wrong response type")]
    WrongResponseType,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KvasirRepoBucketKind {
    NoRepo,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirSocketPath {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTimestampMillis {
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirModelName {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRepoName {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRepoPath {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRepoBucket {
    pub kind: KvasirRepoBucketKind,
    pub name: Option<KvasirRepoName>,
    pub path: Option<KvasirRepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRollupDay {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirRollupQuery {
    pub start: KvasirTimestampMillis,
    pub end: KvasirTimestampMillis,
    pub repo: Option<KvasirRepoBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTokenRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub model: KvasirModelName,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirTokenRollupUpdate {
    pub rollups: Vec<KvasirTokenRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirCostUsd {
    pub nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirCostRollup {
    pub day: KvasirRollupDay,
    pub repo: KvasirRepoBucket,
    pub model: KvasirModelName,
    pub cost_usd: KvasirCostUsd,
}

#[derive(Debug, uniffi::Object)]
pub struct KvasirClient {
    socket_path: PathBuf,
}

#[derive(uniffi::Object)]
pub struct KvasirTokenRollupSubscription {
    reader: Mutex<BufReader<UnixStream>>,
}

#[uniffi::export]
impl KvasirClient {
    #[uniffi::constructor]
    pub fn connect(socket_path: KvasirSocketPath) -> Result<Self, KvasirClientError> {
        let socket_path = PathBuf::from(socket_path.value);
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
        stream
            .write_all(&request_bytes)
            .map_err(|_| KvasirClientError::SocketIo)?;
        Ok(KvasirTokenRollupSubscription {
            reader: Mutex::new(BufReader::new(stream)),
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
            RpcStreamEvent::Error { error } => Err(error.into()),
        }
    }
}

fn send_rpc_request(
    socket_path: &PathBuf,
    request: RpcRequest,
) -> Result<RpcResponse, KvasirClientError> {
    let mut stream = connect_with_retries(socket_path)?;
    let mut request_bytes =
        serde_json::to_vec(&request).map_err(|_| KvasirClientError::RpcSerialization)?;
    request_bytes.push(b'\n');
    stream
        .write_all(&request_bytes)
        .map_err(|_| KvasirClientError::SocketIo)?;

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(&mut reader)?;
    serde_json::from_str(&response).map_err(|_| KvasirClientError::RpcSerialization)
}

fn connect_with_retries(socket_path: &PathBuf) -> Result<UnixStream, KvasirClientError> {
    let mut last_error = None;
    for attempt in 0..CONNECT_RETRY_ATTEMPTS {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                last_error = Some(err);
                if attempt + 1 < CONNECT_RETRY_ATTEMPTS {
                    thread::sleep(CONNECT_RETRY_DELAY);
                }
            }
        }
    }
    let _ = last_error;
    Err(KvasirClientError::SocketIo)
}

fn read_rpc_stream_event<R>(reader: &mut R) -> Result<RpcStreamEvent, KvasirClientError>
where
    R: BufRead + Read,
{
    let response = read_bounded_line(reader)?;
    serde_json::from_str(&response).map_err(|_| KvasirClientError::RpcSerialization)
}

fn read_bounded_line<R>(reader: &mut R) -> Result<String, KvasirClientError>
where
    R: BufRead + Read,
{
    let mut response = String::new();
    let bytes_read = reader
        .take(MAX_RPC_RESPONSE_BYTES + 1)
        .read_line(&mut response)
        .map_err(|_| KvasirClientError::SocketIo)?;
    if bytes_read == 0 && response.is_empty() {
        return Err(KvasirClientError::SocketIo);
    }
    if bytes_read as u64 > MAX_RPC_RESPONSE_BYTES {
        return Err(KvasirClientError::RpcResponseTooLarge);
    }
    Ok(response)
}

impl TryFrom<KvasirRollupQuery> for RollupQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirRollupQuery) -> Result<Self, Self::Error> {
        let mut core_query = Self::new(
            TimestampMillis::from_millis(query.start.value),
            TimestampMillis::from_millis(query.end.value),
        );
        if let Some(repo) = query.repo {
            core_query = core_query.with_repo(repo.try_into()?);
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirRollupQuery> for CostRollupQuery {
    type Error = KvasirClientError;

    fn try_from(query: KvasirRollupQuery) -> Result<Self, Self::Error> {
        let mut core_query = Self::new(
            TimestampMillis::from_millis(query.start.value),
            TimestampMillis::from_millis(query.end.value),
        );
        if let Some(repo) = query.repo {
            core_query = core_query.with_repo(repo.try_into()?);
        }
        Ok(core_query)
    }
}

impl TryFrom<KvasirRepoBucket> for RepoBucket {
    type Error = KvasirClientError;

    fn try_from(repo: KvasirRepoBucket) -> Result<Self, Self::Error> {
        match repo.kind {
            KvasirRepoBucketKind::NoRepo => Ok(Self::no_repo()),
            KvasirRepoBucketKind::Repo => {
                let name = repo.name.map(|name| RepoName::new(name.value));
                let path = repo.path.map(|path| RepoPath::new(path.value));
                RepoIdentity::from_parts(name, path)
                    .map(Self::repo)
                    .ok_or(KvasirClientError::InvalidQuery)
            }
        }
    }
}

fn rollup_day_from_core(
    day: kvasir_core::rpc::RollupDay,
) -> Result<KvasirRollupDay, KvasirClientError> {
    let date = day.as_date();
    Ok(KvasirRollupDay {
        year: date.year(),
        month: u8::try_from(date.month()).map_err(|_| KvasirClientError::InvalidQuery)?,
        day: u8::try_from(date.day()).map_err(|_| KvasirClientError::InvalidQuery)?,
    })
}

impl TryFrom<CoreTokenRollup> for KvasirTokenRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreTokenRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            day: rollup_day_from_core(rollup.day)?,
            repo: rollup.repo.into(),
            model: rollup.model.into(),
            input_tokens: rollup.input_tokens,
            output_tokens: rollup.output_tokens,
            cache_tokens: rollup.cache_tokens,
        })
    }
}

impl TryFrom<CoreCostRollup> for KvasirCostRollup {
    type Error = KvasirClientError;

    fn try_from(rollup: CoreCostRollup) -> Result<Self, Self::Error> {
        Ok(Self {
            day: rollup_day_from_core(rollup.day)?,
            repo: rollup.repo.into(),
            model: rollup.model.into(),
            cost_usd: KvasirCostUsd {
                nanos: rollup.cost_usd.as_nanos(),
            },
        })
    }
}

impl From<RepoBucket> for KvasirRepoBucket {
    fn from(repo: RepoBucket) -> Self {
        match repo {
            RepoBucket::NoRepo => Self {
                kind: KvasirRepoBucketKind::NoRepo,
                name: None,
                path: None,
            },
            RepoBucket::Repo(identity) => Self {
                kind: KvasirRepoBucketKind::Repo,
                name: identity.name.map(KvasirRepoName::from),
                path: identity.path.map(KvasirRepoPath::from),
            },
        }
    }
}

impl From<kvasir_core::rpc::ModelName> for KvasirModelName {
    fn from(model: kvasir_core::rpc::ModelName) -> Self {
        Self {
            value: model.as_str().to_owned(),
        }
    }
}

impl From<RepoName> for KvasirRepoName {
    fn from(name: RepoName) -> Self {
        Self {
            value: name.as_str().to_owned(),
        }
    }
}

impl From<RepoPath> for KvasirRepoPath {
    fn from(path: RepoPath) -> Self {
        Self {
            value: path.as_str().to_owned(),
        }
    }
}

impl From<RpcError> for KvasirClientError {
    fn from(_error: RpcError) -> Self {
        Self::DaemonError
    }
}
