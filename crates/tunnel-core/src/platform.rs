//! The few places where platforms differ: where data lives, the device's name,
//! the downloaded-file mark and the executable bit.

use std::path::{Path, PathBuf};

use tunnel_proto::hello::sanitize_device_name;

/// Folder for the database and logs: `NormandinSystems/Tunnel` under the
/// platform's application-data folder.
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NormandinSystems")
        .join("Tunnel")
}

/// Default folder for received files: `Downloads/Tunnel`.
pub fn default_save_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Tunnel")
}

/// The computer's name, used as the default device name.
pub fn default_device_name() -> String {
    sanitize_device_name(&gethostname::gethostname().to_string_lossy())
}

/// Adds the mark browsers put on downloads, so the operating system warns
/// before a received file is run.
///
/// Windows: a `Zone.Identifier` stream with `ZoneId=3` (Internet). Other
/// platforms have no equivalent a non-sandboxed app can set without special
/// privileges, so this does nothing there. Failures are ignored: the stream is
/// unsupported on non-NTFS volumes.
pub fn mark_downloaded(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(stream, b"[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Makes a received file executable, where that means something.
///
/// Adds execute permission only where read permission already exists, and
/// never touches setuid, setgid or sticky bits.
pub fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            let with_exec = mode | ((mode & 0o444) >> 2);
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(with_exec));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Whether the sender's copy of a file is executable.
pub fn is_executable(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

/// Whether a path is a symbolic link, junction or other reparse point: anything
/// that would take a write somewhere other than where the path says.
pub fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// A path as people expect to read it: without Windows' `\\?\` prefix.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    text.into_owned()
}

/// Modified time in milliseconds since the Unix epoch (negative before 1970).
pub fn modified_ms(metadata: &std::fs::Metadata) -> i64 {
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// The inverse of [`modified_ms`].
pub fn system_time_from_ms(ms: i64) -> std::time::SystemTime {
    let magnitude = std::time::Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 {
        std::time::UNIX_EPOCH + magnitude
    } else {
        std::time::UNIX_EPOCH
            .checked_sub(magnitude)
            .unwrap_or(std::time::UNIX_EPOCH)
    }
}

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    modified_since_epoch(std::time::SystemTime::now())
}

fn modified_since_epoch(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strips_the_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\Users\m\file.txt")),
            r"C:\Users\m\file.txt"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\x")),
            r"\\server\share\x"
        );
        assert_eq!(
            display_path(Path::new("/home/m/file.txt")),
            "/home/m/file.txt"
        );
    }

    #[test]
    fn times_round_trip() {
        for ms in [0i64, 1, 1_700_000_000_123, -86_400_000] {
            let time = system_time_from_ms(ms);
            let back = match time.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_millis() as i64,
                Err(e) => -(e.duration().as_millis() as i64),
            };
            assert_eq!(back, ms);
        }
    }

    #[test]
    fn defaults_are_sensible() {
        assert!(
            default_data_dir().ends_with("NormandinSystems/Tunnel")
                || default_data_dir().ends_with(r"NormandinSystems\Tunnel")
        );
        assert!(default_save_dir().ends_with("Tunnel"));
        assert!(!default_device_name().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_never_adds_special_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tool");
        std::fs::write(&file, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        set_executable(&file);
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o750);
        assert!(is_executable(&std::fs::metadata(&file).unwrap()));
    }
}
