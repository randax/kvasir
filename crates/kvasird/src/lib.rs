use std::collections::HashSet;
use std::fs::File;
use std::net::SocketAddr;
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
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
    BearerToken, ContentQuery, ContentReplay, CostRollup, CostRollupQuery, OverviewRollup,
    RollupQuery, RpcError, RpcRequest, RpcResponse, RpcStreamEvent, TokenRollup, ToolCallRollup,
    ToolCallRollupQuery, Trace, TraceQuery,
};
use kvasir_core::{
    PriceTable, RawBodyImportFailure, RawBodyImportFailureKind, RawBodyImportPreparation,
    StoreError, StoreKey, UsageStore, cleanup_prepared_raw_body_imports, parse_otlp_json_traces,
    parse_otlp_json_usage_logs, parse_otlp_json_usage_metrics, parse_otlp_protobuf_traces,
    parse_otlp_protobuf_usage_logs, parse_otlp_protobuf_usage_metrics,
    prepare_raw_body_import_candidate,
};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

mod setup;

const MAX_OTLP_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_RPC_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 1024 * 1024;
const RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const STORE_STARTUP_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const RAW_BODY_IMPORT_SCAN_INTERVAL: Duration = Duration::from_millis(250);
const RAW_BODY_IMPORT_BATCH_SIZE: usize = 64;
const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const LOCK_UN: c_int = 8;

pub use setup::{KeychainSetupSecretSource, SetupSecretSource};

unsafe extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub otlp_bind: SocketAddr,
    pub rpc_socket_path: PathBuf,
    pub database_path: PathBuf,
    pub bearer_token: BearerToken,
    pub price_table: PriceTable,
}

impl DaemonConfig {
    pub fn new(
        otlp_bind: SocketAddr,
        rpc_socket_path: PathBuf,
        database_path: PathBuf,
        bearer_token: BearerToken,
    ) -> Self {
        Self {
            otlp_bind,
            rpc_socket_path,
            database_path,
            bearer_token,
            price_table: PriceTable::bundled_defaults(),
        }
    }
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
    format!("usage.sqlite3:{}", keychain_path_component(&stable_path))
}

