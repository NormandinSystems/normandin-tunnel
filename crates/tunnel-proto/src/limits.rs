//! Hard limits. A peer that exceeds any of them is treated as hostile.

use std::time::Duration;

/// One mebibyte.
pub const MIB: usize = 1024 * 1024;

/// Largest control-stream frame.
pub const MAX_CONTROL_FRAME: usize = MIB;
/// Largest encoded manifest page (a page travels inside one control frame).
pub const MAX_MANIFEST_PAGE: usize = MIB - 64;
/// Largest request or header frame on a data stream.
pub const MAX_DATA_FRAME: usize = 1024;
/// Default cap on entries (files plus folders) in one offer.
pub const DEFAULT_MAX_ENTRIES: u64 = 1_000_000;
/// Highest value the "entries per offer" setting may be raised to on desktop.
pub const HARD_MAX_ENTRIES: u64 = 10_000_000;
/// Most manifest bytes stored for one offer.
pub const MAX_MANIFEST_BYTES: u64 = 512 * MIB as u64;
/// Largest single file (1 TiB).
pub const MAX_FILE_SIZE: u64 = 1 << 40;
/// Largest text message, in bytes of UTF-8.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Offers that may wait for a decision at once.
pub const MAX_PENDING_OFFERS: usize = 4;
/// Transfers that may run at once in a session.
pub const MAX_ACTIVE_TRANSFERS: usize = 8;
/// Deepest folder nesting accepted.
pub const MAX_PATH_DEPTH: usize = 64;
/// Longest single name component, in bytes, as sent on the wire.
pub const MAX_WIRE_COMPONENT_BYTES: usize = 1024;
/// Longest name component kept on disk, in bytes.
pub const MAX_COMPONENT_BYTES: usize = 255;
/// Longest full path on Windows (extended-length form), in characters.
pub const MAX_WINDOWS_PATH_CHARS: usize = 32_000;
/// Longest error or reason text carried in a message.
pub const MAX_REASON_BYTES: usize = 256;

/// Files at or above this size count as "large" for download concurrency.
pub const LARGE_FILE_THRESHOLD: u64 = 8 * MIB as u64;
/// Large files downloaded at once.
pub const MAX_PARALLEL_LARGE: usize = 4;
/// Small files downloaded at once.
pub const MAX_PARALLEL_SMALL: usize = 32;
/// Read and write buffer size for file data.
pub const IO_BUFFER: usize = MIB;

/// How long a connection code (and its rendezvous room) lives.
pub const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// How long to wait for the other device's hello after key confirmation.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to keep dialing the other device.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for the channel-binding frame on a new connection.
pub const BIND_TIMEOUT: Duration = Duration::from_secs(15);
/// How long to keep trying to reconnect after a dropped connection.
pub const RECONNECT_WINDOW: Duration = Duration::from_secs(120);

/// Longest device name, in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;
/// Most direct addresses a hello may carry.
pub const MAX_DIRECT_ADDRS: usize = 32;
/// Longest relay URL in a hello, in bytes.
pub const MAX_RELAY_URL_BYTES: usize = 255;
/// Longest app version string in a hello, in bytes.
pub const MAX_APP_VERSION_BYTES: usize = 32;
/// Most protocol versions a hello may list.
pub const MAX_VERSIONS: usize = 16;
