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