fn keychain_path_component(stable_path: &Path) -> String {
    if let Some(path) = stable_path.to_str() {
        return format!("utf8:{path}");
    }

    format!("hex:{}", hex_encode(stable_path.as_os_str().as_bytes()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(hex_nibble(byte >> 4));
        encoded.push(hex_nibble(byte & 0x0f));
    }
    encoded
}

fn hex_nibble(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
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
    shutdown: broadcast::Sender<()>,
    tasks: Vec<JoinHandle<()>>,
    state: DaemonState,
}

impl RunningDaemon {
    pub fn otlp_addr(&self) -> SocketAddr {
        self.otlp_addr
    }

    #[doc(hidden)]
    pub async fn import_available_raw_bodies_once(&self) -> anyhow::Result<bool> {
        import_available_raw_bodies(&self.state)
            .await
            .map_err(anyhow::Error::from)
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
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
    raw_body_directory: PathBuf,
    usage_updates: broadcast::Sender<()>,
    shutdown: broadcast::Sender<()>,
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
    let store = UsageStore::open_with_price_table(
        &config.database_path,
        &store_key,
        config.price_table.clone(),
    )?;
    let raw_body_directory = prepare_raw_body_directory(&config.database_path)?;
    let (usage_updates, _usage_update_receiver) = broadcast::channel(32);
    let (shutdown, _shutdown_receiver) = broadcast::channel(1);
    let state = DaemonState {
        store: Arc::new(Mutex::new(store)),
        bearer_token: config.bearer_token,
        raw_body_directory,
        usage_updates,
        shutdown: shutdown.clone(),
    };

    remove_stale_socket(&config.rpc_socket_path)?;
    let unix_listener = bind_private_unix_listener(&config.rpc_socket_path)?;
    std::fs::set_permissions(
        &config.rpc_socket_path,
        std::fs::Permissions::from_mode(0o600),
    )?;

    let tcp_listener = TcpListener::bind(config.otlp_bind).await?;
    let otlp_addr = tcp_listener.local_addr()?;
    let app = Router::new()
        .route("/v1/metrics", post(ingest_metrics))
        .route("/v1/logs", post(ingest_logs))
        .route("/v1/traces", post(ingest_traces))
        .layer(DefaultBodyLimit::max(MAX_OTLP_REQUEST_BYTES))
        .with_state(state.clone());
    let http_task = tokio::spawn(async move {
        let _ = axum::serve(tcp_listener, app).await;
    });
    let rpc_state = state.clone();
    let rpc_shutdown = shutdown.clone();
    let rpc_task = tokio::spawn(async move {
        serve_rpc(unix_listener, rpc_state, rpc_shutdown).await;
    });
    let raw_body_state = state.clone();
    let raw_body_shutdown = shutdown.clone();
    let raw_body_import_task = tokio::spawn(async move {
        run_raw_body_import_scanner(raw_body_state, raw_body_shutdown).await;
    });

    Ok(RunningDaemon {
        otlp_addr,
        rpc_socket_path: config.rpc_socket_path,
        shutdown,
        tasks: vec![http_task, rpc_task, raw_body_import_task],
        state,
    })
}

async fn run_raw_body_import_scanner(state: DaemonState, shutdown: broadcast::Sender<()>) {
    let mut shutdown = shutdown.subscribe();
    let mut interval = tokio::time::interval(RAW_BODY_IMPORT_SCAN_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = interval.tick() => {
                if let Err(error) = import_available_raw_bodies(&state).await {
                    eprintln!("raw body background import failed: {error:?}");
                }
            }
        }
    }
}

fn bind_private_unix_listener(path: &Path) -> std::io::Result<UnixListener> {
    require_private_socket_parent(path)?;
    UnixListener::bind(path)
}

fn raw_body_directory_for_database(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("raw-bodies"))
        .unwrap_or_else(|| PathBuf::from("raw-bodies"))
}

fn prepare_raw_body_directory(database_path: &Path) -> std::io::Result<PathBuf> {
    let raw_body_directory = raw_body_directory_for_database(database_path);
    std::fs::create_dir_all(&raw_body_directory)?;
    let metadata = std::fs::symlink_metadata(&raw_body_directory)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "raw body directory must be a real directory",
        ));
    }
    set_private_directory_permissions(&raw_body_directory)?;
    raw_body_directory.canonicalize()
}

fn set_private_directory_permissions(path: &Path) -> std::io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn require_private_socket_parent(path: &Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let metadata = std::fs::metadata(parent)?;
    let mode = metadata.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(());
    }
    if mode & 0o1000 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("rpc socket parent {} is not private", parent.display()),
        ));
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
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

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(
        &mut reader,
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

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(
        &mut reader,
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

pub async fn query_tool_call_rollup(
    socket_path: impl Into<PathBuf>,
    query: ToolCallRollupQuery,
) -> anyhow::Result<Vec<ToolCallRollup>> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::ToolCallRollup { query };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    stream.write_all(&request_bytes).await?;

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(
        &mut reader,
        MAX_RPC_RESPONSE_BYTES,
        DaemonError::RpcResponseTooLarge,
    )
    .await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::ToolCallRollup { rollups } => Ok(rollups),
        RpcResponse::Error { error } => Err(DaemonError::RpcReturnedError(error).into()),
        _ => Err(DaemonError::RpcReturnedWrongResponse.into()),
    }
}

pub async fn query_trace(
    socket_path: impl Into<PathBuf>,
    query: TraceQuery,
) -> anyhow::Result<Vec<Trace>> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::Trace { query };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    stream.write_all(&request_bytes).await?;

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(
        &mut reader,
        MAX_RPC_RESPONSE_BYTES,
        DaemonError::RpcResponseTooLarge,
    )
    .await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::Trace { traces } => Ok(traces),
        RpcResponse::Error { error } => Err(DaemonError::RpcReturnedError(error).into()),
        _ => Err(DaemonError::RpcReturnedWrongResponse.into()),
    }
}

