//! The transfer engine: listing what to send, serving file data, and receiving
//! it safely.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::events::Issue;

pub(crate) mod recv;
pub(crate) mod send;
pub(crate) mod walk;

/// Stable reason codes for skipped, failed and renamed items. The apps
/// translate them.
pub mod reason {
    /// A symbolic link in the selection (links are never followed).
    pub const LINK: &str = "link";
    /// A socket, device or pipe.
    pub const SPECIAL_FILE: &str = "special_file";
    /// A name that is not valid text and cannot be sent.
    pub const NAME_NOT_TEXT: &str = "name_not_text";
    /// Nested deeper than 64 levels.
    pub const TOO_DEEP: &str = "too_deep";
    /// Could not be read on this device.
    pub const UNREADABLE: &str = "unreadable";
    /// Could not be read on the sending device.
    pub const UNREADABLE_ON_SENDER: &str = "unreadable_on_sender";
    /// Changed while it was being sent.
    pub const CHANGED: &str = "changed";
    /// Arrived damaged twice.
    pub const HASH_MISMATCH: &str = "hash_mismatch";
    /// A file with that name exists and the rule is Skip.
    pub const EXISTS: &str = "exists";
    /// Saved under a numbered name because a file with that name exists.
    pub const KEPT_BOTH: &str = "kept_both";
    /// The other device refused to send it.
    pub const REFUSED: &str = "refused";
    /// The other device broke the protocol while sending it.
    pub const PROTOCOL: &str = "protocol";
    /// A link sits where its folder should be; it was not followed.
    pub const DESTINATION_LINK: &str = "destination_link";
    /// A file sits where its folder should be.
    pub const DESTINATION_FILE: &str = "destination_file";
    /// The full path is too long for this device.
    pub const PATH_TOO_LONG: &str = "path_too_long";
    /// Could not be written.
    pub const WRITE_FAILED: &str = "write_failed";
    /// The name was changed to be safe on this device.
    pub const UNSAFE_NAME: &str = "unsafe_name";
    /// No data arrived for too long.
    pub const TIMEOUT: &str = "timeout";
}

/// Most issues kept per transfer; beyond this only the counts are exact.
const MAX_ISSUES: usize = 500;

/// A bounded list of things worth mentioning in the transfer details.
#[derive(Debug, Default)]
pub(crate) struct IssueLog {
    items: Mutex<Vec<Issue>>,
    truncated: AtomicBool,
}

impl IssueLog {
    pub(crate) fn push(
        &self,
        path: &str,
        kind: &'static str,
        reason: &'static str,
        detail: Option<String>,
    ) {
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        if items.len() >= MAX_ISSUES {
            self.truncated.store(true, Ordering::Relaxed);
            return;
        }
        items.push(Issue {
            path: path.to_string(),
            kind,
            reason,
            detail,
        });
    }

    pub(crate) fn skipped(&self, path: &str, reason: &'static str) {
        self.push(path, "skipped", reason, None);
    }

    pub(crate) fn failed(&self, path: &str, reason: &'static str, detail: Option<String>) {
        self.push(path, "failed", reason, detail);
    }

    pub(crate) fn renamed(&self, path: &str, reason: &'static str, original: String) {
        self.push(path, "renamed", reason, Some(original));
    }

    pub(crate) fn snapshot(&self) -> (Vec<Issue>, bool) {
        let items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        (items.clone(), self.truncated.load(Ordering::Relaxed))
    }
}

/// Progress counters shared between the workers and the session loop.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    /// Bytes moved (received and written, or sent).
    pub bytes: AtomicU64,
    /// Files finished.
    pub files: AtomicU64,
    /// Files skipped.
    pub skipped: AtomicU64,
    /// Files failed.
    pub failed: AtomicU64,
    /// Bytes of files that will not arrive (skipped or failed).
    pub dropped: AtomicU64,
}

impl Counters {
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
    }

    pub(crate) fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    pub(crate) fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Whether a transfer's workers should be running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    /// Go.
    Running,
    /// Hold: keep partial data, start nothing new.
    Paused,
    /// Stop for good.
    Cancelled,
}
