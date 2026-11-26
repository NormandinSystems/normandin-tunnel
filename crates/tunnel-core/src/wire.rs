//! Reading and writing frames on QUIC streams, with size checks and timeouts.

use std::time::Duration;

use iroh::endpoint::{ReadError, ReadExactError, RecvStream, SendStream};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tunnel_proto::frame::{self, FrameError};

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WireError {
    /// The stream ended before a whole frame arrived.
    #[error("stream closed")]
    Closed,
    /// The peer reset the stream with this code. A reset can overtake data the
    /// peer wrote just before it, so it may arrive in place of a frame.
    #[error("stream reset by peer ({0})")]
    Reset(u32),
    /// The whole connection is gone.
    #[error("connection lost")]
    ConnectionLost,
    /// Nothing arrived in time.
    #[error("timed out")]
    Timeout,
    /// The peer sent something that breaks the framing rules.
    #[error("bad frame: {0}")]
    Frame(#[from] FrameError),
}

/// Reads one frame payload, refusing lengths above `max` before allocating.
pub(crate) async fn read_payload(recv: &mut RecvStream, max: usize) -> Result<Vec<u8>, WireError> {
    let mut header = [0u8; frame::HEADER_LEN];
    recv.read_exact(&mut header).await.map_err(classify)?;
    let len = frame::payload_len(header, max)?;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await.map_err(classify)?;
    Ok(payload)
}

fn classify(error: ReadExactError) -> WireError {
    match error {
        ReadExactError::ReadError(ReadError::Reset(code)) => {
            WireError::Reset(u32::try_from(code.into_inner()).unwrap_or(u32::MAX))
        }
        ReadExactError::ReadError(ReadError::ConnectionLost(_)) => WireError::ConnectionLost,
        _ => WireError::Closed,
    }
}

/// Reads and decodes one frame within `timeout`.
pub(crate) async fn read_message<T: DeserializeOwned>(
    recv: &mut RecvStream,
    max: usize,
    timeout: Duration,
) -> Result<T, WireError> {
    let payload = tokio::time::timeout(timeout, read_payload(recv, max))
        .await
        .map_err(|_| WireError::Timeout)??;
    Ok(frame::decode_payload(&payload)?)
}

/// Encodes and writes one frame.
pub(crate) async fn write_message<T: Serialize>(
    send: &mut SendStream,
    message: &T,
    max: usize,
) -> Result<(), WireError> {
    let bytes = frame::encode(message, max)?;
    send.write_all(&bytes).await.map_err(|_| WireError::Closed)
}