pub async fn query_content(
    socket_path: impl Into<PathBuf>,
    query: ContentQuery,
    bearer_token: BearerToken,
) -> anyhow::Result<ContentReplay> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::Content {
        query,
        bearer_token,
    };
    let mut request_bytes = serde_json::to_vec(&request)?;
    request_bytes.push(b'\n');
    stream.write_all(&request_bytes).await?;

    let mut reader = BufReader::new(stream);
    let response = read_bounded_line(
        &mut reader,
        MAX_RPC_RESPONSE_BYTES,
        DaemonError::RpcResponseTooLarge,
    )
    .await?;
    match serde_json::from_str::<RpcResponse>(&response)? {
        RpcResponse::Content { replay } => Ok(replay),
        RpcResponse::Error { error } => Err(DaemonError::RpcReturnedError(error).into()),
        _ => Err(DaemonError::RpcReturnedWrongResponse.into()),
    }
}

async fn ingest_metrics(
    State(state): State<DaemonState>,
    request: Request,
) -> Result<StatusCode, IngestError> {
    ingest_otlp(
        request,
        state,
        parse_otlp_json_usage_metrics,
        parse_otlp_protobuf_usage_metrics,
    )
    .await
}

async fn ingest_logs(
    State(state): State<DaemonState>,
    request: Request,
) -> Result<StatusCode, IngestError> {
    ingest_otlp(
        request,
        state,
        parse_otlp_json_usage_logs,
        parse_otlp_protobuf_usage_logs,
    )
    .await
}

async fn ingest_traces(
    State(state): State<DaemonState>,
    request: Request,
) -> Result<StatusCode, IngestError> {
    ingest_otlp(
        request,
        state,
        parse_otlp_json_traces,
        parse_otlp_protobuf_traces,
    )
    .await
}

async fn ingest_otlp(
    request: Request,
    state: DaemonState,
    parse_json: fn(&[u8]) -> Result<kvasir_core::usage::UsageRecords, kvasir_core::otlp::OtlpError>,
    parse_protobuf: fn(
        &[u8],
    )
        -> Result<kvasir_core::usage::UsageRecords, kvasir_core::otlp::OtlpError>,
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
        parse_json(&body)
    } else if content_type.starts_with("application/x-protobuf") {
        parse_protobuf(&body)
    } else {
        return Err(IngestError::UnsupportedContentType);
    }
    .map_err(|_| IngestError::InvalidPayload)?;

    {
        let mut store = state.store.lock().await;
        store
            .ingest_usage(&records)
            .map_err(|_| IngestError::StoreWriteFailed)?;
        store
            .record_raw_body_references(&records.raw_body_references)
            .map_err(|_| IngestError::StoreWriteFailed)?;
    }
    import_available_raw_bodies(&state).await?;
    let _ = state.usage_updates.send(());

    Ok(StatusCode::ACCEPTED)
}

