//! Receiving a transfer.
//!
//! Each file is requested on its own stream from a byte offset, written to
//! `<file_id>.part` in the transfer's partial folder while being hashed, and
//! only after its BLAKE3 hash matches is it flushed to disk and moved into
//! place. Nothing outside the partial folder is ever incomplete or unverified.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::{Connection, ReadError, ReadExactError, VarInt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tunnel_proto::control::TransferId;
use tunnel_proto::data::{DataHeader, Get, Refusal, TRAILER_LEN, reset};
use tunnel_proto::limits;
use tunnel_proto::manifest::EntryKind;

use super::{Counters, IssueLog, Run, reason};
use crate::pathsafe::{ConflictRule, Destination, PlaceError, Placed};
use crate::platform;
use crate::store::{IncomingEntry, Store};
use crate::wire::{self, WireError};

/// How long to wait for the sender's header after asking for a file.
const HEADER_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a stream may go without delivering any data.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Flush file data to disk at least this often.
const SYNC_EVERY: u64 = 64 * 1024 * 1024;
/// Times a file is retried after stalls before it is marked failed.
const MAX_STALLS: u32 = 3;
/// How often, and how far apart, removing a partial folder is attempted.
const REMOVE_ATTEMPTS: u32 = 40;
const REMOVE_PAUSE: Duration = Duration::from_millis(50);

/// Everything the workers of one incoming transfer share.
pub(crate) struct IncomingShared {
    pub id: TransferId,
    pub store: Store,
    pub dest: Destination,
    /// `<destination>/.tunnel-partial/<transfer id>`.
    pub partial: PathBuf,
    pub rule: ConflictRule,
    pub keep_modified: bool,
    pub keep_executable: bool,
    pub mark_downloaded: bool,
    pub counters: Counters,
    pub issues: IssueLog,
    pub disk: DiskWork,
}

/// Counts blocking disk jobs (hashing, writing, placing) that are still
/// running. A worker task can be dropped while its writer thread is still
/// flushing a partial file; the partial folder must not be removed until
/// those threads have closed their files, or the removal fails on Windows.
#[derive(Clone, Default)]
pub(crate) struct DiskWork(Arc<AtomicUsize>);

/// Held by a blocking disk job for as long as it runs.
pub(crate) struct DiskJob(Arc<AtomicUsize>);

impl DiskWork {
    fn begin(&self) -> DiskJob {
        self.0.fetch_add(1, Ordering::SeqCst);
        DiskJob(self.0.clone())
    }

    /// Waits until no disk job is running, or until `limit` has passed.
    pub(crate) async fn idle(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        while self.0.load(Ordering::SeqCst) > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}

impl Drop for DiskJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The live connection, replaced after a reconnect. `None` while disconnected.
pub(crate) type ConnWatch = watch::Receiver<Option<Connection>>;

/// How the download task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadEnd {
    /// Every file was dealt with (received, skipped or failed).
    Finished,
    /// The transfer was cancelled.
    Cancelled,
}

/// Something the session loop must act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notice {
    /// The disk filled up: pause and tell the person.
    DiskFull,
}

enum Outcome {
    /// Received, skipped or failed: nothing more to do for this file.
    Settled,
    /// Paused or cancelled part-way: try again when running.
    Interrupted(IncomingEntry),
    /// The disk is full: try again after the person frees space.
    DiskFull(IncomingEntry),
}

