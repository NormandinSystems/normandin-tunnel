//! Local storage in SQLite.
//!
//! Two databases live in the app data folder:
//!
//! * `tunnel.db` — settings and transfer history. Small, durable.
//! * `scratch.db` — the manifests of the current session's transfers. A
//!   manifest can hold a million entries, so it is kept on disk rather than in
//!   memory. It is disposable and recreated at every start.
//!
//! Neither database holds a secret. The databases are not encrypted; they rely
//! on the operating-system account and disk encryption.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use tunnel_proto::control::TransferId;
use tunnel_proto::manifest::{EntryKind, ManifestEntry};

use crate::error::{Error, Result};
use crate::pathsafe::{self, Unsafe};
use crate::platform;
use crate::settings::{HistoryRetention, Settings};

const MAIN_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS history (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    at        INTEGER NOT NULL,
    peer      TEXT NOT NULL,
    direction TEXT NOT NULL,
    files     INTEGER NOT NULL,
    folders   INTEGER NOT NULL,
    bytes     INTEGER NOT NULL,
    result    TEXT NOT NULL,
    names     TEXT NOT NULL,
    folder    TEXT
);
CREATE INDEX IF NOT EXISTS history_at ON history(at);
";

const SCRATCH_SCHEMA: &str = "
CREATE TABLE incoming (
    transfer     BLOB NOT NULL,
    file_id      INTEGER NOT NULL,
    kind         INTEGER NOT NULL,
    rel          TEXT NOT NULL,
    orig         TEXT,
    fold         TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    exec         INTEGER NOT NULL,
    renamed_from TEXT,
    PRIMARY KEY (transfer, file_id)
) WITHOUT ROWID;
CREATE UNIQUE INDEX incoming_fold ON incoming(transfer, fold);
CREATE INDEX incoming_orig ON incoming(transfer, orig) WHERE orig IS NOT NULL;
CREATE TABLE outgoing (
    transfer BLOB NOT NULL,
    file_id  INTEGER NOT NULL,
    kind     INTEGER NOT NULL,
    rel      TEXT NOT NULL,
    src      TEXT NOT NULL,
    size     INTEGER NOT NULL,
    mtime    INTEGER NOT NULL,
    exec     INTEGER NOT NULL,
    PRIMARY KEY (transfer, file_id)
) WITHOUT ROWID;
";

/// One line of transfer history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryEntry {
    /// Row ID.
    pub id: i64,
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The other device's name.
    pub peer: String,
    /// `send` or `receive`.
    pub direction: String,
    /// Files transferred.
    pub files: u64,
    /// Folders in the transfer.
    pub folders: u64,
    /// Bytes in the transfer.
    pub bytes: u64,
    /// `done`, `partial`, `cancelled`, `declined` or `failed`.
    pub result: String,
    /// The first few names, when "show file names in history" is on.
    pub names: Vec<String>,
    /// Where the files were saved (received transfers).
    pub folder: Option<String>,
}

/// Why a manifest page was refused. Any of these rejects the whole offer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// A path broke the safety rules.
    #[error("unsafe path: {0}")]
    Unsafe(#[from] Unsafe),
    /// File IDs must count up from 0 with no gaps.
    #[error("file IDs out of order")]
    OutOfOrder,
    /// A file or folder was listed before the folder that contains it.
    #[error("entry listed before its folder")]
    MissingParent,
    /// The same folder was listed twice.
    #[error("folder listed twice")]
    DuplicateFolder,
    /// Too many names collided.
    #[error("too many colliding names")]
    Collisions,
    /// The database failed.
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<rusqlite::Error> for ManifestError {
    fn from(err: rusqlite::Error) -> Self {
        ManifestError::Storage(err.to_string())
    }
}

/// What one accepted manifest page contained, as the receiver computed it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageStats {
    /// Files in the page.
    pub files: u64,
    /// Folders in the page.
    pub folders: u64,
    /// Sum of file sizes.
    pub bytes: u64,
    /// Entries whose name had to change.
    pub renamed: u64,
    /// Entries whose name contained bidirectional-text controls.
    pub bidi: u64,
    /// Programs and scripts.
    pub executables: u64,
    /// Programs hidden behind a second extension.
    pub double_extensions: u64,
}

