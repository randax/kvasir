use std::collections::HashSet;
use std::fs::File;
use std::net::SocketAddr;
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use kvasir_core::rpc::{
    BearerToken, ContentQuery, ContentReplay, CostRollup, CostRollupQuery, OverviewRollup,
    RollupQuery, RpcError, RpcRequest, RpcResponse, RpcStreamEvent, TokenRollup, ToolCallRollup,
    ToolCallRollupQuery, Trace, TraceQuery, UsageUpdateKind,
};
use kvasir_core::{
    ContentRetentionPolicy, ContentRetentionReport, PriceTable, RawBodyImportFailure,
    RawBodyImportFailureKind, RawBodyImportPreparation, StoreError, StoreKey, UsageStore,
    VerifiedRawBodyDirectory, cleanup_invalid_raw_body_candidate,
    cleanup_prepared_raw_body_imports, parse_otlp_json_traces, parse_otlp_json_usage_logs,
    parse_otlp_json_usage_metrics, parse_otlp_protobuf_traces, parse_otlp_protobuf_usage_logs,
    parse_otlp_protobuf_usage_metrics, prepare_raw_body_import_candidate,
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
const DEFAULT_CONTENT_RETENTION_COMPACTION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const LOCK_UN: c_int = 8;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

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
    pub content_retention_policy: ContentRetentionPolicy,
    pub content_retention_schedule: ContentRetentionSchedule,
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
            content_retention_policy: ContentRetentionPolicy::default(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRetentionSchedule {
    interval: Duration,
    window_start_utc: Duration,
}

impl ContentRetentionSchedule {
    pub fn new(interval: Duration, window_start_utc: Duration) -> Result<Self, ScheduleError> {
        if interval.is_zero() {
            return Err(ScheduleError::ZeroInterval);
        }
        if window_start_utc >= Duration::from_secs(SECONDS_PER_DAY) {
            return Err(ScheduleError::WindowStartOutsideDay);
        }
        Ok(Self {
            interval,
            window_start_utc,
        })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn window_start_utc(&self) -> Duration {
        self.window_start_utc
    }
}

impl Default for ContentRetentionSchedule {
    fn default() -> Self {
        Self {
            interval: DEFAULT_CONTENT_RETENTION_COMPACTION_INTERVAL,
            window_start_utc: Duration::ZERO,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("content retention compaction interval must be greater than zero")]
    ZeroInterval,
    #[error("content retention compaction window start must be within one UTC day")]
    WindowStartOutsideDay,
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

    fn resolve(
        &self,
        missing_key_policy: MissingStoreKeyPolicy,
    ) -> anyhow::Result<ResolvedStoreKey> {
        match self {
            Self::Keychain(source) => source.resolve(missing_key_policy),
            Self::Static(key) => Ok(ResolvedStoreKey::existing(key.clone())),
        }
    }

    fn persist_generated_key(&self, resolved_key: &ResolvedStoreKey) -> anyhow::Result<()> {
        if !resolved_key.was_generated {
            return Ok(());
        }
        match self {
            Self::Keychain(source) => source.persist(&resolved_key.key),
            Self::Static(_key) => Ok(()),
        }
    }
}

struct ResolvedStoreKey {
    key: StoreKey,
    was_generated: bool,
}

impl ResolvedStoreKey {
    fn existing(key: StoreKey) -> Self {
        Self {
            key,
            was_generated: false,
        }
    }

    fn generated(key: StoreKey) -> Self {
        Self {
            key,
            was_generated: true,
        }
    }
}

#[derive(Clone, Copy)]
enum MissingStoreKeyPolicy {
    Create,
    RequireExisting,
}

impl MissingStoreKeyPolicy {
    fn for_database_requires_key(database_requires_key: bool) -> Self {
        if database_requires_key {
            Self::RequireExisting
        } else {
            Self::Create
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

    fn resolve(
        &self,
        missing_key_policy: MissingStoreKeyPolicy,
    ) -> anyhow::Result<ResolvedStoreKey> {
        let entry = keyring::Entry::new(self.service, &self.user)?;
        resolve_store_key(&KeyringStoreKeyCredential { entry }, missing_key_policy)
    }

    fn persist(&self, key: &StoreKey) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(self.service, &self.user)?;
        let encoded_key = key.to_hex_secret();
        KeyringStoreKeyCredential { entry }.set_password(&encoded_key)
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

fn resolve_store_key(
    credential: &impl StoreKeyCredential,
    missing_key_policy: MissingStoreKeyPolicy,
) -> anyhow::Result<ResolvedStoreKey> {
    match credential.get_password() {
        Ok(encoded_key) => {
            let encoded_key = Zeroizing::new(encoded_key);
            Ok(ResolvedStoreKey::existing(StoreKey::from_hex(
                &encoded_key,
            )?))
        }
        Err(StoreKeyCredentialReadError::NoEntry)
            if matches!(missing_key_policy, MissingStoreKeyPolicy::Create) =>
        {
            Ok(ResolvedStoreKey::generated(StoreKey::generate()?))
        }
        Err(StoreKeyCredentialReadError::NoEntry) => {
            Err(DaemonError::StoreKeyMissingForExistingDatabase.into())
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

    #[doc(hidden)]
    pub async fn compact_content_retention_once(
        &self,
        policy: &ContentRetentionPolicy,
        compacted_at: kvasir_core::rpc::TimestampMillis,
    ) -> anyhow::Result<ContentRetentionReport> {
        compact_content_retention(&self.state, policy, compacted_at, true)
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
    content_retention_policy: ContentRetentionPolicy,
    raw_body_directory: VerifiedRawBodyDirectory,
    overview_updates: broadcast::Sender<()>,
    usage_updates: broadcast::Sender<()>,
    shutdown: broadcast::Sender<()>,
}

pub async fn start(config: DaemonConfig) -> anyhow::Result<RunningDaemon> {
    let stable_database_path = canonical_database_path(&config.database_path);
    let store_key_source = StoreKeySource::keychain_for_database(&stable_database_path);
    start_with_store_key_source_at_stable_path(config, store_key_source, stable_database_path).await
}

pub async fn start_with_store_key_source(
    config: DaemonConfig,
    store_key_source: StoreKeySource,
) -> anyhow::Result<RunningDaemon> {
    let stable_database_path = canonical_database_path(&config.database_path);
    start_with_store_key_source_at_stable_path(config, store_key_source, stable_database_path).await
}

async fn start_with_store_key_source_at_stable_path(
    config: DaemonConfig,
    store_key_source: StoreKeySource,
    stable_database_path: PathBuf,
) -> anyhow::Result<RunningDaemon> {
    let _startup_lock = StoreStartupLock::acquire(&stable_database_path).await?;
    let database_requires_key =
        database_requires_existing_store_key_at_stable_path(&stable_database_path)?;
    let bootstrap_cleanup =
        BootstrapDatabaseCleanup::capture(&stable_database_path, database_requires_key)?;
    let resolved_store_key = store_key_source.resolve(
        MissingStoreKeyPolicy::for_database_requires_key(database_requires_key),
    )?;
    store_key_source.persist_generated_key(&resolved_store_key)?;
    let bootstrap_cleanup = match bootstrap_cleanup.prepare_for_database_open(&stable_database_path)
    {
        Ok(bootstrap_cleanup) => bootstrap_cleanup,
        Err(err) => return Err(err.into()),
    };
    let store = match UsageStore::open_with_price_table(
        &stable_database_path,
        &resolved_store_key.key,
        config.price_table.clone(),
    ) {
        Ok(store) => store,
        Err(err) => {
            if resolved_store_key.was_generated {
                rollback_failed_generated_key_bootstrap(
                    &stable_database_path,
                    &bootstrap_cleanup,
                    &err,
                )?;
            }
            return Err(err.into());
        }
    };
    let raw_body_directory = prepare_raw_body_directory(&config.database_path)?;
    let (overview_updates, _overview_update_receiver) = broadcast::channel(32);
    let (usage_updates, _usage_update_receiver) = broadcast::channel(32);
    let (shutdown, _shutdown_receiver) = broadcast::channel(1);
    let state = DaemonState {
        store: Arc::new(Mutex::new(store)),
        bearer_token: config.bearer_token,
        content_retention_policy: config.content_retention_policy.clone(),
        raw_body_directory,
        overview_updates,
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
    let content_retention_state = state.clone();
    let content_retention_shutdown = shutdown.clone();
    let content_retention_policy = config.content_retention_policy.clone();
    let content_retention_schedule = config.content_retention_schedule;
    let content_retention_task = tokio::spawn(async move {
        run_content_retention_compactor(
            content_retention_state,
            content_retention_shutdown,
            content_retention_policy,
            content_retention_schedule,
        )
        .await;
    });

    Ok(RunningDaemon {
        otlp_addr,
        rpc_socket_path: config.rpc_socket_path,
        shutdown,
        tasks: vec![
            http_task,
            rpc_task,
            raw_body_import_task,
            content_retention_task,
        ],
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

async fn run_content_retention_compactor(
    state: DaemonState,
    shutdown: broadcast::Sender<()>,
    policy: ContentRetentionPolicy,
    schedule: ContentRetentionSchedule,
) {
    if policy.keeps_forever() {
        return;
    }
    let mut shutdown = shutdown.subscribe();
    if let Err(error) =
        compact_content_retention(&state, &policy, current_timestamp_millis(), false).await
    {
        eprintln!("content retention compaction failed: {error:?}");
    }
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now()
            + duration_until_next_retention_window(SystemTime::now(), schedule),
        schedule.interval(),
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = interval.tick() => {
                if let Err(error) = compact_content_retention(&state, &policy, current_timestamp_millis(), true).await {
                    eprintln!("content retention compaction failed: {error:?}");
                }
            }
        }
    }
}

async fn compact_content_retention(
    state: &DaemonState,
    policy: &ContentRetentionPolicy,
    compacted_at: kvasir_core::rpc::TimestampMillis,
    run_maintenance: bool,
) -> Result<ContentRetentionReport, StoreError> {
    let mut store = state.store.lock().await;
    let report = store.compact_content_retention(policy, compacted_at)?;
    if run_maintenance {
        store.run_pending_maintenance();
    }
    Ok(report)
}

fn current_timestamp_millis() -> kvasir_core::rpc::TimestampMillis {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let millis = i64::try_from(millis).unwrap_or(i64::MAX);
    kvasir_core::rpc::TimestampMillis::from_millis(millis)
}

fn duration_until_next_retention_window(
    now: SystemTime,
    schedule: ContentRetentionSchedule,
) -> Duration {
    let now = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let now_ms = i128::try_from(now.as_millis()).unwrap_or(i128::MAX);
    let interval_ms = i128::try_from(schedule.interval().as_millis()).unwrap_or(i128::MAX);
    let window_start_ms =
        i128::try_from(schedule.window_start_utc().as_millis()).unwrap_or(i128::MAX);
    let elapsed_since_window = (now_ms - window_start_ms).rem_euclid(interval_ms);
    let wait_ms = if elapsed_since_window == 0 {
        interval_ms
    } else {
        interval_ms - elapsed_since_window
    };
    Duration::from_millis(u64::try_from(wait_ms).unwrap_or(u64::MAX))
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

fn prepare_raw_body_directory(database_path: &Path) -> std::io::Result<VerifiedRawBodyDirectory> {
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
    VerifiedRawBodyDirectory::open(&raw_body_directory)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
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
    async fn acquire(stable_database_path: &Path) -> anyhow::Result<Self> {
        let lock_path = store_startup_lock_path_for_stable_database_path(stable_database_path);
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

#[cfg(test)]
fn store_startup_lock_path(database_path: &Path) -> PathBuf {
    let stable_path = canonical_database_path(database_path);
    store_startup_lock_path_for_stable_database_path(&stable_path)
}

fn store_startup_lock_path_for_stable_database_path(stable_path: &Path) -> PathBuf {
    let mut lock_path = stable_path.as_os_str().to_os_string();
    lock_path.push(".startup-lock");
    PathBuf::from(lock_path)
}

#[cfg(test)]
fn database_requires_existing_store_key(database_path: &Path) -> std::io::Result<bool> {
    let stable_database_path = canonical_database_path(database_path);
    database_requires_existing_store_key_at_stable_path(&stable_database_path)
}

fn database_requires_existing_store_key_at_stable_path(
    stable_database_path: &Path,
) -> std::io::Result<bool> {
    reject_stable_database_symlink(stable_database_path)?;
    let main_database_requires_key = match std::fs::metadata(stable_database_path) {
        Ok(metadata) if metadata.is_file() => metadata.len() > 0,
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "database path is not a regular file: {}",
                    stable_database_path.display()
                ),
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };
    if main_database_requires_key {
        return Ok(true);
    }

    sqlite_sidecar_exists(stable_database_path)
}

fn reject_stable_database_symlink(stable_database_path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(stable_database_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "database path is not a regular file: {}",
                stable_database_path.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn sqlite_sidecar_exists(database_path: &Path) -> std::io::Result<bool> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar_path = sqlite_sidecar_path(database_path, suffix);
        match std::fs::symlink_metadata(&sidecar_path) {
            Ok(metadata) if metadata.is_file() => return Ok(true),
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "sqlite sidecar path is not a regular file: {}",
                        sidecar_path.display()
                    ),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(false)
}

enum BootstrapDatabaseCleanup {
    None,
    RemoveCreatedDatabase,
    RemoveCreatedDatabaseFile { dev: u64, ino: u64 },
    RestoreEmptyDatabaseFile { dev: u64, ino: u64 },
}

impl BootstrapDatabaseCleanup {
    fn capture(database_path: &Path, database_requires_key: bool) -> std::io::Result<Self> {
        if database_requires_key {
            return Ok(Self::None);
        }

        match std::fs::metadata(database_path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == 0 => {
                Ok(Self::RestoreEmptyDatabaseFile {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                })
            }
            Ok(_) => Ok(Self::None),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::RemoveCreatedDatabase)
            }
            Err(err) => Err(err),
        }
    }

    fn prepare_for_database_open(self, database_path: &Path) -> std::io::Result<Self> {
        match self {
            Self::RemoveCreatedDatabase => {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(database_path)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "created database placeholder is not a regular file",
                    ));
                }
                Ok(Self::RemoveCreatedDatabaseFile {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                })
            }
            other => Ok(other),
        }
    }

    fn clean_after_failed_bootstrap(&self, database_path: &Path) -> std::io::Result<()> {
        match self {
            Self::None => Ok(()),
            Self::RemoveCreatedDatabase => Ok(()),
            Self::RemoveCreatedDatabaseFile { dev, ino }
            | Self::RestoreEmptyDatabaseFile { dev, ino } => {
                let file = open_verified_database_path(database_path, *dev, *ino)?;
                ensure_no_sqlite_sidecar_files(database_path)?;
                file.set_len(0)
            }
        }
    }
}

fn rollback_failed_generated_key_bootstrap(
    database_path: &Path,
    bootstrap_cleanup: &BootstrapDatabaseCleanup,
    original_error: &StoreError,
) -> anyhow::Result<()> {
    rollback_failed_generated_key_bootstrap_with(database_path, original_error, || {
        bootstrap_cleanup.clean_after_failed_bootstrap(database_path)
    })
}

fn rollback_failed_generated_key_bootstrap_with(
    database_path: &Path,
    original_error: &StoreError,
    cleanup: impl FnOnce() -> std::io::Result<()>,
) -> anyhow::Result<()> {
    if let Err(cleanup_error) = cleanup() {
        return Err(anyhow::anyhow!(
            "store bootstrap failed and database cleanup failed for {}; generated key was preserved: {cleanup_error}; original error: {original_error}",
            database_path.display()
        ));
    }

    Ok(())
}

fn open_verified_database_path(
    database_path: &Path,
    expected_dev: u64,
    expected_ino: u64,
) -> std::io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(database_path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.dev() != expected_dev || metadata.ino() != expected_ino {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "database placeholder changed before rollback",
        ));
    }
    Ok(file)
}

fn ensure_no_sqlite_sidecar_files(database_path: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar_path = sqlite_sidecar_path(database_path, suffix);
        match std::fs::symlink_metadata(&sidecar_path) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("sqlite sidecar exists during bootstrap rollback: {sidecar_path:?}"),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
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

pub async fn query_overview_rollup(
    socket_path: impl Into<PathBuf>,
    query: RollupQuery,
) -> anyhow::Result<OverviewRollup> {
    let mut stream = UnixStream::connect(socket_path.into()).await?;
    let request = RpcRequest::OverviewRollup { query };
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
        RpcResponse::OverviewRollup { rollup } => Ok(rollup),
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
        if !state.content_retention_policy.keeps_forever() {
            store
                .compact_content_retention(
                    &state.content_retention_policy,
                    current_timestamp_millis(),
                )
                .map_err(|_| IngestError::StoreWriteFailed)?;
        }
    }
    import_available_raw_bodies(&state).await?;
    let _ = state.overview_updates.send(());

    Ok(StatusCode::ACCEPTED)
}

async fn import_available_raw_bodies(state: &DaemonState) -> Result<bool, IngestError> {
    let candidates = {
        let store = state.store.lock().await;
        store
            .raw_body_import_candidates(RAW_BODY_IMPORT_BATCH_SIZE)
            .map_err(|error| {
                eprintln!("raw body candidate query failed: {error:?}");
                IngestError::StoreWriteFailed
            })?
    };
    if candidates.is_empty() {
        return Ok(false);
    }

    let mut prepared_imports = Vec::new();
    let mut completed_event_keys = Vec::new();
    let mut import_failures = Vec::new();
    for candidate in candidates {
        let event_key = candidate.event_key().to_owned();
        let cleanup_candidate = candidate.clone();
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
                let failure_kind = raw_body_failure_kind(&error);
                if failure_kind == RawBodyImportFailureKind::InvalidSource {
                    match cleanup_invalid_raw_body_candidate(
                        &state.raw_body_directory,
                        &cleanup_candidate,
                    ) {
                        Ok(()) => {
                            completed_event_keys.push(event_key);
                            continue;
                        }
                        Err(cleanup_error) => {
                            eprintln!("raw body invalid source cleanup failed: {cleanup_error:?}");
                            import_failures.push(RawBodyImportFailure {
                                event_key,
                                failure_kind,
                            });
                            continue;
                        }
                    }
                }
                import_failures.push(RawBodyImportFailure {
                    event_key,
                    failure_kind,
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
            .commit_prepared_raw_body_imports_with_retention(
                &prepared_imports,
                &state.content_retention_policy,
                current_timestamp_millis(),
            )
            .map_err(|error| {
                eprintln!("raw body import commit failed: {error:?}");
                IngestError::StoreWriteFailed
            })?
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
            .map_err(|error| {
                eprintln!("raw body import completion failed: {error:?}");
                IngestError::StoreWriteFailed
            })?;
        store
            .record_raw_body_import_failures(&import_failures)
            .map_err(|error| {
                eprintln!("raw body import failure recording failed: {error:?}");
                IngestError::StoreWriteFailed
            })?;
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
        StoreError::RawBodyImportUnsupportedPlatform => {
            RawBodyImportFailureKind::UnsupportedPlatform
        }
        StoreError::RawBodySourceGrewBeforeDelete => RawBodyImportFailureKind::Io,
        #[cfg(unix)]
        StoreError::RawBodyIo(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            RawBodyImportFailureKind::InvalidSource
        }
        StoreError::RawBodyIo(error) if error.kind() == std::io::ErrorKind::IsADirectory => {
            RawBodyImportFailureKind::InvalidSource
        }
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
            let mut updates = state.overview_updates.subscribe();
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
        Ok(RpcRequest::SubscribeUsageUpdates) => {
            let mut updates = state.overview_updates.subscribe();
            let mut shutdown = state.shutdown.subscribe();
            let mut disconnect_probe = [0_u8; 1];
            match write_usage_update_event(&mut writer, &mut shutdown, UsageUpdateKind::Initial)
                .await?
            {
                StreamWriteOutcome::Written(_) => {}
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
                                match write_usage_update_event(
                                    &mut writer,
                                    &mut shutdown,
                                    UsageUpdateKind::Changed,
                                ).await {
                                    Ok(StreamWriteOutcome::Written(_)) => {}
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
    let session_summary_page = store.session_summaries(query.clone())?;
    let prompt_summary_page = store.prompt_summaries(query.clone())?;
    Ok(OverviewRollup {
        token_rollups: store.token_rollups(query.clone())?,
        cost_rollups: store.cost_rollups(CostRollupQuery {
            start: query.start,
            end: query.end,
            repo: query.repo.clone(),
            harness: query.harness.clone(),
            model: query.model.clone(),
            session_id: query.session_id.clone(),
            prompt_id: query.prompt_id.clone(),
        })?,
        harness_summaries: store.harness_summaries(query.clone())?,
        tool_call_rollups: store.tool_call_rollups(ToolCallRollupQuery {
            start: query.start,
            end: query.end,
            repo: query.repo,
            harness: query.harness,
            model: query.model,
            session_id: query.session_id,
            prompt_id: query.prompt_id,
        })?,
        session_summaries: session_summary_page.summaries,
        session_summaries_more_available: session_summary_page.more_available,
        prompt_summaries: prompt_summary_page.summaries,
        prompt_summaries_more_available: prompt_summary_page.more_available,
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

async fn write_usage_update_event<W>(
    writer: &mut W,
    shutdown: &mut broadcast::Receiver<()>,
    kind: UsageUpdateKind,
) -> anyhow::Result<StreamWriteOutcome>
where
    W: AsyncWrite + Unpin,
{
    let event = RpcStreamEvent::UsageUpdate { kind };
    let event_bytes = serde_json::to_vec(&event)?;
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
    #[error("store key is missing for existing encrypted database")]
    StoreKeyMissingForExistingDatabase,
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn store_key_resolver_generates_once_and_reuses_stored_key() -> anyhow::Result<()> {
        let credential = MemoryStoreKeyCredential::default();

        let first_key = resolve_store_key(&credential, MissingStoreKeyPolicy::Create)?;
        assert!(first_key.was_generated);
        assert_eq!(credential.set_count.get(), 0);

        let encoded_key = first_key.key.to_hex_secret();
        credential.set_password(&encoded_key)?;
        let second_key = resolve_store_key(&credential, MissingStoreKeyPolicy::RequireExisting)?;

        assert!(!second_key.was_generated);
        assert_eq!(first_key.key, second_key.key);
        assert_eq!(credential.set_count.get(), 1);
        assert_eq!(credential.password.borrow().as_ref().unwrap().len(), 64);

        Ok(())
    }

    #[test]
    fn store_key_resolver_does_not_generate_for_existing_database_without_key() -> anyhow::Result<()>
    {
        let credential = MemoryStoreKeyCredential::default();

        let result = resolve_store_key(&credential, MissingStoreKeyPolicy::RequireExisting);
        let error = match result {
            Ok(_) => anyhow::bail!("missing key should fail for existing database"),
            Err(error) => error,
        };

        assert!(matches!(
            error.downcast_ref::<DaemonError>(),
            Some(DaemonError::StoreKeyMissingForExistingDatabase)
        ));
        assert_eq!(credential.set_count.get(), 0);

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
    fn content_retention_scheduler_targets_configured_utc_window() {
        let default_schedule = ContentRetentionSchedule::default();
        assert_eq!(
            duration_until_next_retention_window(
                UNIX_EPOCH + Duration::from_secs(23 * 60 * 60 + 59 * 60 + 59),
                default_schedule,
            ),
            Duration::from_secs(1)
        );
        assert_eq!(
            duration_until_next_retention_window(
                UNIX_EPOCH
                    + Duration::from_secs(23 * 60 * 60 + 59 * 60 + 59)
                    + Duration::from_millis(750),
                default_schedule,
            ),
            Duration::from_millis(250)
        );
        assert_eq!(
            duration_until_next_retention_window(
                UNIX_EPOCH + Duration::from_secs(24 * 60 * 60),
                default_schedule,
            ),
            Duration::from_secs(24 * 60 * 60)
        );
        let two_thirty_daily = ContentRetentionSchedule::new(
            Duration::from_secs(24 * 60 * 60),
            Duration::from_secs(2 * 60 * 60 + 30 * 60),
        )
        .unwrap();
        assert_eq!(
            duration_until_next_retention_window(
                UNIX_EPOCH + Duration::from_secs(2 * 60 * 60),
                two_thirty_daily,
            ),
            Duration::from_secs(30 * 60)
        );
        let hourly_at_five_past = ContentRetentionSchedule::new(
            Duration::from_secs(60 * 60),
            Duration::from_secs(5 * 60),
        )
        .unwrap();
        assert_eq!(
            duration_until_next_retention_window(
                UNIX_EPOCH + Duration::from_secs(2 * 60 * 60 + 6 * 60),
                hourly_at_five_past,
            ),
            Duration::from_secs(59 * 60)
        );
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

    #[tokio::test]
    #[cfg(unix)]
    async fn startup_uses_stable_database_path_after_lock_wait_alias_retarget() -> anyhow::Result<()>
    {
        let temp = tempdir()?;
        let rpc_socket_path = temp.path().join("kvasird.sock");
        let first_database_path = temp.path().join("first.sqlite3");
        let second_database_path = temp.path().join("second.sqlite3");
        let symlink_database_path = temp.path().join("alias.sqlite3");
        File::create(&first_database_path)?;
        std::os::unix::fs::symlink(&first_database_path, &symlink_database_path)?;

        let stable_database_path = canonical_database_path(&symlink_database_path);
        assert_eq!(stable_database_path, first_database_path.canonicalize()?);
        let lock_path = store_startup_lock_path_for_stable_database_path(&stable_database_path);
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        assert!(try_lock_file(&lock_file)?);

        let config = DaemonConfig {
            otlp_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            rpc_socket_path,
            database_path: symlink_database_path.clone(),
            bearer_token: BearerToken::new("test-token"),
            price_table: PriceTable::bundled_defaults(),
            content_retention_policy: ContentRetentionPolicy::keep_forever(),
            content_retention_schedule: ContentRetentionSchedule::default(),
        };
        let start_future =
            start_with_store_key_source(config, StoreKeySource::static_key_for_test([17; 32]));
        tokio::pin!(start_future);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Future::poll(start_future.as_mut(), &mut context),
            Poll::Pending
        ));

        std::fs::remove_file(&symlink_database_path)?;
        std::os::unix::fs::symlink(&second_database_path, &symlink_database_path)?;
        drop(lock_file);

        let daemon = start_future.await?;

        assert!(std::fs::metadata(&first_database_path)?.len() > 0);
        assert!(!second_database_path.try_exists()?);

        drop(daemon);

        Ok(())
    }

    #[test]
    fn empty_placeholder_database_does_not_require_existing_store_key() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        File::create(&database_path)?;

        assert!(!database_requires_existing_store_key(&database_path)?);

        Ok(())
    }

    #[test]
    fn initialized_database_requires_existing_store_key() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        std::fs::write(&database_path, "not empty")?;

        assert!(database_requires_existing_store_key(&database_path)?);

        Ok(())
    }

    #[test]
    fn database_sidecar_requires_existing_store_key() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let missing_database_path = temp.path().join("missing.sqlite3");
        std::fs::write(
            sqlite_sidecar_path(&missing_database_path, "-wal"),
            "sidecar",
        )?;

        assert!(database_requires_existing_store_key(
            &missing_database_path
        )?);

        let empty_database_path = temp.path().join("empty.sqlite3");
        File::create(&empty_database_path)?;
        std::fs::write(
            sqlite_sidecar_path(&empty_database_path, "-journal"),
            "sidecar",
        )?;

        assert!(database_requires_existing_store_key(&empty_database_path)?);

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn database_sidecar_requires_existing_store_key_through_database_symlink() -> anyhow::Result<()>
    {
        let temp = tempdir()?;
        let real_database_path = temp.path().join("real.sqlite3");
        let symlink_database_path = temp.path().join("alias.sqlite3");
        File::create(&real_database_path)?;
        std::os::unix::fs::symlink(&real_database_path, &symlink_database_path)?;
        std::fs::write(
            sqlite_sidecar_path(&real_database_path, "-wal"),
            "canonical sidecar",
        )?;

        assert!(database_requires_existing_store_key(
            &symlink_database_path
        )?);

        Ok(())
    }

    #[test]
    fn non_regular_database_nodes_are_invalid_store_paths() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        std::fs::create_dir(&database_path)?;

        let error = match database_requires_existing_store_key(&database_path) {
            Ok(_) => anyhow::bail!("database directory should be invalid"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn dangling_database_symlink_is_an_invalid_store_path() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        std::os::unix::fs::symlink(temp.path().join("missing.sqlite3"), &database_path)?;

        let error = match database_requires_existing_store_key(&database_path) {
            Ok(_) => anyhow::bail!("dangling database symlink should be invalid"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    fn non_regular_sidecar_nodes_are_invalid_store_paths() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        File::create(&database_path)?;
        std::fs::create_dir(sqlite_sidecar_path(&database_path, "-wal"))?;

        let error = match database_requires_existing_store_key(&database_path) {
            Ok(_) => anyhow::bail!("sidecar directory should be invalid"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    fn empty_placeholder_restore_rejects_path_swap() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        File::create(&database_path)?;
        let BootstrapDatabaseCleanup::RestoreEmptyDatabaseFile { dev, ino } =
            BootstrapDatabaseCleanup::capture(&database_path, false)?
        else {
            anyhow::bail!("empty placeholder should require restore cleanup");
        };

        std::fs::remove_file(&database_path)?;
        File::create(&database_path)?;

        let error = match open_verified_database_path(&database_path, dev, ino) {
            Ok(_file) => anyhow::bail!("placeholder swap must fail rollback restore"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        Ok(())
    }

    #[test]
    fn empty_placeholder_cleanup_rejects_path_swap_before_sidecar_deletion() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        File::create(&database_path)?;
        let cleanup = BootstrapDatabaseCleanup::capture(&database_path, false)?;
        std::fs::remove_file(&database_path)?;
        File::create(&database_path)?;
        let wal_path = sqlite_sidecar_path(&database_path, "-wal");
        let shm_path = sqlite_sidecar_path(&database_path, "-shm");
        let journal_path = sqlite_sidecar_path(&database_path, "-journal");
        std::fs::write(&wal_path, "keep wal")?;
        std::fs::write(&shm_path, "keep shm")?;
        std::fs::write(&journal_path, "keep journal")?;

        let error = match cleanup.clean_after_failed_bootstrap(&database_path) {
            Ok(()) => anyhow::bail!("placeholder swap must fail cleanup"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_to_string(wal_path)?, "keep wal");
        assert_eq!(std::fs::read_to_string(shm_path)?, "keep shm");
        assert_eq!(std::fs::read_to_string(journal_path)?, "keep journal");

        Ok(())
    }

    #[test]
    fn created_database_cleanup_truncates_verified_path_without_sidecars() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let cleanup = BootstrapDatabaseCleanup::capture(&database_path, false)?
            .prepare_for_database_open(&database_path)?;
        std::fs::write(&database_path, "partial bootstrap database")?;

        cleanup.clean_after_failed_bootstrap(&database_path)?;

        assert!(database_path.try_exists()?);
        assert_eq!(std::fs::metadata(database_path)?.len(), 0);

        Ok(())
    }

    #[test]
    fn created_database_cleanup_preserves_verified_path_when_sidecar_exists() -> anyhow::Result<()>
    {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let cleanup = BootstrapDatabaseCleanup::capture(&database_path, false)?
            .prepare_for_database_open(&database_path)?;
        std::fs::write(&database_path, "partial bootstrap database")?;
        std::fs::write(sqlite_sidecar_path(&database_path, "-wal"), "sidecar")?;

        let error = match cleanup.clean_after_failed_bootstrap(&database_path) {
            Ok(()) => anyhow::bail!("sidecar must fail bootstrap cleanup"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(&database_path)?,
            "partial bootstrap database"
        );

        Ok(())
    }

    #[test]
    fn created_database_cleanup_rejects_path_swap_before_sidecar_deletion() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let cleanup = BootstrapDatabaseCleanup::capture(&database_path, false)?
            .prepare_for_database_open(&database_path)?;
        std::fs::remove_file(&database_path)?;
        File::create(&database_path)?;
        let wal_path = sqlite_sidecar_path(&database_path, "-wal");
        let shm_path = sqlite_sidecar_path(&database_path, "-shm");
        let journal_path = sqlite_sidecar_path(&database_path, "-journal");
        std::fs::write(&wal_path, "keep wal")?;
        std::fs::write(&shm_path, "keep shm")?;
        std::fs::write(&journal_path, "keep journal")?;

        let error = match cleanup.clean_after_failed_bootstrap(&database_path) {
            Ok(()) => anyhow::bail!("created database path swap must fail cleanup"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_to_string(wal_path)?, "keep wal");
        assert_eq!(std::fs::read_to_string(shm_path)?, "keep shm");
        assert_eq!(std::fs::read_to_string(journal_path)?, "keep journal");

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn empty_placeholder_restore_rejects_symlink_swap() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let target_path = temp.path().join("target.txt");
        File::create(&database_path)?;
        std::fs::write(&target_path, "do not truncate")?;
        let BootstrapDatabaseCleanup::RestoreEmptyDatabaseFile { dev, ino } =
            BootstrapDatabaseCleanup::capture(&database_path, false)?
        else {
            anyhow::bail!("empty placeholder should require restore cleanup");
        };

        std::fs::remove_file(&database_path)?;
        std::os::unix::fs::symlink(&target_path, &database_path)?;

        let error = match open_verified_database_path(&database_path, dev, ino) {
            Ok(_file) => anyhow::bail!("symlink swap must fail rollback restore"),
            Err(error) => error,
        };

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::read_to_string(&target_path)?, "do not truncate");

        Ok(())
    }

    #[test]
    fn failed_bootstrap_rollback_preserves_key_when_database_cleanup_fails() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let database_path = temp.path().join("usage.sqlite3");
        let original_error = StoreError::Sqlite(rusqlite::Error::ExecuteReturnedResults);

        let error = match rollback_failed_generated_key_bootstrap_with(
            &database_path,
            &original_error,
            || {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cleanup failed",
                ))
            },
        ) {
            Ok(()) => anyhow::bail!("cleanup failure must be surfaced"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("generated key was preserved"),
            "{error:?}"
        );

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
