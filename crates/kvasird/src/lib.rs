use std::net::SocketAddr;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use kvasir_core::rpc::{
    BearerToken, CostRollup, CostRollupQuery, RollupQuery, RpcError, RpcRequest, RpcResponse,
    TokenRollup,
};
use kvasir_core::{UsageStore, parse_otlp_json_usage_metrics, parse_otlp_protobuf_usage_metrics};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const MAX_OTLP_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_RPC_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub otlp_bind: SocketAddr,
    pub rpc_socket_path: PathBuf,
    pub database_path: PathBuf,
    pub bearer_token: BearerToken,
}

pub struct RunningDaemon {
    otlp_addr: SocketAddr,
    rpc_socket_path: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

impl RunningDaemon {
    pub fn otlp_addr(&self) -> SocketAddr {
        self.otlp_addr
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = remove_stale_socket(&self.rpc_socket_path);
    }
}

#[derive(Clone)]
struct DaemonState {
    store: Arc<Mutex<UsageStore>>,
    bearer_token: BearerToken,
}

pub async fn start(config: DaemonConfig) -> anyhow::Result<RunningDaemon> {
    let store = UsageStore::open(&config.database_path)?;
    let state = DaemonState {
        store: Arc::new(Mutex::new(store)),
        bearer_token: config.bearer_token,
    };

    let tcp_listener = TcpListener::bind(config.otlp_bind).await?;
    let otlp_addr = tcp_listener.local_addr()?;
    let app = Router::new()
        .route("/v1/metrics", post(ingest_metrics))
        .layer(DefaultBodyLimit::max(MAX_OTLP_REQUEST_BYTES))
        .with_state(state.clone());
    let http_task = tokio::spawn(async move {
        let _ = axum::serve(tcp_listener, app).await;
    });

    remove_stale_socket(&config.rpc_socket_path)?;
    let unix_listener = UnixListener::bind(&config.rpc_socket_path)?;
    std::fs::set_permissions(
        &config.rpc_socket_path,
        std::fs::Permissions::from_mode(0o600),
    )?;
    let rpc_state = state.clone();
    let rpc_task = tokio::spawn(async move {
        serve_rpc(unix_listener, rpc_state).await;
    });

    Ok(RunningDaemon {
        otlp_addr,
        rpc_socket_path: config.rpc_socket_path,
        tasks: vec![http_task, rpc_task],
    })
}

pub async fn query_token_rollup(
    socket_path: impl Into<PathBuf>,
    query: RollupQuery,
) -> anyhow::Result<Vec<TokenRollup>> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::TokenRollup { query };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    stream.write_all(&request_bytes).await?;

    let response = read_bounded_line(
        stream,
        MAX_RPC_RESPONSE_BYTES,
        DaemonError::RpcResponseTooLarge,
    )
    .await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::TokenRollup { rollups } => Ok(rollups),
        RpcResponse::Error { error } => Err(DaemonError::RpcReturnedError(error).into()),
        _ => Err(DaemonError::RpcReturnedWrongResponse.into()),
    }
}

pub async fn query_cost_rollup(
    socket_path: impl Into<PathBuf>,
    query: CostRollupQuery,
) -> anyhow::Result<Vec<CostRollup>> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::CostRollup { query };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    stream.write_all(&request_bytes).await?;

    let response = read_bounded_line(
        stream,
        MAX_RPC_RESPONSE_BYTES,
        DaemonError::RpcResponseTooLarge,
    )
    .await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::CostRollup { rollups } => Ok(rollups),
        RpcResponse::Error { error } => Err(DaemonError::RpcReturnedError(error).into()),
        _ => Err(DaemonError::RpcReturnedWrongResponse.into()),
    }
}

async fn ingest_metrics(
    State(state): State<DaemonState>,
    request: Request,
) -> Result<StatusCode, IngestError> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    authorize(&state, &headers)?;
    reject_oversized_content_length(&headers)?;

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let body = to_bytes(body, MAX_OTLP_REQUEST_BYTES)
        .await
        .map_err(|_| IngestError::PayloadTooLarge)?;
    let records = if content_type.starts_with("application/json") {
        parse_otlp_json_usage_metrics(&body)
    } else if content_type.starts_with("application/x-protobuf") {
        parse_otlp_protobuf_usage_metrics(&body)
    } else {
        return Err(IngestError::UnsupportedContentType);
    }
    .map_err(|_| IngestError::InvalidPayload)?;

    state
        .store
        .lock()
        .await
        .ingest_usage(&records)
        .map_err(|_| IngestError::StoreWriteFailed)?;

    Ok(StatusCode::ACCEPTED)
}