impl PageStats {
    /// Adds another page's numbers.
    pub fn add(&mut self, other: &PageStats) {
        self.files += other.files;
        self.folders += other.folders;
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.renamed += other.renamed;
        self.bidi += other.bidi;
        self.executables += other.executables;
        self.double_extensions += other.double_extensions;
    }
}

/// A validated entry of an incoming transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingEntry {
    /// Position in the manifest.
    pub file_id: u32,
    /// File or folder.
    pub kind: EntryKind,
    /// Safe path relative to the destination, components joined with `/`.
    pub rel: String,
    /// Size in bytes.
    pub size: u64,
    /// Modified time (ms since the Unix epoch).
    pub modified_ms: i64,
    /// The sender flagged the file executable.
    pub executable: bool,
}

/// An entry of an outgoing transfer, with where it lives on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingEntry {
    /// Position in the manifest.
    pub file_id: u32,
    /// File or folder.
    pub kind: EntryKind,
    /// Path on the wire: name components joined with `/`.
    pub rel: String,
    /// Absolute path on this device.
    pub src: PathBuf,
    /// Size recorded when the manifest was built.
    pub size: u64,
    /// Modified time recorded when the manifest was built.
    pub modified_ms: i64,
    /// Executable flag.
    pub executable: bool,
}

impl OutgoingEntry {
    /// The wire form of this entry.
    pub fn to_manifest(&self) -> ManifestEntry {
        ManifestEntry {
            file_id: self.file_id,
            kind: self.kind,
            path: self.rel.split('/').map(str::to_string).collect(),
            size: self.size,
            modified_ms: self.modified_ms,
            executable: self.executable,
        }
    }
}

/// Handle to both databases. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    main: Arc<Mutex<Connection>>,
    scratch: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

fn kind_to_int(kind: EntryKind) -> i64 {
    match kind {
        EntryKind::File => 0,
        EntryKind::Folder => 1,
    }
}