async fn import_available_raw_bodies(state: &DaemonState) -> Result<bool, IngestError> {
    let candidates = {
        let store = state.store.lock().await;
        store
            .raw_body_import_candidates(RAW_BODY_IMPORT_BATCH_SIZE)
            .map_err(|_| IngestError::StoreWriteFailed)?
    };
    if candidates.is_empty() {
        return Ok(false);
    }

    let mut prepared_imports = Vec::new();
    let mut completed_event_keys = Vec::new();
    let mut import_failures = Vec::new();
    for candidate in candidates {
        let event_key = candidate.event_key().to_owned();
        if candidate.is_blocked_by_unsupported_compression() {
            import_failures.push(RawBodyImportFailure {
                event_key,
                failure_kind: RawBodyImportFailureKind::UnsupportedStoredCompression,
            });
            continue;
        }

        match prepare_raw_body_import_candidate(&state.raw_body_directory, candidate) {
            Ok(RawBodyImportPreparation::Prepared(prepared)) => {
                prepared_imports.push(prepared);
            }
            Ok(RawBodyImportPreparation::Missing(candidate)) => {
                import_failures.push(RawBodyImportFailure {
                    event_key: candidate.event_key().to_owned(),
                    failure_kind: RawBodyImportFailureKind::Missing,
                });
            }
            Ok(RawBodyImportPreparation::AlreadyCleaned(candidate)) => {
                completed_event_keys.push(candidate.event_key().to_owned());
            }
            Err(error) => {
                eprintln!("raw body source preparation failed: {error:?}");
                import_failures.push(RawBodyImportFailure {
                    event_key,
                    failure_kind: raw_body_failure_kind(&error),
                });
            }
        }
    }

    let stores_new_body = prepared_imports
        .iter()
        .any(|prepared| prepared.stores_body());
    let inserted_event_keys = if prepared_imports.is_empty() {
        Vec::new()
    } else {
        let mut store = state.store.lock().await;
        store
            .commit_prepared_raw_body_imports(&prepared_imports)
            .map_err(|_| IngestError::StoreWriteFailed)?
    };
    let inserted_event_keys: HashSet<String> = inserted_event_keys.into_iter().collect();
    let mut cleanup_imports = Vec::new();
    for prepared in prepared_imports {
        if !prepared.stores_body() || inserted_event_keys.contains(prepared.event_key()) {
            cleanup_imports.push(prepared);
        } else {
            import_failures.push(RawBodyImportFailure {
                event_key: prepared.event_key().to_owned(),
                failure_kind: RawBodyImportFailureKind::Io,
            });
        }
    }

    let cleanup_report = cleanup_prepared_raw_body_imports(cleanup_imports);
    for cleanup_error in cleanup_report.cleanup_errors {
        eprintln!(
            "raw body cleanup failed for event_key={} body_ref={}: {:?}",
            cleanup_error.event_key, cleanup_error.body_ref, cleanup_error.error
        );
        import_failures.push(RawBodyImportFailure {
            event_key: cleanup_error.event_key,
            failure_kind: raw_body_failure_kind(&cleanup_error.error),
        });
    }
    completed_event_keys.extend(cleanup_report.completed_event_keys);

    if !completed_event_keys.is_empty() || !import_failures.is_empty() {
        let mut store = state.store.lock().await;
        store
            .complete_raw_body_imports(&completed_event_keys)
            .map_err(|_| IngestError::StoreWriteFailed)?;
        store
            .record_raw_body_import_failures(&import_failures)
            .map_err(|_| IngestError::StoreWriteFailed)?;
    }

    if stores_new_body {
        let _ = state.usage_updates.send(());
    }
    Ok(stores_new_body || !completed_event_keys.is_empty())
}

fn raw_body_failure_kind(error: &StoreError) -> RawBodyImportFailureKind {
    match error {
        StoreError::RawBodyPathEscapesDirectory
        | StoreError::RawBodyNotRegularFile
        | StoreError::RawBodyPathChangedBeforeDelete => RawBodyImportFailureKind::InvalidSource,
        StoreError::RawBodySourceGrewBeforeDelete => RawBodyImportFailureKind::Io,
        StoreError::RawBodyIo(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RawBodyImportFailureKind::Missing
        }
        StoreError::RawBodyIo(_) => RawBodyImportFailureKind::Io,
        _ => RawBodyImportFailureKind::Io,
    }
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

async fn serve_rpc(listener: UnixListener, state: DaemonState, shutdown: broadcast::Sender<()>) {
    let mut shutdown_receiver = shutdown.subscribe();
    loop {
        let (stream, _addr) = tokio::select! {
            biased;
            _ = shutdown_receiver.recv() => return,
            accepted = listener.accept() => {
                match accepted {
                    Ok(connection) => connection,
                    Err(err) => {
                        eprintln!("kvasird rpc accept failed: {err}");
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        continue;
                    }
                }
            }
        };
        let connection_state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_rpc_connection(stream, connection_state).await {
                eprintln!("kvasird rpc connection failed: {err:#}");
            }
        });
    }
}

