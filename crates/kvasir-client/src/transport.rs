use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

use kvasir_core::rpc::{RpcRequest, RpcResponse, RpcStreamEvent};

use crate::error::KvasirClientError;

const MAX_RPC_RESPONSE_BYTES: u64 = 16 * 1024;
const CONNECT_RETRY_ATTEMPTS: usize = 80;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);

pub(crate) fn send_rpc_request(
    socket_path: &Path,
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

pub(crate) fn connect_with_retries(socket_path: &Path) -> Result<UnixStream, KvasirClientError> {
    for attempt in 0..CONNECT_RETRY_ATTEMPTS {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(_err) if attempt + 1 < CONNECT_RETRY_ATTEMPTS => {
                thread::sleep(CONNECT_RETRY_DELAY);
            }
            Err(_err) => return Err(KvasirClientError::SocketIo),
        }
    }
    Err(KvasirClientError::SocketIo)
}

pub(crate) fn read_rpc_stream_event<R>(reader: &mut R) -> Result<RpcStreamEvent, KvasirClientError>
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
