//! Serving file data to the receiver.
//!
//! For each file the receiver opens a stream and sends `Get`. This side answers
//! only for files of a transfer the receiver accepted, streams the bytes from
//! the requested offset, and ends with the BLAKE3 hash of the whole file.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream, VarInt};
use tokio::sync::{Semaphore, mpsc};
use tunnel_proto::control::TransferId;
use tunnel_proto::data::{DataHeader, Get, Refusal, reset};
use tunnel_proto::limits;
use tunnel_proto::manifest::EntryKind;

use super::{IssueLog, reason};
use crate::platform;
use crate::store::{OutgoingEntry, Store};
use crate::wire;

/// How long to wait for the request on a newly opened stream.
const GET_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one chunk may take to be accepted by the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(300);
/// File streams served at once.
const MAX_SERVING: usize = 64;
/// How long a request may wait for its transfer's `Accept` to be processed.
///
/// The receiver sends `Accept` on the control stream and may open file streams
/// straight away. Streams are independent, so a request can overtake the
/// `Accept` it depends on.
const ACCEPT_GRACE: Duration = Duration::from_secs(3);

/// One outgoing transfer, as the stream handlers see it.
#[derive(Debug)]
pub(crate) struct OutgoingShared {
    /// The receiver accepted.
    pub accepted: AtomicBool,
    /// Paused on this device.
    pub paused: AtomicBool,
    /// Cancelled or finished: nothing more is served.
    pub closed: AtomicBool,
    /// Bytes handed to the connection.
    pub bytes_sent: AtomicU64,
    /// Files sent to the end, hash included.
    pub files_sent: AtomicU64,
    /// Files that could not be read or changed while sending.
    pub issues: IssueLog,
}

/// Registry of outgoing transfers plus the stream handlers.
pub(crate) struct SenderHub {
    store: Store,
    transfers: Mutex<HashMap<TransferId, Arc<OutgoingShared>>>,
    permits: Arc<Semaphore>,
}

enum ReadFailure {
    Changed,
    Unreadable,
    /// The stream handler went away; nothing to report.
    Abandoned,
}

impl SenderHub {
    pub(crate) fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            store,
            transfers: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(MAX_SERVING)),
        })
    }

    pub(crate) fn register(&self, transfer: TransferId) -> Arc<OutgoingShared> {
        let shared = Arc::new(OutgoingShared {
            accepted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            bytes_sent: AtomicU64::new(0),
            files_sent: AtomicU64::new(0),
            issues: IssueLog::default(),
        });
        self.transfers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(transfer, shared.clone());
        shared
    }

    pub(crate) fn remove(&self, transfer: TransferId) {
        if let Some(shared) = self
            .transfers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&transfer)
        {
            shared.closed.store(true, Ordering::Relaxed);
        }
    }

    fn get(&self, transfer: TransferId) -> Option<Arc<OutgoingShared>> {
        self.transfers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&transfer)
            .cloned()
    }

    /// Handles one incoming stream. Never trusts the request.
    pub(crate) async fn serve(self: Arc<Self>, mut send: SendStream, mut recv: RecvStream) {
        let Ok(_permit) = self.permits.clone().acquire_owned().await else {
            return;
        };
        let get: Get =
            match wire::read_message(&mut recv, limits::MAX_DATA_FRAME, GET_TIMEOUT).await {
                Ok(get) => get,
                Err(_) => {
                    let _ = send.reset(VarInt::from_u32(reset::PROTOCOL));
                    return;
                }
            };
        tracing::debug!(file = get.file_id, offset = get.offset, "file requested");
        let refuse = |refusal: Refusal| {
            tracing::debug!(file = get.file_id, ?refusal, "request refused");
            DataHeader::Refused(refusal)
        };

        // Only transfers the receiver accepted, and only while they are live.
        let Some(shared) = self.get(get.transfer_id) else {
            return finish_with(&mut send, refuse(Refusal::NotAccepted)).await;
        };
        let waited = tokio::time::Instant::now();
        while !shared.accepted.load(Ordering::Relaxed)
            && !shared.closed.load(Ordering::Relaxed)
            && waited.elapsed() < ACCEPT_GRACE
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if !shared.accepted.load(Ordering::Relaxed) || shared.closed.load(Ordering::Relaxed) {
            return finish_with(&mut send, refuse(Refusal::NotAccepted)).await;
        }
        if shared.paused.load(Ordering::Relaxed) {
            return finish_with(&mut send, refuse(Refusal::Paused)).await;
        }

        let store = self.store.clone();
        let opened = tokio::task::spawn_blocking(move || open_source(&store, get)).await;
        let (file, entry) = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err((refusal, entry))) => {
                if let Some(entry) = entry {
                    match refusal {
                        Refusal::Unreadable => {
                            shared.issues.skipped(&entry.rel, reason::UNREADABLE)
                        }
                        Refusal::Changed => shared.issues.failed(&entry.rel, reason::CHANGED, None),
                        _ => {}
                    }
                }
                return finish_with(&mut send, refuse(refusal)).await;
            }
            Err(_) => return finish_with(&mut send, refuse(Refusal::Unreadable)).await,
        };

        let header = DataHeader::Ok {
            size: entry.size,
            offset: get.offset,
        };
        if wire::write_message(&mut send, &header, limits::MAX_DATA_FRAME)
            .await
            .is_err()
        {
            return;
        }

        let (chunks, mut ready) = mpsc::channel::<Bytes>(4);
        let rel = entry.rel.clone();
        let reader =
            tokio::task::spawn_blocking(move || read_and_hash(file, &entry, get.offset, &chunks));

        while let Some(chunk) = ready.recv().await {
            if shared.closed.load(Ordering::Relaxed) {
                let _ = send.reset(VarInt::from_u32(reset::CANCELLED));
                return;
            }
            if shared.paused.load(Ordering::Relaxed) {
                let _ = send.reset(VarInt::from_u32(reset::PAUSED));
                return;
            }
            let len = chunk.len() as u64;
            match tokio::time::timeout(WRITE_TIMEOUT, send.write_chunk(chunk)).await {
                Ok(Ok(())) => {
                    shared.bytes_sent.fetch_add(len, Ordering::Relaxed);
                }
                // The receiver stopped the stream or the connection dropped.
                Ok(Err(_)) | Err(_) => {
                    let _ = send.reset(VarInt::from_u32(reset::CANCELLED));
                    return;
                }
            }
        }

        match reader.await {
            Ok(Ok(hash)) => {
                if send.write_all(&hash).await.is_ok() && send.finish().is_ok() {
                    shared.files_sent.fetch_add(1, Ordering::Relaxed);
                    // Wait until the receiver has everything (or gives up).
                    let _ = tokio::time::timeout(WRITE_TIMEOUT, send.stopped()).await;
                }
            }
            Ok(Err(ReadFailure::Changed)) => {
                shared.issues.failed(&rel, reason::CHANGED, None);
                let _ = send.reset(VarInt::from_u32(reset::CHANGED));
            }
            Ok(Err(ReadFailure::Unreadable)) => {
                shared.issues.skipped(&rel, reason::UNREADABLE);
                let _ = send.reset(VarInt::from_u32(reset::UNREADABLE));
            }
            Ok(Err(ReadFailure::Abandoned)) | Err(_) => {
                let _ = send.reset(VarInt::from_u32(reset::CANCELLED));
            }
        }
    }
}