async fn handle_rpc_connection(stream: UnixStream, state: DaemonState) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let request = read_bounded_line(
        &mut reader,
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
        Ok(RpcRequest::OverviewRollup { query }) => {
            let store = state.store.lock().await;
            match overview_rollups(&store, query) {
                Ok(rollup) => RpcResponse::OverviewRollup { rollup },
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
        Ok(RpcRequest::ToolCallRollup { query }) => {
            match state.store.lock().await.tool_call_rollups(query) {
                Ok(rollups) => RpcResponse::ToolCallRollup { rollups },
                Err(_err) => RpcResponse::Error {
                    error: RpcError::Internal,
                },
            }
        }
        Ok(RpcRequest::Trace { query }) => match state.store.lock().await.traces(query) {
            Ok(traces) => RpcResponse::Trace { traces },
            Err(_err) => RpcResponse::Error {
                error: RpcError::Internal,
            },
        },
        Ok(RpcRequest::Content {
            query,
            bearer_token,
        }) => {
            if bearer_token != state.bearer_token {
                RpcResponse::Error {
                    error: RpcError::Unauthorized,
                }
            } else {
                match state.store.lock().await.content_replay(query) {
                    Ok(replay) => RpcResponse::Content { replay },
                    Err(_err) => RpcResponse::Error {
                        error: RpcError::Internal,
                    },
                }
            }
        }
        Ok(RpcRequest::SubscribeTokenRollup { query }) => {
            let mut updates = state.usage_updates.subscribe();
            let mut shutdown = state.shutdown.subscribe();
            let mut disconnect_probe = [0_u8; 1];
            let mut last_event = None;
            match write_token_rollup_event(&mut writer, &state, query.clone(), &mut shutdown)
                .await?
            {
                StreamWriteOutcome::Written(event) => last_event = Some(event),
                StreamWriteOutcome::Unchanged => {}
                StreamWriteOutcome::Shutdown => return Ok(()),
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.recv() => return Ok(()),
                    disconnected = reader.read(&mut disconnect_probe) => {
                        match disconnected {
                            Ok(0) | Err(_) => return Ok(()),
                            Ok(_) => return Ok(()),
                        }
                    }
                    update = updates.recv() => {
                        match update {
                            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                                match write_token_rollup_event_if_changed(
                                    &mut writer,
                                    &state,
                                    query.clone(),
                                    &mut shutdown,
                                    last_event.as_ref(),
                                ).await {
                                    Ok(StreamWriteOutcome::Written(event)) => last_event = Some(event),
                                    Ok(StreamWriteOutcome::Unchanged) => {}
                                    Ok(StreamWriteOutcome::Shutdown) | Err(_) => return Ok(()),
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        }
                    }
                }
            }
        }
        Err(_err) => RpcResponse::Error {
            error: RpcError::InvalidRequest,
        },
    };
    write_rpc_response(&mut writer, response).await?;
    Ok(())
}

fn overview_rollups(
    store: &kvasir_core::UsageStore,
    query: RollupQuery,
) -> Result<OverviewRollup, kvasir_core::store::StoreError> {
    Ok(OverviewRollup {
        token_rollups: store.token_rollups(query.clone())?,
        cost_rollups: store.cost_rollups(CostRollupQuery {
            start: query.start,
            end: query.end,
            repo: query.repo.clone(),
            model: query.model.clone(),
        })?,
        tool_call_rollups: store.tool_call_rollups(ToolCallRollupQuery {
            start: query.start,
            end: query.end,
            repo: query.repo,
            model: query.model,
        })?,
    })
}

async fn write_rpc_response<W>(writer: &mut W, response: RpcResponse) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut response_bytes = serde_json::to_vec(&response)?;
    if response_bytes.len() + 1 > MAX_RPC_RESPONSE_BYTES {
        response_bytes = serde_json::to_vec(&RpcResponse::Error {
            error: RpcError::ResponseTooLarge,
        })?;
    }
    response_bytes.push(b'\n');
    writer.write_all(&response_bytes).await?;
    Ok(())
}

