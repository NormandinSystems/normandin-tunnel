//! The manifest: what a sender says it is about to send.
//!
//! Paths travel as lists of name components, never as strings with separators,
//! so there is nothing to split and no separator to smuggle. File hashes are not
//! in the manifest; they travel at the end of each file stream.
//!
//! Nothing here is trusted. The receiver re-validates every entry with its
//! path-safety rules before a single byte is written.

use serde::{Deserialize, Serialize};

use crate::control::TransferId;
use crate::frame::FrameError;
use crate::limits;

/// File or folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder (sent so that empty folders are recreated).
    Folder,
}

/// One file or folder in an offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Position in the manifest, counting from 0 with no gaps.
    pub file_id: u32,
    /// File or folder.
    pub kind: EntryKind,
    /// Name components from the top of the transfer down to this entry.
    pub path: Vec<String>,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Modified time, milliseconds since the Unix epoch.
    pub modified_ms: i64,
    /// Whether the sender's copy is executable (Linux and macOS).
    pub executable: bool,
}

impl ManifestEntry {
    /// Checks the limits that do not depend on the receiver's file system.
    ///
    /// Path *safety* (reserved names, traversal, collisions) is the receiver's
    /// job; this only bounds sizes so later code can rely on them.
    pub fn check_limits(&self) -> Result<(), FrameError> {
        if self.path.is_empty() {
            return Err(FrameError::Invalid("empty path"));
        }
        if self.path.len() > limits::MAX_PATH_DEPTH {
            return Err(FrameError::Invalid("path too deep"));
        }
        if self
            .path
            .iter()
            .any(|c| c.len() > limits::MAX_WIRE_COMPONENT_BYTES)
        {
            return Err(FrameError::Invalid("name too long"));
        }
        match self.kind {
            EntryKind::File if self.size > limits::MAX_FILE_SIZE => {
                Err(FrameError::Invalid("file too large"))
            }
            EntryKind::Folder if self.size != 0 => Err(FrameError::Invalid("folder with a size")),
            _ => Ok(()),
        }
    }
}

/// Up to 1 MiB of manifest entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPage {
    /// The transfer this page belongs to.
    pub transfer_id: TransferId,
    /// Page number, counting from 0.
    pub page: u32,
    /// The entries, in `file_id` order.
    pub entries: Vec<ManifestEntry>,
}

/// Splits entries into pages whose encoded size stays under the page limit.
#[derive(Debug)]
pub struct Paginator {
    transfer_id: TransferId,
    next_page: u32,
    current: Vec<ManifestEntry>,
    current_bytes: usize,
}

impl Paginator {
    /// Starts paginating a transfer's manifest.
    pub fn new(transfer_id: TransferId) -> Self {
        Self {
            transfer_id,
            next_page: 0,
            current: Vec::new(),
            current_bytes: 0,
        }
    }

    /// Adds an entry; returns a full page when one is ready.
    pub fn push(&mut self, entry: ManifestEntry) -> Option<ManifestPage> {
        let size = encoded_size(&entry);
        let full = !self.current.is_empty()
            && self.current_bytes + size > limits::MAX_MANIFEST_PAGE - PAGE_OVERHEAD;
        let page = full.then(|| self.take_page());
        self.current_bytes += size;
        self.current.push(entry);
        page
    }

    /// Returns the final, possibly partial, page.
    pub fn finish(mut self) -> Option<ManifestPage> {
        (!self.current.is_empty()).then(|| self.take_page())
    }

    /// Pages produced so far (including one still being filled).
    pub fn pages(&self) -> u32 {
        self.next_page + u32::from(!self.current.is_empty())
    }

    fn take_page(&mut self) -> ManifestPage {
        let page = ManifestPage {
            transfer_id: self.transfer_id,
            page: self.next_page,
            entries: std::mem::take(&mut self.current),
        };
        self.next_page += 1;
        self.current_bytes = 0;
        page
    }
}

/// Room left in a page for the transfer ID, page number and entry count.
const PAGE_OVERHEAD: usize = 64;

/// Upper bound on the encoded size of one entry.
pub fn encoded_size(entry: &ManifestEntry) -> usize {
    // file_id (<=5) + kind (1) + path len (<=2) + size (<=10) + mtime (<=10) + exec (1)
    29 + entry.path.iter().map(|c| c.len() + 3).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlMessage;
    use crate::frame;

    fn entry(id: u32, name: &str) -> ManifestEntry {
        ManifestEntry {
            file_id: id,
            kind: EntryKind::File,
            path: vec!["folder".into(), name.into()],
            size: u64::MAX >> 24,
            modified_ms: i64::MIN,
            executable: true,
        }
    }

    #[test]
    fn size_bound_holds() {
        for name in ["a", "a much longer name.txt", &"é".repeat(300)] {
            let e = entry(u32::MAX, name);
            let real = postcard::to_allocvec(&e).unwrap().len();
            assert!(real <= encoded_size(&e), "{real} > {}", encoded_size(&e));
        }
    }

    #[test]
    fn pages_fit_in_a_control_frame() {
        let mut paginator = Paginator::new(TransferId([7; 16]));
        let mut pages = Vec::new();
        let name = "x".repeat(200);
        for id in 0..20_000 {
            pages.extend(paginator.push(entry(id, &name)));
        }
        assert_eq!(paginator.pages() as usize, pages.len() + 1);
        pages.extend(paginator.finish());
        assert!(pages.len() > 2);
        let mut next_id = 0;
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.page as usize, i);
            for e in &page.entries {
                assert_eq!(e.file_id, next_id);
                next_id += 1;
            }
            let message = ControlMessage::ManifestPage(page.clone());
            frame::encode(&message, limits::MAX_CONTROL_FRAME).expect("page fits in a frame");
        }
        assert_eq!(next_id, 20_000);
    }

    #[test]
    fn limits_are_checked() {
        let mut e = entry(0, "a");
        assert!(e.check_limits().is_ok());
        e.path.clear();
        assert!(e.check_limits().is_err());
        e.path = vec!["a".into(); limits::MAX_PATH_DEPTH + 1];
        assert!(e.check_limits().is_err());
        e.path = vec!["a".repeat(limits::MAX_WIRE_COMPONENT_BYTES + 1)];
        assert!(e.check_limits().is_err());
        e.path = vec!["a".into()];
        e.size = limits::MAX_FILE_SIZE + 1;
        assert!(e.check_limits().is_err());
        e.kind = EntryKind::Folder;
        e.size = 1;
        assert!(e.check_limits().is_err());
    }
}