async fn finish_with(send: &mut SendStream, header: DataHeader) {
    if wire::write_message(send, &header, limits::MAX_DATA_FRAME)
        .await
        .is_ok()
    {
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(10), send.stopped()).await;
    }
}

type Refused = (Refusal, Option<OutgoingEntry>);

/// Looks the file up and opens it, refusing if it is not exactly the file that
/// was offered (same size, same modified time, still a regular file).
fn open_source(store: &Store, get: Get) -> Result<(std::fs::File, OutgoingEntry), Refused> {
    let entry = match store.outgoing_entry(get.transfer_id, get.file_id) {
        Ok(Some(entry)) if entry.kind == EntryKind::File => entry,
        _ => return Err((Refusal::UnknownFile, None)),
    };
    if get.offset > entry.size {
        return Err((Refusal::BadOffset, Some(entry)));
    }
    let metadata = match std::fs::symlink_metadata(&entry.src) {
        Ok(metadata) => metadata,
        Err(_) => return Err((Refusal::Unreadable, Some(entry))),
    };
    if !metadata.is_file() || platform::is_link(&metadata) || !unchanged(&metadata, &entry) {
        return Err((Refusal::Changed, Some(entry)));
    }
    match std::fs::File::open(&entry.src) {
        Ok(file) => Ok((file, entry)),
        Err(_) => Err((Refusal::Unreadable, Some(entry))),
    }
}

fn unchanged(metadata: &std::fs::Metadata, entry: &OutgoingEntry) -> bool {
    metadata.len() == entry.size && platform::modified_ms(metadata) == entry.modified_ms
}

/// Reads the whole file in 1 MiB buffers, hashing everything and handing over
/// the part from `offset` on. The skipped part is read only to hash it.
fn read_and_hash(
    mut file: std::fs::File,
    entry: &OutgoingEntry,
    offset: u64,
    chunks: &mpsc::Sender<Bytes>,
) -> Result<[u8; 32], ReadFailure> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; limits::IO_BUFFER];
    let mut position = 0u64;
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ReadFailure::Unreadable),
        };
        let end = position + read as u64;
        if end > entry.size {
            return Err(ReadFailure::Changed);
        }
        hasher.update(&buffer[..read]);
        if end > offset {
            let start = offset.saturating_sub(position) as usize;
            let chunk = Bytes::copy_from_slice(&buffer[start..read]);
            chunks
                .blocking_send(chunk)
                .map_err(|_| ReadFailure::Abandoned)?;
        }
        position = end;
    }
    let still_same = file
        .metadata()
        .map(|metadata| unchanged(&metadata, entry))
        .unwrap_or(false);
    if position != entry.size || !still_same {
        return Err(ReadFailure::Changed);
    }
    Ok(*hasher.finalize().as_bytes())
}
