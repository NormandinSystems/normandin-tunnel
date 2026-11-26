//! Listing what the person chose to send.
//!
//! Walks the chosen files and folders without following symbolic links. Links,
//! sockets, devices and pipes are skipped and listed in the sender's report.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tunnel_proto::control::TransferId;
use tunnel_proto::limits;
use tunnel_proto::manifest::{EntryKind, Paginator};

use super::reason;
use crate::error::{Error, Result};
use crate::events::Issue;
use crate::pathsafe;
use crate::platform;
use crate::store::{OutgoingEntry, Store};

/// What the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkResult {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    /// Manifest pages the entries will be sent in.
    pub pages: u32,
    /// Name of the first top-level item.
    pub label: String,
    /// Number of top-level items.
    pub top_level: u64,
    /// Items that will not be sent, with the reason.
    pub skipped: Vec<Issue>,
    pub skipped_truncated: bool,
}

const BATCH: usize = 2000;
const MAX_SKIPPED_LISTED: usize = 500;

struct Walker<'a> {
    store: &'a Store,
    transfer: TransferId,
    cancel: &'a AtomicBool,
    max_entries: u64,
    next_id: u32,
    batch: Vec<OutgoingEntry>,
    paginator: Paginator,
    result: WalkResult,
}

impl Walker<'_> {
    fn skip(&mut self, path: &Path, reason: &'static str) {
        if self.result.skipped.len() >= MAX_SKIPPED_LISTED {
            self.result.skipped_truncated = true;
            return;
        }
        self.result.skipped.push(Issue {
            path: platform::display_path(path),
            kind: "skipped",
            reason,
            detail: None,
        });
    }

    fn emit(
        &mut self,
        kind: EntryKind,
        components: &[String],
        src: &Path,
        metadata: &std::fs::Metadata,
    ) -> Result<()> {
        if u64::from(self.next_id) >= self.max_entries {
            return Err(Error::TooManyEntries);
        }
        let (size, executable) = match kind {
            EntryKind::File => (metadata.len(), platform::is_executable(metadata)),
            EntryKind::Folder => (0, false),
        };
        let entry = OutgoingEntry {
            file_id: self.next_id,
            kind,
            rel: components.join("/"),
            src: src.to_path_buf(),
            size,
            modified_ms: platform::modified_ms(metadata),
            executable,
        };
        self.next_id += 1;
        match kind {
            EntryKind::File => {
                self.result.files += 1;
                self.result.bytes = self.result.bytes.saturating_add(size);
            }
            EntryKind::Folder => self.result.folders += 1,
        }
        // Counts pages exactly as they will be cut when the manifest is sent.
        let _ = self.paginator.push(entry.to_manifest());
        self.batch.push(entry);
        if self.batch.len() >= BATCH {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.batch.is_empty() {
            self.store.insert_outgoing(self.transfer, &self.batch)?;
            self.batch.clear();
        }
        Ok(())
    }

    /// Whether a name can travel: valid text, and nothing a receiver would
    /// treat as an attack (a `/` or NUL inside a name cannot occur on disk).
    fn wire_name(name: &std::ffi::OsStr) -> Option<String> {
        let name = name.to_str()?;
        (!name.is_empty()
            && name.len() <= limits::MAX_WIRE_COMPONENT_BYTES
            && !name.contains(['/', '\0']))
        .then(|| name.to_string())
    }

    fn visit(&mut self, path: &Path, components: Vec<String>) -> Result<()> {
        let mut pending: Vec<(PathBuf, Vec<String>)> = Vec::new();
        self.visit_one(path, components, &mut pending)?;
        while let Some((folder, components)) = pending.pop() {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let entries = match std::fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(_) => {
                    self.skip(&folder, reason::UNREADABLE);
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    self.skip(&folder, reason::UNREADABLE);
                    continue;
                };
                let child = entry.path();
                let Some(name) = Self::wire_name(&entry.file_name()) else {
                    self.skip(&child, reason::NAME_NOT_TEXT);
                    continue;
                };
                if components.len() + 1 > limits::MAX_PATH_DEPTH {
                    self.skip(&child, reason::TOO_DEEP);
                    continue;
                }
                let mut child_components = components.clone();
                child_components.push(name);
                self.visit_one(&child, child_components, &mut pending)?;
            }
        }
        Ok(())
    }

    fn visit_one(
        &mut self,
        path: &Path,
        components: Vec<String>,
        pending: &mut Vec<(PathBuf, Vec<String>)>,
    ) -> Result<()> {
        // symlink_metadata never follows a link.
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                self.skip(path, reason::UNREADABLE);
                return Ok(());
            }
        };
        if platform::is_link(&metadata) {
            self.skip(path, reason::LINK);
        } else if metadata.is_dir() {
            self.emit(EntryKind::Folder, &components, path, &metadata)?;
            pending.push((path.to_path_buf(), components));
        } else if metadata.is_file() {
            if path.to_str().is_none() {
                self.skip(path, reason::NAME_NOT_TEXT);
            } else {
                self.emit(EntryKind::File, &components, path, &metadata)?;
            }
        } else {
            self.skip(path, reason::SPECIAL_FILE);
        }
        Ok(())
    }
}