fn authorize(state: &DaemonState, headers: &HeaderMap) -> Result<(), IngestError> {
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == state.bearer_token.authorization_header())
        .unwrap_or(false);
    if authorized {
        Ok(())
    } else {
        Err(IngestError::Unauthorized)
    }
}

fn reject_oversized_content_length(headers: &HeaderMap) -> Result<(), IngestError> {
    let Some(content_length) = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return Ok(());
    };
    if content_length > MAX_OTLP_REQUEST_BYTES {
        Err(IngestError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

async fn serve_rpc(listener: UnixListener, state: DaemonState) {
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(connection) => connection,
            Err(err) => {
                eprintln!("kvasird rpc accept failed: {err}");
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                continue;
            }
        };
        let connection_state = state.clone();
        tokio::spawn(async move {
            let _ = handle_rpc_connection(stream, connection_state).await;
        });
    }
}

async fn handle_rpc_connection(stream: UnixStream, state: DaemonState) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let request = read_bounded_line(
        reader,
        MAX_RPC_REQUEST_BYTES,
        DaemonError::RpcRequestTooLarge,
    )
    .await?;
    let response = match serde_json::from_str::<RpcRequest>(&request) {
        Ok(RpcRequest::TokenRollup { query }) => {
            match state.store.lock().await.token_rollups(query) {
                Ok(rollups) => RpcResponse::TokenRollup { rollups },
                Err(_err) => RpcResponse::Error {
                    error: RpcError::Internal,
                },
            }
        }
        Ok(RpcRequest::CostRollup { query }) => {
            match state.store.lock().await.cost_rollups(query) {
                Ok(rollups) => RpcResponse::CostRollup { rollups },
                Err(_err) => RpcResponse::Error {
                    error: RpcError::Internal,
                },
            }
        }
        Err(_err) => RpcResponse::Error {
            error: RpcError::InvalidRequest,
        },
    };
    let mut response_bytes = serde_json::to_vec(&response)?;
    response_bytes.push(b'\n');
    writer.write_all(&response_bytes).await?;
    Ok(())
}

async fn read_bounded_line<R>(
    mut reader: R,
    byte_limit: usize,
    limit_error: DaemonError,
) -> Result<String, DaemonError>
where
    R: AsyncRead + Unpin,
{
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let bytes_read = reader.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        for byte in &buffer[..bytes_read] {
            request.push(*byte);
            if request.len() > byte_limit {
                return Err(limit_error);
            }
            if *byte == b'\n' {
                return String::from_utf8(request).map_err(DaemonError::InvalidRpcUtf8);
            }
        }
    }
    String::from_utf8(request).map_err(DaemonError::InvalidRpcUtf8)
}

fn remove_stale_socket(path: &PathBuf) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if metadata.file_type().is_socket() {
        std::fs::remove_file(path)?;
        Ok(())
    } else {
        Err(DaemonError::RpcSocketPathIsNotSocket { path: path.clone() }.into())
    }
}

#[derive(Debug, thiserror::Error)]
enum IngestError {
    #[error("unauthorized metrics ingest")]
    Unauthorized,
    #[error("unsupported metrics content type")]
    UnsupportedContentType,
    #[error("invalid metrics payload")]
    InvalidPayload,
    #[error("metrics payload is too large")]
    PayloadTooLarge,
    #[error("metrics store write failed")]
    StoreWriteFailed,
}

impl IntoResponse for IngestError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::InvalidPayload => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::StoreWriteFailed => StatusCode::INTERNAL_SERVER_ERROR,
        }
        .into_response()
    }
}

#[derive(Debug, thiserror::Error)]
enum DaemonError {
    #[error("rpc request exceeds byte limit")]
    RpcRequestTooLarge,
    #[error("rpc response exceeds byte limit")]
    RpcResponseTooLarge,
    #[error("rpc returned typed error {0:?}")]
    RpcReturnedError(RpcError),
    #[error("rpc returned the wrong response type")]
    RpcReturnedWrongResponse,
    #[error("rpc payload is not valid utf-8")]
    InvalidRpcUtf8(#[from] std::string::FromUtf8Error),
    #[error("rpc io failed")]
    RpcIo(#[from] std::io::Error),
    #[error("refusing to remove non-socket rpc path {path}")]
    RpcSocketPathIsNotSocket { path: PathBuf },
}
