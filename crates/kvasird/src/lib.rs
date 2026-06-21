use std::fs::File;
use std::net::SocketAddr;
use std::os::raw::c_int;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use kvasir_core::{
    StoreKey, UsageStore, parse_otlp_json_usage_metrics, parse_otlp_protobuf_usage_metrics,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

const MAX_OTLP_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_RPC_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 16 * 1024;
const STORE_STARTUP_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const LOCK_UN: c_int = 8;

unsafe extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub otlp_bind: SocketAddr,
    pub rpc_socket_path: PathBuf,
    pub database_path: PathBuf,
    pub bearer_token: BearerToken,
}

#[derive(Clone)]
pub enum StoreKeySource {
    Keychain(KeychainStoreKeySource),
    Static(StoreKey),
}

impl std::fmt::Debug for StoreKeySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keychain(source) => formatter.debug_tuple("Keychain").field(source).finish(),
            Self::Static(_key) => formatter.write_str("Static(StoreKey(<redacted>))"),
        }
    }
}

impl StoreKeySource {
    pub fn keychain_for_database(database_path: &Path) -> Self {
        Self::Keychain(KeychainStoreKeySource::for_database(database_path))
    }

    pub fn static_key_for_test(bytes: [u8; 32]) -> Self {
        Self::Static(StoreKey::from_bytes(bytes))
    }