/// Lists everything under `roots` into the store and returns the totals.
///
/// Blocking: call from a blocking thread.
pub(crate) fn walk(
    store: &Store,
    transfer: TransferId,
    roots: &[PathBuf],
    max_entries: u64,
    cancel: &AtomicBool,
) -> Result<WalkResult> {
    let mut walker = Walker {
        store,
        transfer,
        cancel,
        max_entries,
        next_id: 0,
        batch: Vec::with_capacity(BATCH),
        paginator: Paginator::new(transfer),
        result: WalkResult {
            files: 0,
            folders: 0,
            bytes: 0,
            pages: 0,
            label: String::new(),
            top_level: 0,
            skipped: Vec::new(),
            skipped_truncated: false,
        },
    };

    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut used_names: HashSet<String> = HashSet::new();
    for root in roots {
        let root = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        if !seen_paths.insert(root.clone()) {
            continue;
        }
        let Some(name) = root.file_name().and_then(Walker::wire_name) else {
            walker.skip(&root, reason::NAME_NOT_TEXT);
            continue;
        };
        // Two chosen items with the same name (from different folders) would
        // collide at the top of the transfer: number the later ones.
        let mut unique = name.clone();
        let mut number = 1;
        while !used_names.insert(pathsafe::fold_key(&unique)) {
            number += 1;
            unique = pathsafe::numbered_name(&name, number);
        }
        let before = walker.next_id;
        walker.visit(&root, vec![unique.clone()])?;
        if walker.next_id > before {
            if walker.result.top_level == 0 {
                walker.result.label = unique;
            }
            walker.result.top_level += 1;
        }
    }
    walker.flush()?;
    walker.result.pages = walker.paginator.pages();
    if walker.next_id == 0 {
        return Err(Error::NothingToSend);
    }
    Ok(walker.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Store, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        (data, store, tempfile::tempdir().unwrap())
    }

    const T: TransferId = TransferId([3; 16]);

    fn rels(store: &Store) -> Vec<String> {
        let mut rels: Vec<String> = store
            .outgoing_batch(T, -1, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| e.rel)
            .collect();
        rels.sort();
        rels
    }

    #[test]
    fn walks_files_and_folders_with_parents_first() {
        let (_data, store, src) = setup();
        let root = src.path().join("Project");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("README.md"), b"hello").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("src/deep/zero"), b"").unwrap();
        std::fs::write(src.path().join("single.txt"), b"abc").unwrap();

        let cancel = AtomicBool::new(false);
        let result = walk(
            &store,
            T,
            &[root.clone(), src.path().join("single.txt")],
            1000,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            (result.files, result.folders, result.bytes),
            (4, 4, 5 + 12 + 3)
        );
        assert_eq!(
            (result.label.as_str(), result.top_level, result.pages),
            ("Project", 2, 1)
        );
        assert!(result.skipped.is_empty());
        assert_eq!(
            rels(&store),
            [
                "Project",
                "Project/README.md",
                "Project/empty",
                "Project/src",
                "Project/src/deep",
                "Project/src/deep/zero",
                "Project/src/main.rs",
                "single.txt",
            ]
        );
        // Every entry's folder is listed before it.
        let entries = store.outgoing_batch(T, -1, 10_000).unwrap();
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.file_id as usize, i);
            if let Some((parent, _)) = entry.rel.rsplit_once('/') {
                let parent_pos = entries.iter().position(|e| e.rel == parent).unwrap();
                assert!(parent_pos < i, "{} listed before its folder", entry.rel);
                assert_eq!(entries[parent_pos].kind, EntryKind::Folder);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_and_special_files_are_skipped_and_listed() {
        let (_data, store, src) = setup();
        let root = src.path().join("stuff");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("real.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("link-to-file")).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("link-to-dir")).unwrap();
        std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();

        let result = walk(&store, T, &[root], 1000, &AtomicBool::new(false)).unwrap();
        assert_eq!(rels(&store), ["stuff", "stuff/real.txt"]);
        let mut reasons: Vec<&str> = result.skipped.iter().map(|i| i.reason).collect();
        reasons.sort();
        assert_eq!(reasons, [reason::LINK, reason::LINK, reason::SPECIAL_FILE]);
    }

    #[cfg(unix)]
    #[test]
    fn a_chosen_link_is_not_followed() {
        let (_data, store, src) = setup();
        std::os::unix::fs::symlink("/etc", src.path().join("etc-link")).unwrap();
        let result = walk(
            &store,
            T,
            &[src.path().join("etc-link")],
            1000,
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(Error::NothingToSend));
    }

    #[test]
    fn duplicate_top_level_names_are_numbered() {
        let (_data, store, src) = setup();
        for dir in ["a", "b"] {
            std::fs::create_dir_all(src.path().join(dir).join("Photos")).unwrap();
            std::fs::write(src.path().join(dir).join("Photos/x.jpg"), dir).unwrap();
        }
        let roots = [
            src.path().join("a/Photos"),
            src.path().join("b/Photos"),
            src.path().join("a/Photos"),
        ];
        let result = walk(&store, T, &roots, 1000, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            result.top_level, 2,
            "the same path chosen twice is sent once"
        );
        assert_eq!(
            rels(&store),
            ["Photos", "Photos (2)", "Photos (2)/x.jpg", "Photos/x.jpg"]
        );
    }

    #[test]
    fn limits_and_empty_selections() {
        let (_data, store, src) = setup();
        for i in 0..10 {
            std::fs::write(src.path().join(format!("f{i}")), b"x").unwrap();
        }
        let roots: Vec<PathBuf> = (0..10).map(|i| src.path().join(format!("f{i}"))).collect();
        assert_eq!(
            walk(&store, T, &roots, 5, &AtomicBool::new(false)),
            Err(Error::TooManyEntries)
        );
        let none = [src.path().join("missing")];
        assert_eq!(
            walk(
                &store,
                TransferId([4; 16]),
                &none,
                5,
                &AtomicBool::new(false)
            ),
            Err(Error::NothingToSend)
        );
    }

    #[test]
    fn cancelling_stops_the_walk() {
        let (_data, store, src) = setup();
        std::fs::create_dir(src.path().join("d")).unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            walk(&store, T, &[src.path().join("d")], 100, &cancel),
            Err(Error::Cancelled)
        );
    }
}
