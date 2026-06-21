use std::net::SocketAddr;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use kvasir_core::rpc::{BearerToken, RollupQuery, RpcError, RpcRequest, RpcResponse, TokenRollup};
use kvasir_core::{UsageStore, parse_otlp_json_metrics, parse_otlp_protobuf_metrics};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const MAX_RPC_REQUEST_BYTES: usize = 16 * 1024;

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

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::TokenRollup { rollups } => Ok(rollups),
        RpcResponse::Error { error } => anyhow::bail!("rpc failed: {error:?}"),
    }
}

async fn ingest_metrics(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    authorize(&state, &headers)?;

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let records = if content_type.starts_with("application/json") {
        parse_otlp_json_metrics(&body)
    } else if content_type.starts_with("application/x-protobuf") {
        parse_otlp_protobuf_metrics(&body)
    } else {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported metrics content type".to_owned(),
        ));
    }
    .map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "invalid otlp metrics payload".to_owned(),
        )
    })?;

    state
        .store
        .lock()
        .await
        .ingest_token_usage(&records)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "store write failed".to_owned(),
            )
        })?;

    Ok(StatusCode::ACCEPTED)
}

fn authorize(state: &DaemonState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == state.bearer_token.authorization_header())
        .unwrap_or(false);
    if authorized {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "missing bearer token".to_owned()))
    }
}

async fn serve_rpc(listener: UnixListener, state: DaemonState) {
    loop {
        let Ok((stream, _addr)) = listener.accept().await else {
            break;
        };
        let connection_state = state.clone();
        tokio::spawn(async move {
            let _ = handle_rpc_connection(stream, connection_state).await;
        });
    }
}

async fn handle_rpc_connection(stream: UnixStream, state: DaemonState) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let request = read_bounded_line(reader).await?;
    let response = match serde_json::from_str::<RpcRequest>(&request) {
        Ok(RpcRequest::TokenRollup { query }) => {
            match state.store.lock().await.token_rollups(query) {
                Ok(rollups) => RpcResponse::TokenRollup { rollups },
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

async fn read_bounded_line(mut reader: tokio::net::unix::OwnedReadHalf) -> anyhow::Result<String> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let bytes_read = reader.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        for byte in &buffer[..bytes_read] {
            request.push(*byte);
            if request.len() > MAX_RPC_REQUEST_BYTES {
                anyhow::bail!("rpc request exceeds byte limit");
            }
            if *byte == b'\n' {
                return String::from_utf8(request).map_err(Into::into);
            }
        }
    }
    String::from_utf8(request).map_err(Into::into)
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
        anyhow::bail!("refusing to remove non-socket file at {}", path.display())
    }
}