enum Fetch {
    Done,
    Skipped(&'static str),
    Failed(&'static str, Option<String>),
    /// The connection dropped; wait for the next one.
    ConnectionLost,
    /// Paused here or on the sender, or cancelled.
    Paused,
    /// The hash did not match; the partial file was deleted.
    HashMismatch,
    /// No data for too long.
    Stalled,
    DiskFull,
    /// The partial file was unusable and was discarded; ask again from zero.
    Restart,
}

impl Fetch {
    fn name(&self) -> &'static str {
        match self {
            Fetch::Done => "done",
            Fetch::Skipped(reason) => reason,
            Fetch::Failed(reason, _) => reason,
            Fetch::ConnectionLost => "connection_lost",
            Fetch::Paused => "paused",
            Fetch::HashMismatch => "hash_mismatch",
            Fetch::Stalled => "stalled",
            Fetch::DiskFull => "disk_full",
            Fetch::Restart => "restart",
        }
    }
}

/// Runs a whole incoming transfer: folders first, then files with bounded
/// parallelism (4 at once for files of 8 MiB or more, 32 for smaller ones).
pub(crate) async fn run_download(
    shared: Arc<IncomingShared>,
    conn: ConnWatch,
    mut run: watch::Receiver<Run>,
    notices: mpsc::Sender<(TransferId, Notice)>,
) -> DownloadEnd {
    if create_folders(&shared).await.is_err() {
        // Storage failed while listing folders; files will report their own errors.
    }

    let large = Arc::new(Semaphore::new(limits::MAX_PARALLEL_LARGE));
    let small = Arc::new(Semaphore::new(limits::MAX_PARALLEL_SMALL));
    let mut workers: JoinSet<Outcome> = JoinSet::new();
    let mut retry: Vec<IncomingEntry> = Vec::new();
    let mut after: i64 = -1;
    let mut listed_all = false;

    loop {
        let batch: Vec<IncomingEntry> = if !listed_all {
            let store = shared.store.clone();
            let id = shared.id;
            let batch = tokio::task::spawn_blocking(move || store.incoming_batch(id, after, 256))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            match batch.last() {
                Some(last) => after = i64::from(last.file_id),
                None => listed_all = true,
            }
            batch
        } else if !retry.is_empty() {
            // Files interrupted by a pause or a full disk get another turn.
            tokio::time::sleep(Duration::from_millis(250)).await;
            std::mem::take(&mut retry)
        } else if workers.is_empty() {
            return DownloadEnd::Finished;
        } else {
            Vec::new()
        };

        for entry in batch
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::File)
        {
            if !wait_until_running(&mut run).await {
                stop_workers(&mut workers).await;
                return DownloadEnd::Cancelled;
            }
            let semaphore = if entry.size >= limits::LARGE_FILE_THRESHOLD {
                &large
            } else {
                &small
            };
            let permit = loop {
                tokio::select! {
                    permit = semaphore.clone().acquire_owned() => break permit.ok(),
                    Some(done) = workers.join_next(), if !workers.is_empty() => {
                        settle(done.ok(), &shared.id, &mut retry, &notices).await;
                    }
                }
            };
            let Some(permit) = permit else {
                return DownloadEnd::Cancelled;
            };
            workers.spawn(worker(
                shared.clone(),
                conn.clone(),
                run.clone(),
                entry,
                permit,
            ));
        }

        if listed_all && !workers.is_empty() {
            // Everything is dispatched: wait for workers, watching for cancel.
            tokio::select! {
                done = workers.join_next(), if !workers.is_empty() => {
                    if let Some(done) = done {
                        settle(done.ok(), &shared.id, &mut retry, &notices).await;
                    }
                }
                changed = run.changed() => {
                    if changed.is_err() || *run.borrow() == Run::Cancelled {
                        stop_workers(&mut workers).await;
                        return DownloadEnd::Cancelled;
                    }
                }
            }
        }
    }
}