enum StreamWriteOutcome {
    Written(RpcStreamEvent),
    Unchanged,
    Shutdown,
}

async fn write_token_rollup_event<W>(
    writer: &mut W,
    state: &DaemonState,
    query: RollupQuery,
    shutdown: &mut broadcast::Receiver<()>,
) -> anyhow::Result<StreamWriteOutcome>
where
    W: AsyncWrite + Unpin,
{
    write_token_rollup_event_if_changed(writer, state, query, shutdown, None).await
}

async fn write_token_rollup_event_if_changed<W>(
    writer: &mut W,
    state: &DaemonState,
    query: RollupQuery,
    shutdown: &mut broadcast::Receiver<()>,
    last_event: Option<&RpcStreamEvent>,
) -> anyhow::Result<StreamWriteOutcome>
where
    W: AsyncWrite + Unpin,
{
    let event = match state.store.lock().await.token_rollups(query) {
        Ok(rollups) => RpcStreamEvent::TokenRollup { rollups },
        Err(_err) => RpcStreamEvent::Error {
            error: RpcError::Internal,
        },
    };
    if last_event == Some(&event) {
        return Ok(StreamWriteOutcome::Unchanged);
    }
    let mut event_bytes = serde_json::to_vec(&event)?;
    if event_bytes.len() >= MAX_RPC_RESPONSE_BYTES {
        let bounded_error = RpcStreamEvent::Error {
            error: RpcError::Internal,
        };
        if last_event == Some(&bounded_error) {
            return Ok(StreamWriteOutcome::Unchanged);
        }
        event_bytes = serde_json::to_vec(&bounded_error)?;
        return write_rpc_stream_event(writer, shutdown, bounded_error, event_bytes).await;
    }
    write_rpc_stream_event(writer, shutdown, event, event_bytes).await
}

async fn write_rpc_stream_event<W>(
    writer: &mut W,
    shutdown: &mut broadcast::Receiver<()>,
    event: RpcStreamEvent,
    mut event_bytes: Vec<u8>,
) -> anyhow::Result<StreamWriteOutcome>
where
    W: AsyncWrite + Unpin,
{
    event_bytes.push(b'\n');
    tokio::select! {
        biased;
        _ = shutdown.recv() => return Ok(StreamWriteOutcome::Shutdown),
        result = tokio::time::timeout(RPC_WRITE_TIMEOUT, writer.write_all(&event_bytes)) => {
            result??;
        }
    }
    Ok(StreamWriteOutcome::Written(event))
}

async fn read_bounded_line<R>(
    reader: &mut R,
    byte_limit: usize,
    limit_error: DaemonError,
) -> Result<String, DaemonError>
where
    R: AsyncBufRead + Unpin,
{
    let mut request = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return String::from_utf8(request).map_err(DaemonError::InvalidRpcUtf8);
        }

        let bytes_to_consume = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        if request.len() + bytes_to_consume > byte_limit {
            return Err(limit_error);
        }
        request.extend_from_slice(&available[..bytes_to_consume]);
        reader.consume(bytes_to_consume);
        if request.last() == Some(&b'\n') {
            return String::from_utf8(request).map_err(DaemonError::InvalidRpcUtf8);
        }
    }
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
    #[cfg(unix)]
    fn invalid_utf8_database_paths_have_distinct_keychain_users() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first_path = PathBuf::from(OsString::from_vec(b"invalid-\xff.sqlite3".to_vec()));
        let second_path = PathBuf::from(OsString::from_vec(b"invalid-\xfe.sqlite3".to_vec()));

        assert_eq!(first_path.to_string_lossy(), second_path.to_string_lossy());
        assert_ne!(
            keychain_user_for_database(&first_path),
            keychain_user_for_database(&second_path)
        );
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
