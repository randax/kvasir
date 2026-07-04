use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kvasir_core::{StoreError, StoreKey, UsageStore};

const MAX_READ_CONNECTIONS: usize = 4;

/// Runs store operations off the async runtime. Writes serialize on the
/// single write connection behind a blocking mutex; reads run on pooled
/// read-only connections so queries never queue behind ingest or
/// maintenance. All SQLite work executes on the blocking thread pool.
#[derive(Clone)]
pub(crate) struct StoreHandle {
    writer: Arc<Mutex<UsageStore>>,
    readers: ReaderPool,
}

impl StoreHandle {
    pub(crate) fn new(store: UsageStore, database_path: PathBuf, key: StoreKey) -> Self {
        Self {
            writer: Arc::new(Mutex::new(store)),
            readers: ReaderPool::new(database_path, key),
        }
    }

    /// Runs a mutating operation on the write connection.
    pub(crate) async fn write<T, F>(&self, operation: F) -> T
    where
        F: FnOnce(&mut UsageStore) -> T + Send + 'static,
        T: Send + 'static,
    {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            // A poisoning panic cannot leave the store inconsistent: rusqlite
            // rolls back any transaction dropped mid-flight.
            let mut store = writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            operation(&mut store)
        })
        .await
        .expect("store write operation panicked")
    }

    /// Runs a query on a pooled read-only connection.
    pub(crate) async fn read<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        F: FnOnce(&UsageStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        self.readers.read(operation).await
    }
}

#[derive(Clone)]
struct ReaderPool {
    database_path: Arc<PathBuf>,
    key: StoreKey,
    idle: Arc<Mutex<Vec<UsageStore>>>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl ReaderPool {
    fn new(database_path: PathBuf, key: StoreKey) -> Self {
        Self {
            database_path: Arc::new(database_path),
            key,
            idle: Arc::new(Mutex::new(Vec::new())),
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_READ_CONNECTIONS)),
        }
    }

    async fn read<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        F: FnOnce(&UsageStore) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("reader pool semaphore closed");
        let idle = Arc::clone(&self.idle);
        let database_path = Arc::clone(&self.database_path);
        let key = self.key.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let pooled = idle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop();
            let reader = match pooled {
                Some(reader) => reader,
                None => UsageStore::open_reader(database_path.as_path(), &key)?,
            };
            let result = operation(&reader);
            idle.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(reader);
            result
        })
        .await
        .expect("store read operation panicked")
    }
}