/// Ends the workers of a cancelled transfer. They watch the run state and stop
/// on their own, closing their partial files first; one that is stuck waiting
/// on the network is aborted after a short grace period.
async fn stop_workers(workers: &mut JoinSet<Outcome>) {
    let drained = tokio::time::timeout(Duration::from_secs(5), async {
        while workers.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        workers.shutdown().await;
    }
}

async fn settle(
    outcome: Option<Outcome>,
    id: &TransferId,
    retry: &mut Vec<IncomingEntry>,
    notices: &mpsc::Sender<(TransferId, Notice)>,
) {
    match outcome {
        Some(Outcome::Settled) | None => {}
        Some(Outcome::Interrupted(entry)) => retry.push(entry),
        Some(Outcome::DiskFull(entry)) => {
            retry.push(entry);
            let _ = notices.send((*id, Notice::DiskFull)).await;
        }
    }
}

/// Waits while paused. Returns `false` if the transfer was cancelled.
async fn wait_until_running(run: &mut watch::Receiver<Run>) -> bool {
    loop {
        match *run.borrow_and_update() {
            Run::Running => return true,
            Run::Cancelled => return false,
            Run::Paused => {}
        }
        if run.changed().await.is_err() {
            return false;
        }
    }
}

/// Creates every folder of the transfer, so empty folders exist and files find
/// their place.
async fn create_folders(shared: &Arc<IncomingShared>) -> crate::error::Result<()> {
    let mut after: i64 = -1;
    loop {
        let shared = shared.clone();
        let last = tokio::task::spawn_blocking(move || -> crate::error::Result<Option<i64>> {
            let batch = shared.store.incoming_batch(shared.id, after, 512)?;
            for entry in batch.iter().filter(|entry| entry.kind == EntryKind::Folder) {
                let components: Vec<&str> = entry.rel.split('/').collect();
                if let Err(err) = shared.dest.ensure_folder(&components) {
                    let (why, detail) = place_reason(&err);
                    shared.issues.failed(&entry.rel, why, detail);
                }
            }
            Ok(batch.last().map(|entry| i64::from(entry.file_id)))
        })
        .await
        .map_err(crate::error::Error::internal)??;
        match last {
            Some(last) => after = last,
            None => return Ok(()),
        }
    }
}

fn place_reason(err: &PlaceError) -> (&'static str, Option<String>) {
    match err {
        PlaceError::Link => (reason::DESTINATION_LINK, None),
        PlaceError::NotAFolder => (reason::DESTINATION_FILE, None),
        PlaceError::TooLong => (reason::PATH_TOO_LONG, None),
        PlaceError::UnsafeName | PlaceError::Escapes => (reason::PROTOCOL, None),
        PlaceError::NoFreeName => (reason::WRITE_FAILED, None),
        PlaceError::Io(io) => (reason::WRITE_FAILED, Some(io.to_string())),
    }
}

fn is_disk_full(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
    )
}