    fn resolve(&self) -> anyhow::Result<StoreKey> {
        match self {
            Self::Keychain(source) => source.resolve(),
            Self::Static(key) => Ok(key.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeychainStoreKeySource {
    service: &'static str,
    user: String,
}

impl KeychainStoreKeySource {
    fn for_database(database_path: &Path) -> Self {
        Self {
            service: "dev.kvasir.store",
            user: keychain_user_for_database(database_path),
        }
    }

    fn resolve(&self) -> anyhow::Result<StoreKey> {
        let entry = keyring::Entry::new(self.service, &self.user)?;
        resolve_store_key(&KeyringStoreKeyCredential { entry })
    }
}

trait StoreKeyCredential {
    fn get_password(&self) -> Result<String, StoreKeyCredentialReadError>;
    fn set_password(&self, password: &str) -> anyhow::Result<()>;
}

enum StoreKeyCredentialReadError {
    NoEntry,
    Read(anyhow::Error),
}

struct KeyringStoreKeyCredential {
    entry: keyring::Entry,
}

impl StoreKeyCredential for KeyringStoreKeyCredential {
    fn get_password(&self) -> Result<String, StoreKeyCredentialReadError> {
        self.entry.get_password().map_err(|err| match err {
            keyring::Error::NoEntry => StoreKeyCredentialReadError::NoEntry,
            other => StoreKeyCredentialReadError::Read(other.into()),
        })
    }

    fn set_password(&self, password: &str) -> anyhow::Result<()> {
        self.entry.set_password(password)?;
        Ok(())
    }
}

fn resolve_store_key(credential: &impl StoreKeyCredential) -> anyhow::Result<StoreKey> {
    match credential.get_password() {
        Ok(encoded_key) => {
            let encoded_key = Zeroizing::new(encoded_key);
            Ok(StoreKey::from_hex(&encoded_key)?)
        }
        Err(StoreKeyCredentialReadError::NoEntry) => {
            let key = StoreKey::generate()?;
            let encoded_key = key.to_hex_secret();
            credential.set_password(&encoded_key)?;
            Ok(key)
        }
        Err(StoreKeyCredentialReadError::Read(err)) => Err(err),
    }
}

fn keychain_user_for_database(database_path: &Path) -> String {
    let stable_path = canonical_database_path(database_path);
    format!("usage.sqlite3:{}", stable_path.to_string_lossy())
}

fn canonical_database_path(database_path: &Path) -> PathBuf {
    if let Ok(path) = database_path.canonicalize() {
        return path;
    }

    let absolute_path = absolute_lexical_path(database_path);
    let Some(parent) = absolute_path.parent() else {
        return absolute_path;
    };
    let Some(file_name) = absolute_path.file_name() else {
        return absolute_path;
    };
    parent
        .canonicalize()
        .map(|parent| parent.join(file_name))
        .unwrap_or(absolute_path)
}

fn absolute_lexical_path(path: &Path) -> PathBuf {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    lexical_normalize_path(&absolute_path)
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.as_os_str() != "/" && !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
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
    let store_key_source = StoreKeySource::keychain_for_database(&config.database_path);
    start_with_store_key_source(config, store_key_source).await
}

pub async fn start_with_store_key_source(
    config: DaemonConfig,
    store_key_source: StoreKeySource,
) -> anyhow::Result<RunningDaemon> {
    let _startup_lock = StoreStartupLock::acquire(&config.database_path).await?;
    let store_key = store_key_source.resolve()?;
    let store = UsageStore::open(&config.database_path, &store_key)?;
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

struct StoreStartupLock {
    file: File,
}

impl StoreStartupLock {
    async fn acquire(database_path: &Path) -> anyhow::Result<Self> {
        let lock_path = store_startup_lock_path(database_path);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let started_at = Instant::now();
        loop {
            match try_lock_file(&file) {
                Ok(true) => {
                    return Ok(Self { file });
                }
                Ok(false) => {}
                Err(err) => return Err(err.into()),
            }
            if started_at.elapsed() >= STORE_STARTUP_LOCK_TIMEOUT {
                return Err(DaemonError::StoreStartupLockTimedOut {
                    path: lock_path.clone(),
                }
                .into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for StoreStartupLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

fn store_startup_lock_path(database_path: &Path) -> PathBuf {
    let stable_path = canonical_database_path(database_path);
    let mut lock_path = stable_path.as_os_str().to_os_string();
    lock_path.push(".startup-lock");
    PathBuf::from(lock_path)
}

fn try_lock_file(file: &File) -> std::io::Result<bool> {
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err)
    }
}

fn unlock_file(file: &File) -> std::io::Result<()> {
    let result = unsafe { flock(file.as_raw_fd(), LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
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
    #[error("timed out waiting for store startup lock {path}")]
    StoreStartupLockTimedOut { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn store_key_resolver_generates_once_and_reuses_stored_key() -> anyhow::Result<()> {
        let credential = MemoryStoreKeyCredential::default();

        let first_key = resolve_store_key(&credential)?;
        let second_key = resolve_store_key(&credential)?;

        assert_eq!(first_key, second_key);
        assert_eq!(credential.set_count.get(), 1);
        assert_eq!(credential.password.borrow().as_ref().unwrap().len(), 64);

        Ok(())
    }

    #[test]
    fn keychain_users_are_scoped_to_database_paths() -> anyhow::Result<()> {
        let temp = tempdir()?;

        assert_ne!(
            keychain_user_for_database(&temp.path().join("first.sqlite3")),
            keychain_user_for_database(&temp.path().join("second.sqlite3"))
        );

        Ok(())
    }

    #[test]
    fn startup_lock_paths_use_the_same_canonical_database_identity() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let alias_path = temp.path().join(".").join("usage.sqlite3");

        assert_eq!(
            store_startup_lock_path(&database_path),
            store_startup_lock_path(&alias_path)
        );

        Ok(())
    }

    #[test]
    fn relative_database_path_spellings_share_keychain_and_lock_identity() {
        let bare_path = Path::new("usage.sqlite3");
        let dot_path = Path::new("./usage.sqlite3");

        assert_eq!(
            keychain_user_for_database(bare_path),
            keychain_user_for_database(dot_path)
        );
        assert_eq!(
            store_startup_lock_path(bare_path),
            store_startup_lock_path(dot_path)
        );
    }

    #[test]
    #[cfg(unix)]
    fn nonexistent_database_under_symlinked_parent_uses_real_parent_identity() -> anyhow::Result<()>
    {
        let temp = tempdir()?;
        let real_parent = temp.path().join("real");
        let symlink_parent = temp.path().join("link");
        std::fs::create_dir(&real_parent)?;
        std::os::unix::fs::symlink(&real_parent, &symlink_parent)?;
        let real_path = real_parent.join("usage.sqlite3");
        let symlink_path = symlink_parent.join("usage.sqlite3");

        assert_eq!(
            keychain_user_for_database(&real_path),
            keychain_user_for_database(&symlink_path)
        );
        assert_eq!(
            store_startup_lock_path(&real_path),
            store_startup_lock_path(&symlink_path)
        );

        Ok(())
    }

    #[tokio::test]
    async fn startup_lock_can_acquire_existing_unlocked_file() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        std::fs::write(store_startup_lock_path(&database_path), "stale")?;

        let _lock = StoreStartupLock::acquire(&database_path).await?;

        Ok(())
    }

    #[test]
    fn static_store_key_source_debug_output_is_redacted() {
        let source = StoreKeySource::static_key_for_test([11; 32]);

        assert_eq!(format!("{source:?}"), "Static(StoreKey(<redacted>))");
    }

    #[derive(Default)]
    struct MemoryStoreKeyCredential {
        password: RefCell<Option<String>>,
        set_count: Cell<usize>,
    }

    impl StoreKeyCredential for MemoryStoreKeyCredential {
        fn get_password(&self) -> Result<String, StoreKeyCredentialReadError> {
            self.password
                .borrow()
                .clone()
                .ok_or(StoreKeyCredentialReadError::NoEntry)
        }

        fn set_password(&self, password: &str) -> anyhow::Result<()> {
            self.set_count.set(self.set_count.get() + 1);
            self.password.replace(Some(password.to_owned()));
            Ok(())
        }
    }
}
