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
    #[error("harness telemetry setup failed")]
    HarnessTelemetrySetup,
    #[error("harness telemetry setup rollback failed")]
    HarnessTelemetryRollback,
    #[error("harness telemetry setup state is unknown")]
    HarnessTelemetryStateUnknown,
    #[error("harness telemetry uninstall would overwrite user changes")]
    HarnessTelemetryUninstallConflict,
    #[error("filesystem operation failed")]
    Filesystem,
}
