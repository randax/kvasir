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
    #[error("Claude Code settings JSON is invalid")]
    HarnessTelemetryInvalidClaudeSettings,
    #[error("Codex telemetry config cannot be updated automatically")]
    HarnessTelemetryInvalidCodexConfig,
    #[error("OpenCode config JSON is invalid")]
    HarnessTelemetryInvalidOpenCodeConfig,
    #[error("stored harness telemetry setup secret is invalid")]
    HarnessTelemetryInvalidStoredSecret,
    #[error("harness telemetry setup rollback failed")]
    HarnessTelemetryRollback,
    #[error("harness telemetry setup state is unknown")]
    HarnessTelemetryStateUnknown,
    #[error("harness telemetry uninstall would overwrite user changes")]
    HarnessTelemetryUninstallConflict,
    #[error("filesystem operation failed")]
    Filesystem,
}