fn kind_from_int(value: i64) -> EntryKind {
    if value == 1 {
        EntryKind::Folder
    } else {
        EntryKind::File
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

impl Store {
    /// Opens (creating if needed) the databases in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(Error::io)?;
        let main = Connection::open(data_dir.join("tunnel.db"))?;
        main.pragma_update(None, "journal_mode", "WAL")?;
        main.pragma_update(None, "synchronous", "NORMAL")?;
        main.execute_batch(MAIN_SCHEMA)?;

        let scratch_path = data_dir.join("scratch.db");
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut name = scratch_path.as_os_str().to_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
        let scratch = Connection::open(&scratch_path)?;
        // Disposable data: favour speed over durability.
        scratch.pragma_update(None, "journal_mode", "MEMORY")?;
        scratch.pragma_update(None, "synchronous", "OFF")?;
        scratch.pragma_update(None, "temp_store", "MEMORY")?;
        scratch.execute_batch(SCRATCH_SCHEMA)?;

        Ok(Self {
            main: Arc::new(Mutex::new(main)),
            scratch: Arc::new(Mutex::new(scratch)),
        })
    }

    fn main(&self) -> MutexGuard<'_, Connection> {
        self.main
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn scratch(&self) -> MutexGuard<'_, Connection> {
        self.scratch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ----- settings -----

    /// Reads the settings, falling back to defaults for anything missing.
    pub fn settings(&self) -> Settings {
        let json: Option<String> = self
            .main()
            .query_row(
                "SELECT value FROM settings WHERE key = 'settings'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        json.and_then(|json| serde_json::from_str::<Settings>(&json).ok())
            .unwrap_or_default()
            .normalized()
    }

    /// Saves the settings and returns them as stored (after clamping).
    pub fn save_settings(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.normalized();
        let json = serde_json::to_string(&settings).map_err(Error::internal)?;
        self.main().execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![json],
        )?;
        Ok(settings)
    }

    // ----- history -----

    /// Adds a history line, honouring the retention and file-name settings.
    pub fn add_history(&self, settings: &Settings, mut entry: HistoryEntry) -> Result<()> {
        if settings.history == HistoryRetention::Off {
            return Ok(());
        }
        if !settings.history_names {
            entry.names.clear();
        }
        let names = serde_json::to_string(&entry.names).map_err(Error::internal)?;
        self.main().execute(
            "INSERT INTO history (at, peer, direction, files, folders, bytes, result, names, folder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.at,
                entry.peer,
                entry.direction,
                to_i64(entry.files),
                to_i64(entry.folders),
                to_i64(entry.bytes),
                entry.result,
                names,
                entry.folder,
            ],
        )?;
        Ok(())
    }

    /// The most recent history lines, newest first.
    pub fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>> {
        let conn = self.main();
        let mut stmt = conn.prepare_cached(
            "SELECT id, at, peer, direction, files, folders, bytes, result, names, folder
             FROM history ORDER BY at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let names: String = row.get(8)?;
            Ok(HistoryEntry {
                id: row.get(0)?,
                at: row.get(1)?,
                peer: row.get(2)?,
                direction: row.get(3)?,
                files: to_u64(row.get(4)?),
                folders: to_u64(row.get(5)?),
                bytes: to_u64(row.get(6)?),
                result: row.get(7)?,
                names: serde_json::from_str(&names).unwrap_or_default(),
                folder: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Deletes all history.
    pub fn clear_history(&self) -> Result<()> {
        self.main().execute("DELETE FROM history", [])?;
        Ok(())
    }

    /// Deletes history older than the retention setting allows.
    pub fn prune_history(&self, retention: HistoryRetention) -> Result<()> {
        if let Some(window) = retention.window_ms() {
            let cutoff = platform::now_ms().saturating_sub(window);
            self.main()
                .execute("DELETE FROM history WHERE at < ?1", params![cutoff])?;
        }
        Ok(())
    }

    // ----- manifests of the current session -----

    /// Forgets a transfer's manifest (both directions).
    pub fn forget_transfer(&self, transfer: TransferId) {
        let conn = self.scratch();
        let _ = conn.execute(
            "DELETE FROM incoming WHERE transfer = ?1",
            params![transfer.0],
        );
        let _ = conn.execute(
            "DELETE FROM outgoing WHERE transfer = ?1",
            params![transfer.0],
        );
    }

    /// Forgets every manifest (a session ended).
    pub fn forget_all_transfers(&self) {
        let conn = self.scratch();
        let _ = conn.execute_batch("DELETE FROM incoming; DELETE FROM outgoing;");
    }

    /// Validates one manifest page from a peer and stores it.
    ///
    /// This is where the path-safety rules meet untrusted input. Every name is
    /// sanitized, every entry must sit under a folder the sender already
    /// listed, and names that would collide on a case-insensitive file system
    /// get ` (2)`. Any error rejects the whole offer and stores nothing.
    pub fn insert_incoming_page(
        &self,
        transfer: TransferId,
        first_file_id: u32,
        entries: &[ManifestEntry],
    ) -> Result<PageStats, ManifestError> {
        let conn = self.scratch();
        let tx = conn.unchecked_transaction()?;
        let mut stats = PageStats::default();
        {
            let mut find_parent = tx.prepare_cached(
                "SELECT rel, kind FROM incoming WHERE transfer = ?1 AND orig = ?2",
            )?;
            let mut fold_taken =
                tx.prepare_cached("SELECT 1 FROM incoming WHERE transfer = ?1 AND fold = ?2")?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO incoming
                 (transfer, file_id, kind, rel, orig, fold, size, mtime, exec, renamed_from)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;

            for (index, entry) in entries.iter().enumerate() {
                let expected = first_file_id
                    .checked_add(index as u32)
                    .ok_or(ManifestError::OutOfOrder)?;
                if entry.file_id != expected {
                    return Err(ManifestError::OutOfOrder);
                }
                // Rules 2 and 4 for the whole path; the parents were sanitized
                // when their own entries arrived, so only the last name is used.
                let safe = pathsafe::sanitize_path(&entry.path)?;
                let leaf = safe.last().expect("sanitize_path rejects empty paths");
                let orig_key = entry.path.join("/");

                let parent_rel = match entry.path.len() {
                    1 => None,
                    depth => {
                        let parent_key = entry.path[..depth - 1].join("/");
                        let parent: Option<(String, i64)> = find_parent
                            .query_row(params![transfer.0, parent_key], |row| {
                                Ok((row.get(0)?, row.get(1)?))
                            })
                            .optional()?;
                        match parent {
                            Some((rel, kind)) if kind_from_int(kind) == EntryKind::Folder => {
                                Some(rel)
                            }
                            _ => return Err(ManifestError::MissingParent),
                        }
                    }
                };
                if entry.kind == EntryKind::Folder {
                    let duplicate: Option<i64> = find_parent
                        .query_row(params![transfer.0, orig_key], |row| row.get(1))
                        .optional()?;
                    if duplicate.is_some() {
                        return Err(ManifestError::DuplicateFolder);
                    }
                }

                let join = |name: &str| match &parent_rel {
                    Some(parent) => format!("{parent}/{name}"),
                    None => name.to_string(),
                };
                let mut name = leaf.name.clone();
                let mut rel = join(&name);
                let mut fold = pathsafe::fold_key(&rel);
                let mut number = 1u32;
                while fold_taken.exists(params![transfer.0, fold])? {
                    number += 1;
                    if number > 10_000 {
                        return Err(ManifestError::Collisions);
                    }
                    name = pathsafe::numbered_name(&leaf.name, number);
                    rel = join(&name);
                    fold = pathsafe::fold_key(&rel);
                }
                let renamed = leaf.renamed || number > 1;
                let renamed_from = renamed.then(|| entry.path.last().cloned()).flatten();

                insert.execute(params![
                    transfer.0,
                    entry.file_id,
                    kind_to_int(entry.kind),
                    rel,
                    // Only folders are ever looked up as parents.
                    (entry.kind == EntryKind::Folder).then_some(&orig_key),
                    fold,
                    to_i64(entry.size),
                    entry.modified_ms,
                    entry.executable,
                    renamed_from,
                ])?;

                match entry.kind {
                    EntryKind::File => {
                        stats.files += 1;
                        stats.bytes = stats.bytes.saturating_add(entry.size);
                        if pathsafe::is_executable_name(&name) {
                            stats.executables += 1;
                        }
                        if pathsafe::is_double_extension(&name) {
                            stats.double_extensions += 1;
                        }
                    }
                    EntryKind::Folder => stats.folders += 1,
                }
                stats.renamed += u64::from(renamed);
                stats.bidi += u64::from(leaf.bidi);
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// Reads the next entries of an incoming transfer after `after_file_id`.
    pub fn incoming_batch(
        &self,
        transfer: TransferId,
        after_file_id: i64,
        limit: u32,
    ) -> Result<Vec<IncomingEntry>> {
        let conn = self.scratch();
        let mut stmt = conn.prepare_cached(
            "SELECT file_id, kind, rel, size, mtime, exec FROM incoming
             WHERE transfer = ?1 AND file_id > ?2 ORDER BY file_id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![transfer.0, after_file_id, limit], |row| {
            Ok(IncomingEntry {
                file_id: row.get(0)?,
                kind: kind_from_int(row.get(1)?),
                rel: row.get(2)?,
                size: to_u64(row.get(3)?),
                modified_ms: row.get(4)?,
                executable: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One entry of an incoming transfer.
    pub fn incoming_entry(
        &self,
        transfer: TransferId,
        file_id: u32,
    ) -> Result<Option<IncomingEntry>> {
        Ok(self
            .incoming_batch(transfer, i64::from(file_id) - 1, 1)?
            .into_iter()
            .find(|e| e.file_id == file_id))
    }

    /// The first names of an incoming transfer, shallowest first, for the offer.
    pub fn incoming_names(&self, transfer: TransferId, limit: u32) -> Result<Vec<String>> {
        let conn = self.scratch();
        let mut stmt = conn.prepare_cached(
            "SELECT rel, kind FROM incoming WHERE transfer = ?1
             ORDER BY length(rel) - length(replace(rel, '/', '')), file_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![transfer.0, limit], |row| {
            let rel: String = row.get(0)?;
            let kind: i64 = row.get(1)?;
            Ok(if kind_from_int(kind) == EntryKind::Folder {
                format!("{rel}/")
            } else {
                rel
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Entries that were renamed for safety: `(sender's name, saved path)`.
    pub fn incoming_renamed(
        &self,
        transfer: TransferId,
        limit: u32,
    ) -> Result<Vec<(String, String)>> {
        let conn = self.scratch();
        let mut stmt = conn.prepare_cached(
            "SELECT renamed_from, rel FROM incoming
             WHERE transfer = ?1 AND renamed_from IS NOT NULL ORDER BY file_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![transfer.0, limit], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Stores entries of an outgoing transfer as the walker finds them.
    pub fn insert_outgoing(&self, transfer: TransferId, entries: &[OutgoingEntry]) -> Result<()> {
        let conn = self.scratch();
        let tx = conn.unchecked_transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO outgoing (transfer, file_id, kind, rel, src, size, mtime, exec)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for entry in entries {
                let src = entry
                    .src
                    .to_str()
                    .ok_or_else(|| Error::internal("non-UTF-8 source path"))?;
                insert.execute(params![
                    transfer.0,
                    entry.file_id,
                    kind_to_int(entry.kind),
                    entry.rel,
                    src,
                    to_i64(entry.size),
                    entry.modified_ms,
                    entry.executable,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Reads the next entries of an outgoing transfer after `after_file_id`.
    pub fn outgoing_batch(
        &self,
        transfer: TransferId,
        after_file_id: i64,
        limit: u32,
    ) -> Result<Vec<OutgoingEntry>> {
        let conn = self.scratch();
        let mut stmt = conn.prepare_cached(
            "SELECT file_id, kind, rel, src, size, mtime, exec FROM outgoing
             WHERE transfer = ?1 AND file_id > ?2 ORDER BY file_id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![transfer.0, after_file_id, limit], |row| {
            let src: String = row.get(3)?;
            Ok(OutgoingEntry {
                file_id: row.get(0)?,
                kind: kind_from_int(row.get(1)?),
                rel: row.get(2)?,
                src: PathBuf::from(src),
                size: to_u64(row.get(4)?),
                modified_ms: row.get(5)?,
                executable: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One entry of an outgoing transfer.
    pub fn outgoing_entry(
        &self,
        transfer: TransferId,
        file_id: u32,
    ) -> Result<Option<OutgoingEntry>> {
        Ok(self
            .outgoing_batch(transfer, i64::from(file_id) - 1, 1)?
            .into_iter()
            .find(|e| e.file_id == file_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: TransferId = TransferId([1; 16]);

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn file(id: u32, path: &[&str], size: u64) -> ManifestEntry {
        ManifestEntry {
            file_id: id,
            kind: EntryKind::File,
            path: path.iter().map(|s| s.to_string()).collect(),
            size,
            modified_ms: 1_700_000_000_000,
            executable: false,
        }
    }

    fn folder(id: u32, path: &[&str]) -> ManifestEntry {
        ManifestEntry {
            kind: EntryKind::Folder,
            size: 0,
            ..file(id, path, 0)
        }
    }

    fn rels(store: &Store) -> Vec<String> {
        store
            .incoming_batch(T, -1, 1000)
            .unwrap()
            .into_iter()
            .map(|e| e.rel)
            .collect()
    }

    #[test]
    fn settings_round_trip_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.settings().theme, "system");
        let mut s = store.settings();
        s.theme = "dark".into();
        s.device_name = "Michael's Laptop".into();
        store.save_settings(s).unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.settings().theme, "dark");
        assert_eq!(store.settings().device_name, "Michael's Laptop");
    }

    #[test]
    fn history_respects_privacy_settings() {
        let (_dir, store) = store();
        let entry = |at: i64| HistoryEntry {
            id: 0,
            at,
            peer: "Phone".into(),
            direction: "receive".into(),
            files: 3,
            folders: 1,
            bytes: 1234,
            result: "done".into(),
            names: vec!["a.txt".into()],
            folder: Some("/tmp/x".into()),
        };
        let mut settings = Settings::default();
        store
            .add_history(&settings, entry(platform::now_ms()))
            .unwrap();
        settings.history_names = false;
        store
            .add_history(&settings, entry(platform::now_ms() + 1))
            .unwrap();
        settings.history = HistoryRetention::Off;
        store
            .add_history(&settings, entry(platform::now_ms() + 2))
            .unwrap();

        let history = store.history(10).unwrap();
        assert_eq!(history.len(), 2, "nothing is recorded when history is off");
        assert!(
            history[0].names.is_empty(),
            "names are dropped when the setting is off"
        );
        assert_eq!(history[1].names, vec!["a.txt".to_string()]);
        assert_eq!((history[1].files, history[1].bytes), (3, 1234));

        store
            .add_history(
                &Settings::default(),
                entry(platform::now_ms() - 40 * 86_400_000),
            )
            .unwrap();
        store.prune_history(HistoryRetention::Days30).unwrap();
        assert_eq!(store.history(10).unwrap().len(), 2);
        store.prune_history(HistoryRetention::Forever).unwrap();
        assert_eq!(store.history(10).unwrap().len(), 2);
        store.clear_history().unwrap();
        assert!(store.history(10).unwrap().is_empty());
    }

    #[test]
    fn a_normal_manifest_is_stored_with_totals() {
        let (_dir, store) = store();
        let stats = store
            .insert_incoming_page(
                T,
                0,
                &[
                    folder(0, &["Photos"]),
                    file(1, &["Photos", "a.jpg"], 10),
                    folder(2, &["Photos", "2026"]),
                    file(3, &["Photos", "2026", "b.jpg"], 20),
                    file(4, &["notes.txt"], 5),
                    folder(5, &["Empty"]),
                ],
            )
            .unwrap();
        assert_eq!(
            (stats.files, stats.folders, stats.bytes, stats.renamed),
            (3, 3, 35, 0)
        );
        assert_eq!(
            rels(&store),
            [
                "Photos",
                "Photos/a.jpg",
                "Photos/2026",
                "Photos/2026/b.jpg",
                "notes.txt",
                "Empty"
            ]
        );
        assert_eq!(
            store.incoming_names(T, 3).unwrap(),
            ["Photos/", "notes.txt", "Empty/"],
            "shallowest names first"
        );
        assert_eq!(store.incoming_entry(T, 3).unwrap().unwrap().size, 20);
        assert!(store.incoming_entry(T, 99).unwrap().is_none());
    }

    #[test]
    fn unsafe_paths_reject_the_page_and_store_nothing() {
        for bad in [
            vec![file(0, &["..", "x"], 1)],
            vec![file(0, &["a/b"], 1)],
            vec![file(0, &[""], 1)],
            vec![file(0, &["."], 1)],
            vec![file(0, &["a\0"], 1)],
            vec![folder(0, &["ok"]), file(1, &["ok", ".."], 1)],
            vec![file(0, &[], 1)],
        ] {
            let (_dir, store) = store();
            let result = store.insert_incoming_page(T, 0, &bad);
            assert!(
                matches!(result, Err(ManifestError::Unsafe(_))),
                "{bad:?} -> {result:?}"
            );
            assert!(
                rels(&store).is_empty(),
                "a rejected page must store nothing"
            );
        }
    }

    #[test]
    fn structure_rules_are_enforced() {
        let (_dir, store) = store();
        // A file inside a folder that was never listed.
        assert_eq!(
            store.insert_incoming_page(T, 0, &[file(0, &["ghost", "x"], 1)]),
            Err(ManifestError::MissingParent)
        );
        // A "folder" that is actually a file.
        assert_eq!(
            store.insert_incoming_page(T, 0, &[file(0, &["a"], 1), file(1, &["a", "x"], 1)]),
            Err(ManifestError::MissingParent)
        );
        // IDs must count up with no gaps.
        assert_eq!(
            store.insert_incoming_page(T, 0, &[file(1, &["a"], 1)]),
            Err(ManifestError::OutOfOrder)
        );
        assert_eq!(
            store.insert_incoming_page(T, 0, &[file(0, &["a"], 1), file(0, &["b"], 1)]),
            Err(ManifestError::OutOfOrder)
        );
        // The same folder twice.
        assert_eq!(
            store.insert_incoming_page(T, 0, &[folder(0, &["d"]), folder(1, &["d"])]),
            Err(ManifestError::DuplicateFolder)
        );
        assert!(rels(&store).is_empty());
    }

    #[test]
    fn pages_continue_from_the_previous_one() {
        let (_dir, store) = store();
        store
            .insert_incoming_page(T, 0, &[folder(0, &["d"]), file(1, &["d", "a"], 1)])
            .unwrap();
        store
            .insert_incoming_page(T, 2, &[file(2, &["d", "b"], 1)])
            .unwrap();
        assert_eq!(rels(&store), ["d", "d/a", "d/b"]);
        assert_eq!(
            store.insert_incoming_page(T, 3, &[file(5, &["d", "c"], 1)]),
            Err(ManifestError::OutOfOrder)
        );
    }

    #[test]
    fn colliding_and_unsafe_names_are_renamed_and_listed() {
        let (_dir, store) = store();
        let stats = store
            .insert_incoming_page(
                T,
                0,
                &[
                    file(0, &["Report.pdf"], 1),
                    file(1, &["report.PDF"], 1),
                    file(2, &["REPORT.pdf"], 1),
                    file(3, &["CON"], 1),
                    file(4, &["a:b?.txt"], 1),
                    folder(5, &["Dir"]),
                    folder(6, &["dir"]),
                    file(7, &["dir", "x.txt"], 1),
                    file(8, &["Dir", "x.txt"], 1),
                    file(9, &["e\u{301}.txt"], 1),
                    file(10, &["\u{e9}.txt"], 1),
                    file(11, &["trick\u{202e}cod.exe"], 1),
                ],
            )
            .unwrap();
        assert_eq!(
            rels(&store),
            [
                "Report.pdf",
                "report (2).PDF",
                "REPORT (3).pdf",
                "CON_",
                "a_b_.txt",
                "Dir",
                "dir (2)",
                "dir (2)/x.txt",
                "Dir/x.txt",
                "\u{e9}.txt",
                "\u{e9} (2).txt",
                "trick_cod.exe",
            ]
        );
        assert_eq!(stats.renamed, 7);
        assert_eq!(stats.bidi, 1);
        assert_eq!(stats.executables, 1);
        let renamed = store.incoming_renamed(T, 100).unwrap();
        assert_eq!(renamed.len(), 7);
        assert!(renamed.contains(&("CON".to_string(), "CON_".to_string())));
        assert!(renamed.contains(&("dir".to_string(), "dir (2)".to_string())));
    }

    #[test]
    fn warnings_are_counted_by_the_receiver() {
        let (_dir, store) = store();
        let stats = store
            .insert_incoming_page(
                T,
                0,
                &[
                    file(0, &["setup.exe"], 1),
                    file(1, &["invoice.pdf.exe"], 1),
                    file(2, &["a.txt"], 1),
                ],
            )
            .unwrap();
        assert_eq!((stats.executables, stats.double_extensions), (2, 1));
    }

    #[test]
    fn transfers_are_isolated_and_forgettable() {
        let (_dir, store) = store();
        let other = TransferId([2; 16]);
        store
            .insert_incoming_page(T, 0, &[file(0, &["a"], 1)])
            .unwrap();
        store
            .insert_incoming_page(other, 0, &[file(0, &["a"], 1)])
            .unwrap();
        assert_eq!(
            rels(&store),
            ["a"],
            "the same name in another transfer does not collide"
        );
        store.forget_transfer(T);
        assert!(rels(&store).is_empty());
        assert_eq!(store.incoming_batch(other, -1, 10).unwrap().len(), 1);
        store.forget_all_transfers();
        assert!(store.incoming_batch(other, -1, 10).unwrap().is_empty());
    }

    #[test]
    fn outgoing_entries_round_trip() {
        let (_dir, store) = store();
        let entries: Vec<OutgoingEntry> = (0..5)
            .map(|i| OutgoingEntry {
                file_id: i,
                kind: if i == 0 {
                    EntryKind::Folder
                } else {
                    EntryKind::File
                },
                rel: if i == 0 {
                    "d".into()
                } else {
                    format!("d/f{i}")
                },
                src: PathBuf::from(format!("/src/d/f{i}")),
                size: u64::from(i) * 10,
                modified_ms: -5,
                executable: i == 2,
            })
            .collect();
        store.insert_outgoing(T, &entries).unwrap();
        assert_eq!(store.outgoing_batch(T, -1, 100).unwrap(), entries);
        assert_eq!(store.outgoing_batch(T, 2, 100).unwrap(), entries[3..]);
        assert_eq!(
            store.outgoing_entry(T, 2).unwrap().as_ref(),
            Some(&entries[2])
        );
        assert_eq!(store.outgoing_entry(T, 9).unwrap(), None);
        assert_eq!(entries[1].to_manifest().path, ["d", "f1"]);
    }

    #[test]
    fn a_hundred_thousand_entries_are_fast_enough() {
        let (_dir, store) = store();
        let start = std::time::Instant::now();
        let mut id = 0u32;
        store
            .insert_incoming_page(T, 0, &[folder(0, &["big"])])
            .unwrap();
        id += 1;
        while id < 100_000 {
            let page: Vec<ManifestEntry> = (id..(id + 5000).min(100_000))
                .map(|i| file(i, &["big", &format!("file-{i:06}.bin")], 10))
                .collect();
            store.insert_incoming_page(T, id, &page).unwrap();
            id += page.len() as u32;
        }
        assert_eq!(store.incoming_batch(T, 99_998, 10).unwrap().len(), 1);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "took {:?}",
            start.elapsed()
        );
    }
}