async fn worker(
    shared: Arc<IncomingShared>,
    mut conn: ConnWatch,
    mut run: watch::Receiver<Run>,
    entry: IncomingEntry,
    _permit: OwnedSemaphorePermit,
) -> Outcome {
    let mut hash_failures = 0u32;
    let mut stalls = 0u32;
    loop {
        if *run.borrow() != Run::Running {
            return Outcome::Interrupted(entry);
        }
        // Wait for a live connection (there is none while reconnecting).
        let connection = loop {
            if let Some(connection) = conn.borrow_and_update().clone() {
                break connection;
            }
            tokio::select! {
                changed = conn.changed() => if changed.is_err() { return Outcome::Interrupted(entry) },
                changed = run.changed() => {
                    if changed.is_err() || *run.borrow() != Run::Running {
                        return Outcome::Interrupted(entry);
                    }
                }
            }
        };

        let settle = |kind: &Fetch| match kind {
            Fetch::Done => {
                shared.counters.files.fetch_add(1, Ordering::Relaxed);
            }
            Fetch::Skipped(why) => {
                drop_file(&shared, &entry);
                shared.counters.skipped.fetch_add(1, Ordering::Relaxed);
                shared.issues.skipped(&entry.rel, why);
            }
            Fetch::Failed(why, detail) => {
                drop_file(&shared, &entry);
                shared.counters.failed.fetch_add(1, Ordering::Relaxed);
                shared.issues.failed(&entry.rel, why, detail.clone());
            }
            _ => {}
        };

        let fetched = fetch(&shared, &connection, &entry, &mut run).await;
        tracing::debug!(
            file = entry.file_id,
            outcome = fetched.name(),
            "fetch attempt"
        );
        match fetched {
            done @ (Fetch::Done | Fetch::Skipped(_) | Fetch::Failed(..)) => {
                settle(&done);
                return Outcome::Settled;
            }
            Fetch::Paused => return Outcome::Interrupted(entry),
            Fetch::DiskFull => return Outcome::DiskFull(entry),
            Fetch::Restart => continue,
            Fetch::HashMismatch => {
                hash_failures += 1;
                if hash_failures >= 2 {
                    settle(&Fetch::Failed(reason::HASH_MISMATCH, None));
                    return Outcome::Settled;
                }
            }
            Fetch::Stalled => {
                stalls += 1;
                if stalls >= MAX_STALLS {
                    settle(&Fetch::Failed(reason::TIMEOUT, None));
                    return Outcome::Settled;
                }
            }
            Fetch::ConnectionLost => {
                // Wait for the session to reconnect (or end).
                loop {
                    let gone = conn
                        .borrow()
                        .as_ref()
                        .is_none_or(|c| c.stable_id() == connection.stable_id());
                    if !gone {
                        break;
                    }
                    tokio::select! {
                        changed = conn.changed() => if changed.is_err() { return Outcome::Interrupted(entry) },
                        changed = run.changed() => {
                            if changed.is_err() || *run.borrow() == Run::Cancelled {
                                return Outcome::Interrupted(entry);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A file will not arrive: throw away any partial data and count its size as
/// dealt with, so progress still reaches the end.
fn drop_file(shared: &IncomingShared, entry: &IncomingEntry) {
    let part = part_path(shared, entry);
    if let Ok(metadata) = std::fs::metadata(&part) {
        discard_part(shared, &part, metadata.len());
    }
    shared
        .counters
        .dropped
        .fetch_add(entry.size, Ordering::Relaxed);
}

fn uncount(shared: &IncomingShared, bytes: u64) {
    shared
        .counters
        .bytes
        .fetch_sub(bytes.min(shared.counters.bytes()), Ordering::Relaxed);
}

fn part_path(shared: &IncomingShared, entry: &IncomingEntry) -> PathBuf {
    shared.partial.join(format!("{}.part", entry.file_id))
}

/// Re-hashes whatever is already in the partial file to rebuild the hash state.
/// Returns the verified length, or an error that says how many bytes of an
/// unusable partial file were thrown away.
fn prepare_part(part: &Path, size: u64) -> std::io::Result<(u64, blake3::Hasher, u64)> {
    let mut hasher = blake3::Hasher::new();
    let existing = match std::fs::metadata(part) {
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((0, hasher, 0)),
        Err(err) => return Err(err),
    };
    if existing == 0 {
        return Ok((0, hasher, 0));
    }
    if existing > size {
        std::fs::remove_file(part)?;
        return Ok((0, hasher, existing));
    }
    let mut file = std::fs::File::open(part)?;
    let mut buffer = vec![0u8; limits::IO_BUFFER];
    let mut hashed = 0u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                hashed += read as u64;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok((hashed, hasher, 0))
}

/// Appends chunks to the partial file while hashing them. Blocking.
fn write_part(
    part: &Path,
    offset: u64,
    mut hasher: blake3::Hasher,
    mut chunks: mpsc::Receiver<Bytes>,
) -> std::io::Result<blake3::Hash> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(part)?;
    file.set_len(offset)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut since_sync = 0u64;
    while let Some(chunk) = chunks.blocking_recv() {
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        since_sync += chunk.len() as u64;
        if since_sync >= SYNC_EVERY {
            file.sync_data()?;
            since_sync = 0;
        }
    }
    // Flush to disk before the file can be moved into place.
    file.sync_data()?;
    Ok(hasher.finalize())
}

fn discard_part(shared: &IncomingShared, part: &Path, counted: u64) {
    let _ = std::fs::remove_file(part);
    uncount(shared, counted);
}

/// One attempt at one file.
async fn fetch(
    shared: &Arc<IncomingShared>,
    connection: &Connection,
    entry: &IncomingEntry,
    run: &mut watch::Receiver<Run>,
) -> Fetch {
    let part = part_path(shared, entry);
    let lost_or = |otherwise: Fetch| {
        if connection.close_reason().is_some() {
            Fetch::ConnectionLost
        } else {
            otherwise
        }
    };

    // Resume: trust only what is actually in the partial file, re-hashed.
    let prepared = {
        let part = part.clone();
        let size = entry.size;
        let job = shared.disk.begin();
        tokio::task::spawn_blocking(move || {
            let _job = job;
            prepare_part(&part, size)
        })
        .await
    };
    let (offset, hasher) = match prepared {
        Ok(Ok((offset, hasher, discarded))) => {
            uncount(shared, discarded);
            (offset, hasher)
        }
        Ok(Err(err)) => return Fetch::Failed(reason::WRITE_FAILED, Some(err.to_string())),
        Err(_) => return Fetch::Failed(reason::WRITE_FAILED, None),
    };

    let (mut send, mut recv) =
        match tokio::time::timeout(HEADER_TIMEOUT, connection.open_bi()).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(_)) => return Fetch::ConnectionLost,
            Err(_) => return lost_or(Fetch::Stalled),
        };
    let get = Get {
        transfer_id: shared.id,
        file_id: entry.file_id,
        offset,
    };
    if wire::write_message(&mut send, &get, limits::MAX_DATA_FRAME)
        .await
        .is_err()
        || send.finish().is_err()
    {
        return lost_or(Fetch::Stalled);
    }

    let header: DataHeader =
        match wire::read_message(&mut recv, limits::MAX_DATA_FRAME, HEADER_TIMEOUT).await {
            Ok(header) => header,
            Err(WireError::Timeout) => return lost_or(Fetch::Stalled),
            Err(WireError::ConnectionLost) => return Fetch::ConnectionLost,
            // A reset can overtake the header the sender wrote just before it.
            Err(WireError::Reset(code)) => return reset_outcome(code),
            Err(WireError::Closed) => return lost_or(Fetch::Failed(reason::REFUSED, None)),
            Err(WireError::Frame(_)) => return Fetch::Failed(reason::PROTOCOL, None),
        };
    match header {
        DataHeader::Ok {
            size,
            offset: echoed,
        } if size == entry.size && echoed == offset => {}
        // The sender's idea of the file differs from its own manifest.
        DataHeader::Ok { .. } => {
            let _ = recv.stop(VarInt::from_u32(reset::PROTOCOL));
            return Fetch::Failed(reason::PROTOCOL, None);
        }
        DataHeader::Refused(Refusal::Paused) => return Fetch::Paused,
        DataHeader::Refused(Refusal::Unreadable) => {
            return Fetch::Skipped(reason::UNREADABLE_ON_SENDER);
        }
        DataHeader::Refused(Refusal::Changed) => return Fetch::Failed(reason::CHANGED, None),
        DataHeader::Refused(Refusal::BadOffset) if offset > 0 => {
            discard_part(shared, &part, offset);
            return Fetch::Restart;
        }
        DataHeader::Refused(_) => return Fetch::Failed(reason::REFUSED, None),
    }

    // Stream the body to a writer thread: disk work stays off the network task,
    // and the bounded channel stops a slow disk from buffering unbounded data.
    let (chunks, queue) = mpsc::channel::<Bytes>(8);
    let writer = {
        let part = part.clone();
        let job = shared.disk.begin();
        tokio::task::spawn_blocking(move || {
            let _job = job;
            write_part(&part, offset, hasher, queue)
        })
    };
    let mut remaining = entry.size - offset;
    let mut interrupted: Option<Fetch> = None;
    while remaining > 0 {
        let want = remaining.min(limits::IO_BUFFER as u64) as usize;
        let read = tokio::select! {
            read = tokio::time::timeout(STALL_TIMEOUT, recv.read_chunk(want)) => read,
            changed = run.changed() => {
                if changed.is_err() || *run.borrow() != Run::Running {
                    let _ = recv.stop(VarInt::from_u32(reset::PAUSED));
                    interrupted = Some(Fetch::Paused);
                    break;
                }
                continue;
            }
        };
        match read {
            Ok(Ok(Some(chunk))) => {
                let len = chunk.len() as u64;
                if len > remaining {
                    interrupted = Some(Fetch::Failed(reason::PROTOCOL, None));
                    break;
                }
                remaining -= len;
                shared.counters.bytes.fetch_add(len, Ordering::Relaxed);
                if chunks.send(chunk).await.is_err() {
                    // The writer stopped: its error is collected below.
                    break;
                }
            }
            // The stream ended before the promised number of bytes.
            Ok(Ok(None)) => {
                interrupted = Some(Fetch::Failed(reason::CHANGED, None));
                break;
            }
            Ok(Err(err)) => {
                interrupted = Some(read_failure(&err));
                break;
            }
            Err(_) => {
                let _ = recv.stop(VarInt::from_u32(reset::PROTOCOL));
                interrupted = Some(lost_or(Fetch::Stalled));
                break;
            }
        }
    }
    drop(chunks);
    let written = writer.await;
    let local_hash = match written {
        Ok(Ok(hash)) => hash,
        Ok(Err(err)) => {
            let _ = recv.stop(VarInt::from_u32(reset::WRITE_FAILED));
            return if is_disk_full(&err) {
                Fetch::DiskFull
            } else {
                Fetch::Failed(reason::WRITE_FAILED, Some(err.to_string()))
            };
        }
        Err(_) => return Fetch::Failed(reason::WRITE_FAILED, None),
    };
    if let Some(interrupted) = interrupted {
        // A protocol violation or a changed file poisons the partial data.
        if matches!(interrupted, Fetch::Failed(..)) {
            discard_part(shared, &part, entry.size - remaining);
        }
        return interrupted;
    }

    // The trailer: BLAKE3 of the whole file. Then the stream must end.
    let mut trailer = [0u8; TRAILER_LEN];
    match tokio::time::timeout(STALL_TIMEOUT, recv.read_exact(&mut trailer)).await {
        Ok(Ok(())) => {}
        Ok(Err(ReadExactError::ReadError(err))) => {
            let failure = read_failure(&err);
            if matches!(failure, Fetch::Failed(..)) {
                discard_part(shared, &part, entry.size);
            }
            return failure;
        }
        Ok(Err(ReadExactError::FinishedEarly(_))) => {
            discard_part(shared, &part, entry.size);
            return Fetch::Failed(reason::PROTOCOL, None);
        }
        Err(_) => return lost_or(Fetch::Stalled),
    }
    let mut extra = [0u8; 1];
    if let Ok(Ok(Some(_))) =
        tokio::time::timeout(Duration::from_secs(10), recv.read(&mut extra)).await
    {
        // Bytes past the end of the file: refuse the file.
        let _ = recv.stop(VarInt::from_u32(reset::PROTOCOL));
        discard_part(shared, &part, entry.size);
        return Fetch::Failed(reason::PROTOCOL, None);
    }
    if local_hash.as_bytes() != &trailer {
        discard_part(shared, &part, entry.size);
        return Fetch::HashMismatch;
    }

    let placed = {
        let shared = shared.clone();
        let entry = entry.clone();
        let job = shared.disk.begin();
        tokio::task::spawn_blocking(move || {
            let _job = job;
            place(&shared, &entry, &part)
        })
        .await
    };
    match placed {
        Ok(fetch) => fetch,
        Err(_) => Fetch::Failed(reason::WRITE_FAILED, None),
    }
}

/// What a stream reset from the sender means for the file.
fn reset_outcome(code: u32) -> Fetch {
    match code {
        reset::CHANGED => Fetch::Failed(reason::CHANGED, None),
        reset::UNREADABLE => Fetch::Skipped(reason::UNREADABLE_ON_SENDER),
        // Paused or cancelled on the sender: the control message that says so
        // is on its way; hold the partial data until it arrives.
        reset::PAUSED | reset::CANCELLED => Fetch::Paused,
        _ => Fetch::Failed(reason::PROTOCOL, None),
    }
}

fn read_failure(err: &ReadError) -> Fetch {
    match err {
        ReadError::Reset(code) => {
            reset_outcome(u32::try_from(code.into_inner()).unwrap_or(u32::MAX))
        }
        ReadError::ConnectionLost(_) => Fetch::ConnectionLost,
        _ => Fetch::Failed(reason::PROTOCOL, None),
    }
}

/// Moves a verified file into place and restores its metadata. Blocking.
fn place(shared: &IncomingShared, entry: &IncomingEntry, part: &Path) -> Fetch {
    let components: Vec<&str> = entry.rel.split('/').collect();
    let Some((name, parents)) = components.split_last() else {
        return Fetch::Failed(reason::PROTOCOL, None);
    };
    let folder = match shared.dest.ensure_folder(parents) {
        Ok(folder) => folder,
        Err(err) => {
            let _ = std::fs::remove_file(part);
            uncount(shared, entry.size);
            let (why, detail) = place_reason(&err);
            return if matches!(&err, PlaceError::Io(io) if is_disk_full(io)) {
                Fetch::DiskFull
            } else {
                Fetch::Failed(why, detail)
            };
        }
    };
    match shared.dest.place(part, &folder, name, shared.rule) {
        Ok(Placed::Saved { path, numbered }) => {
            if numbered {
                let saved_as = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                shared
                    .issues
                    .push(&entry.rel, "renamed", reason::KEPT_BOTH, Some(saved_as));
            }
            if shared.keep_executable && entry.executable {
                platform::set_executable(&path);
            }
            if shared.mark_downloaded {
                platform::mark_downloaded(&path);
            }
            if shared.keep_modified
                && let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path)
            {
                let _ = file.set_modified(platform::system_time_from_ms(entry.modified_ms));
            }
            Fetch::Done
        }
        Ok(Placed::Skipped) => {
            uncount(shared, entry.size);
            Fetch::Skipped(reason::EXISTS)
        }
        Err(err) => {
            let _ = std::fs::remove_file(part);
            uncount(shared, entry.size);
            let (why, detail) = place_reason(&err);
            Fetch::Failed(why, detail)
        }
    }
}

/// Lists partial files with their sizes, for the `Resume` message.
pub(crate) fn partial_offsets(partial: &Path, limit: usize) -> Vec<(u32, u64)> {
    let Ok(entries) = std::fs::read_dir(partial) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let file_id: u32 = name.to_str()?.strip_suffix(".part")?.parse().ok()?;
            let len = entry.metadata().ok()?.len();
            (len > 0).then_some((file_id, len))
        })
        .take(limit)
        .collect()
}

/// Removes a transfer's partial folder, and the `.tunnel-partial` folder above
/// it when that is empty. Blocking.
pub(crate) fn remove_partial(partial: &Path) {
    use std::io::ErrorKind::NotFound;
    // On Windows a file that was closed a moment ago, or that a virus scanner
    // is looking at, can refuse to go on the first attempt. Try for a while.
    for attempt in 0..REMOVE_ATTEMPTS {
        match std::fs::remove_dir_all(partial) {
            Ok(()) => break,
            Err(err) if err.kind() == NotFound => break,
            Err(err) if attempt + 1 == REMOVE_ATTEMPTS => {
                tracing::warn!(error = %err, "could not remove a partial folder");
            }
            Err(_) => std::thread::sleep(REMOVE_PAUSE),
        }
    }
    let Some(parent) = partial.parent() else {
        return;
    };
    for _ in 0..REMOVE_ATTEMPTS {
        match std::fs::remove_dir(parent) {
            Ok(()) => break,
            Err(err) if err.kind() == NotFound => break,
            Err(_) => {
                // Another transfer keeps its own partial folder here: leave it.
                let own = partial.file_name();
                let shared_with_others = std::fs::read_dir(parent)
                    .map(|entries| {
                        entries
                            .flatten()
                            .any(|entry| Some(entry.file_name().as_os_str()) != own)
                    })
                    .unwrap_or(true);
                if shared_with_others {
                    break;
                }
                std::thread::sleep(REMOVE_PAUSE);
            }
        }
    }
}
