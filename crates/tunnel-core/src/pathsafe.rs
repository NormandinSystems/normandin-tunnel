//! Path safety. Receiver side, never relaxed.
//!
//! A peer's manifest is untrusted input. Everything it names must land inside
//! the one destination folder the person chose, with names that are safe on
//! every platform (files often move to Windows later).
//!
//! The rules, numbered as in the design document:
//!
//! 1. Paths arrive as lists of name components, never as strings with separators.
//! 2. An empty component, `.`, `..`, or one containing `/` or NUL rejects the
//!    whole offer: a well-behaved sender can never produce these.
//! 3. Names that are legal on the sender but unsafe here are renamed, and the
//!    rename is listed in the transfer details.
//! 4. At most 64 levels deep; very long Windows paths are refused.
//! 5. Names are normalized to Unicode NFC; names that collide when case is
//!    ignored get ` (2)`.
//! 6. The joined path must still be inside the destination, as text and after
//!    resolving the parent folder's real path.
//! 7. Folders are created one level at a time and nothing that is a symbolic
//!    link, junction or reparse point is ever entered.
//! 8. Nothing is overwritten silently; the conflict rule decides.
//! 9. No special permission bits (see `platform::set_executable`).
//! 10. Nothing received is ever opened, previewed, unpacked or run.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tunnel_proto::hello::is_bidi_control;
use tunnel_proto::limits;
use unicode_normalization::UnicodeNormalization;

use crate::platform;

/// Why an entire offer is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unsafe {
    /// No components at all.
    #[error("empty path")]
    EmptyPath,
    /// An empty name component.
    #[error("empty name")]
    EmptyComponent,
    /// `.` or `..`.
    #[error("relative path component")]
    DotComponent,
    /// A `/` inside a name.
    #[error("path separator inside a name")]
    Separator,
    /// A NUL byte inside a name.
    #[error("NUL inside a name")]
    Nul,
    /// More than 64 levels.
    #[error("path too deep")]
    TooDeep,
}

/// A name made safe for every platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeName {
    /// The name to use on disk.
    pub name: String,
    /// The name differs from what the sender called it.
    pub renamed: bool,
    /// The original contained bidirectional-text controls (worth a warning).
    pub bidi: bool,
}

const WINDOWS_FORBIDDEN: [char; 8] = ['\\', ':', '<', '>', '"', '|', '?', '*'];

/// Applies rules 2, 3 and 5 (normalization) to one name component.
pub fn sanitize_component(raw: &str) -> Result<SafeName, Unsafe> {
    if raw.is_empty() {
        return Err(Unsafe::EmptyComponent);
    }
    if raw == "." || raw == ".." {
        return Err(Unsafe::DotComponent);
    }
    if raw.contains('/') {
        return Err(Unsafe::Separator);
    }
    if raw.contains('\0') {
        return Err(Unsafe::Nul);
    }

    let normalized: String = raw.nfc().collect();
    let mut bidi = false;
    let mut name: String = normalized
        .chars()
        .map(|c| {
            if is_bidi_control(c) {
                bidi = true;
                '_'
            } else if c.is_control()
                || WINDOWS_FORBIDDEN.contains(&c)
                || matches!(c, '\u{2028}' | '\u{2029}')
            {
                '_'
            } else {
                c
            }
        })
        .collect();

    fix_trailing(&mut name);
    if is_windows_device_name(&name) {
        let stem_end = name.find('.').unwrap_or(name.len());
        name.insert(stem_end, '_');
    }
    if name.len() > limits::MAX_COMPONENT_BYTES {
        name = shorten(&name, limits::MAX_COMPONENT_BYTES);
        fix_trailing(&mut name);
    }
    debug_assert!(is_clean_component(&name), "sanitizer produced {name:?}");
    let renamed = name != normalized;
    Ok(SafeName {
        name,
        renamed,
        bidi,
    })
}

/// Windows silently drops trailing spaces and dots; replace them instead.
fn fix_trailing(name: &mut String) {
    let keep = name.trim_end_matches([' ', '.']).len();
    let tail = name.len() - keep;
    if tail > 0 {
        name.truncate(keep);
        name.extend(std::iter::repeat_n('_', tail));
    }
}

/// CON, PRN, AUX, NUL, COM1–COM9, LPT1–LPT9 (and their superscript-digit
/// spellings, CONIN$ and CONOUT$), in any case, with or without an extension.
pub fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    chars.next().is_none()
        && matches!(prefix.as_str(), "COM" | "LPT")
        && digit.is_some_and(|d| matches!(d, '1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'))
}

/// Shortens a name to `max` bytes, keeping a reasonable extension.
fn shorten(name: &str, max: usize) -> String {
    let (stem, ext) = split_extension(name);
    let room = max.saturating_sub(ext.len()).max(1);
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(max);
    out.push_str(&stem[..cut.max(first_char_len(stem))]);
    out.push_str(ext);
    while out.len() > max {
        out.pop();
    }
    out
}

fn first_char_len(text: &str) -> usize {
    text.chars().next().map_or(0, char::len_utf8)
}

/// Splits `photo.tar.gz` into (`photo.tar`, `.gz`). Extensions longer than 16
/// bytes, and names that are only an extension, are not treated as extensions.
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= 16 && dot + 1 < name.len() => {
            name.split_at(dot)
        }
        _ => (name, ""),
    }
}

/// `name (2).ext`, `name (3).ext`, … for collisions and "Keep both".
pub fn numbered_name(name: &str, number: u32) -> String {
    let (stem, ext) = split_extension(name);
    let suffix = format!(" ({number})");
    let room = limits::MAX_COMPONENT_BYTES
        .saturating_sub(ext.len() + suffix.len())
        .max(1);
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}{ext}", &stem[..cut])
}

/// The key under which two names collide on a case-insensitive file system.
pub fn fold_key(text: &str) -> String {
    text.nfc().flat_map(char::to_lowercase).collect()
}

/// The invariants every sanitized component satisfies. Checked again at the
/// point of use, so a bug upstream cannot turn into a write outside the folder.
pub fn is_clean_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= limits::MAX_COMPONENT_BYTES
        && !name.ends_with([' ', '.'])
        && !name.chars().any(|c| {
            c == '/' || c.is_control() || WINDOWS_FORBIDDEN.contains(&c) || is_bidi_control(c)
        })
        && !is_windows_device_name(name)
}

/// Checks the whole-path rules (2 and 4) and sanitizes every component.
pub fn sanitize_path(components: &[String]) -> Result<Vec<SafeName>, Unsafe> {
    if components.is_empty() {
        return Err(Unsafe::EmptyPath);
    }
    if components.len() > limits::MAX_PATH_DEPTH {
        return Err(Unsafe::TooDeep);
    }
    components.iter().map(|c| sanitize_component(c)).collect()
}

/// What to do when a file with the same name already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictRule {
    /// Keep the existing file and save the new one as `name (2).ext`.
    #[default]
    KeepBoth,
    /// Replace the existing file.
    Replace,
    /// Keep the existing file and drop the new one.
    Skip,
}

/// Why a file or folder could not be placed.
#[derive(Debug, thiserror::Error)]
pub enum PlaceError {
    /// A component failed the clean-name check at the point of use.
    #[error("unsafe name")]
    UnsafeName,
    /// The path would leave the destination folder.
    #[error("path leaves the destination folder")]
    Escapes,
    /// Something on the way is a symbolic link, junction or reparse point.
    #[error("a folder on the way is a link")]
    Link,
    /// A file exists where a folder is needed.
    #[error("a file is in the way of a folder")]
    NotAFolder,
    /// The full path is longer than the platform allows.
    #[error("path too long")]
    TooLong,
    /// No free name was found for "Keep both".
    #[error("too many files with the same name")]
    NoFreeName,
    /// The operating system refused.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// How a finished file ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placed {
    /// Saved at this path (possibly under a numbered name).
    Saved {
        /// Where the file is.
        path: PathBuf,
        /// It was given a numbered name because of a conflict.
        numbered: bool,
    },
    /// Dropped because a file with that name exists and the rule is Skip.
    Skipped,
}

/// The destination folder: the only place received data may go.
#[derive(Debug, Clone)]
pub struct Destination {
    root: PathBuf,
}

impl Destination {
    /// Opens (creating if needed) the folder the person chose.
    ///
    /// The real path is resolved once here. On Windows that is the
    /// extended-length (`\\?\`) form, which lifts the 260-character limit and
    /// stops Windows from reinterpreting names.
    pub fn open(folder: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(folder)?;
        Ok(Self {
            root: std::fs::canonicalize(folder)?,
        })
    }

    /// The resolved destination folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Rule 6, textual half: joins clean components and confirms the result is
    /// inside the destination.
    pub fn join(&self, components: &[&str]) -> Result<PathBuf, PlaceError> {
        let mut path = self.root.clone();
        for component in components {
            if !is_clean_component(component) {
                return Err(PlaceError::UnsafeName);
            }
            path.push(component);
        }
        let inside = path.starts_with(&self.root)
            && path.components().count() == self.root.components().count() + components.len();
        if !inside {
            return Err(PlaceError::Escapes);
        }
        if cfg!(windows) && path.as_os_str().len() > limits::MAX_WINDOWS_PATH_CHARS {
            return Err(PlaceError::TooLong);
        }
        Ok(path)
    }

    /// Rule 7: creates a folder path one level at a time, refusing to pass
    /// through anything that is a link, then (rule 6, second half) confirms the
    /// real path is still inside the destination.
    pub fn ensure_folder(&self, components: &[&str]) -> Result<PathBuf, PlaceError> {
        let full = self.join(components)?;
        let mut current = self.root.clone();
        for component in components {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) => check_is_plain_folder(&metadata)?,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    match std::fs::create_dir(&current) {
                        Ok(()) => {}
                        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                            check_is_plain_folder(&std::fs::symlink_metadata(&current)?)?;
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
        let real = std::fs::canonicalize(&full)?;
        if !real.starts_with(&self.root) {
            return Err(PlaceError::Escapes);
        }
        Ok(full)
    }

    /// Rule 8: moves a verified file from the partial folder into `folder`
    /// (which must come from [`Destination::ensure_folder`]) under `name`.
    pub fn place(
        &self,
        part: &Path,
        folder: &Path,
        name: &str,
        rule: ConflictRule,
    ) -> Result<Placed, PlaceError> {
        if !is_clean_component(name) {
            return Err(PlaceError::UnsafeName);
        }
        if !folder.starts_with(&self.root) {
            return Err(PlaceError::Escapes);
        }
        let target = folder.join(name);
        if cfg!(windows) && target.as_os_str().len() > limits::MAX_WINDOWS_PATH_CHARS {
            return Err(PlaceError::TooLong);
        }
        let existing = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => Some(metadata),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        match (rule, existing) {
            (ConflictRule::Skip, Some(_)) => {
                let _ = std::fs::remove_file(part);
                Ok(Placed::Skipped)
            }
            // A folder cannot be replaced by a file: keep both instead.
            (ConflictRule::Replace, Some(metadata))
                if !metadata.is_dir() || platform::is_link(&metadata) =>
            {
                if platform::is_link(&metadata) {
                    // Remove the link itself; never write through it.
                    remove_link(&target, &metadata)?;
                }
                std::fs::rename(part, &target)?;
                Ok(Placed::Saved {
                    path: target,
                    numbered: false,
                })
            }
            _ => place_without_replacing(part, folder, name),
        }
    }
}

fn check_is_plain_folder(metadata: &std::fs::Metadata) -> Result<(), PlaceError> {
    if platform::is_link(metadata) {
        Err(PlaceError::Link)
    } else if !metadata.is_dir() {
        Err(PlaceError::NotAFolder)
    } else {
        Ok(())
    }
}

fn remove_link(target: &Path, metadata: &std::fs::Metadata) -> io::Result<()> {
    // On Windows a link to a folder is removed with remove_dir.
    if cfg!(windows) && metadata.is_dir() {
        std::fs::remove_dir(target)
    } else {
        std::fs::remove_file(target)
    }
}

/// Tries `name`, then `name (2)`, `name (3)`, … without ever replacing a file.
///
/// A hard link fails atomically when the name exists, which closes the gap
/// between checking and moving. Where hard links are unavailable (FAT, exFAT)
/// it falls back to check-then-rename.
fn place_without_replacing(part: &Path, folder: &Path, name: &str) -> Result<Placed, PlaceError> {
    for attempt in 1..=10_000u32 {
        let candidate_name = if attempt == 1 {
            name.to_string()
        } else {
            numbered_name(name, attempt)
        };
        let candidate = folder.join(&candidate_name);
        match std::fs::hard_link(part, &candidate) {
            Ok(()) => {
                let _ = std::fs::remove_file(part);
                return Ok(Placed::Saved {
                    path: candidate,
                    numbered: attempt > 1,
                });
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => match std::fs::symlink_metadata(&candidate) {
                Ok(_) => continue,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    std::fs::rename(part, &candidate)?;
                    return Ok(Placed::Saved {
                        path: candidate,
                        numbered: attempt > 1,
                    });
                }
                Err(err) => return Err(err.into()),
            },
        }
    }
    Err(PlaceError::NoFreeName)
}

/// Extensions of programs and scripts, which get a warning on the offer.
pub const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "js", "jse", "wsf", "hta", "lnk",
    "reg", "dll", "sys", "jar", "apk", "app", "dmg", "pkg", "sh", "command",
];

/// The lower-cased final extension, without the dot.
pub fn extension(name: &str) -> Option<String> {
    let (_, ext) = split_extension(name);
    (!ext.is_empty()).then(|| ext[1..].to_lowercase())
}

/// Whether a name ends in a program or script extension.
pub fn is_executable_name(name: &str) -> bool {
    extension(name).is_some_and(|ext| EXECUTABLE_EXTENSIONS.contains(&ext.as_str()))
}

/// Whether a name hides a program behind a harmless-looking extension, like
/// `report.pdf.exe`.
pub fn is_double_extension(name: &str) -> bool {
    if !is_executable_name(name) {
        return false;
    }
    let (stem, _) = split_extension(name);
    let (inner_stem, inner_ext) = split_extension(stem);
    !inner_stem.is_empty()
        && (2..=5).contains(&inner_ext.len())
        && inner_ext[1..].chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn clean(raw: &str) -> SafeName {
        sanitize_component(raw).unwrap_or_else(|e| panic!("{raw:?} rejected: {e}"))
    }

    #[test]
    fn rule_2_rejects_the_whole_offer() {
        assert_eq!(sanitize_component(""), Err(Unsafe::EmptyComponent));
        assert_eq!(sanitize_component("."), Err(Unsafe::DotComponent));
        assert_eq!(sanitize_component(".."), Err(Unsafe::DotComponent));
        assert_eq!(sanitize_component("a/b"), Err(Unsafe::Separator));
        assert_eq!(sanitize_component("/"), Err(Unsafe::Separator));
        assert_eq!(sanitize_component("/etc/passwd"), Err(Unsafe::Separator));
        assert_eq!(sanitize_component("../../x"), Err(Unsafe::Separator));
        assert_eq!(sanitize_component("a\0b"), Err(Unsafe::Nul));
        assert_eq!(sanitize_path(&[]), Err(Unsafe::EmptyPath));
        assert_eq!(
            sanitize_path(&vec!["a".to_string(); 65]),
            Err(Unsafe::TooDeep)
        );
        assert!(sanitize_path(&vec!["a".to_string(); 64]).is_ok());
        assert_eq!(
            sanitize_path(&["ok".into(), "..".into()]),
            Err(Unsafe::DotComponent)
        );
    }

    /// (sender's name, name on disk). At least 100 hostile and unusual names.
    const TABLE: &[(&str, &str)] = &[
        // Ordinary names pass through untouched.
        ("report.pdf", "report.pdf"),
        ("Photo 2026-11-17.jpg", "Photo 2026-11-17.jpg"),
        (".gitignore", ".gitignore"),
        ("archive.tar.gz", "archive.tar.gz"),
        ("no extension", "no extension"),
        ("-rf", "-rf"),
        ("~", "~"),
        ("$RECYCLE.BIN", "$RECYCLE.BIN"),
        ("résumé.docx", "résumé.docx"),
        ("日本語のファイル.txt", "日本語のファイル.txt"),
        ("中文文件名.pdf", "中文文件名.pdf"),
        ("📷 vacances 🏖️.png", "📷 vacances 🏖️.png"),
        ("Ünïcödé", "Ünïcödé"),
        ("a b  c", "a b  c"),
        ("name with 'quotes'.txt", "name with 'quotes'.txt"),
        ("semi;colon&amp.txt", "semi;colon&amp.txt"),
        ("100%.txt", "100%.txt"),
        ("#hash@tag!.md", "#hash@tag!.md"),
        (
            "(parens) [brackets] {braces}",
            "(parens) [brackets] {braces}",
        ),
        ("...leading dots", "...leading dots"),
        (" leading space", " leading space"),
        // Windows-forbidden characters each become an underscore.
        ("a\\b", "a_b"),
        (
            "..\\..\\Windows\\System32\\x.dll",
            ".._.._Windows_System32_x.dll",
        ),
        ("C:\\", "C__"),
        ("C:", "C_"),
        ("C:file.txt", "C_file.txt"),
        ("a:b", "a_b"),
        ("file.txt:Zone.Identifier", "file.txt_Zone.Identifier"),
        ("file.txt::$DATA", "file.txt__$DATA"),
        ("a<b", "a_b"),
        ("a>b", "a_b"),
        ("a\"b", "a_b"),
        ("a|b", "a_b"),
        ("a?b", "a_b"),
        ("a*b", "a_b"),
        ("<>:\"|?*", "_______"),
        ("what?.txt", "what_.txt"),
        ("*.*", "_._"),
        ("\\\\server\\share", "__server_share"),
        ("\\\\?\\C:\\x", "____C__x"),
        // Control characters.
        ("a\nb", "a_b"),
        ("a\rb", "a_b"),
        ("a\tb", "a_b"),
        ("bell\u{7}.txt", "bell_.txt"),
        ("esc\u{1b}[31m.txt", "esc_[31m.txt"),
        ("del\u{7f}.txt", "del_.txt"),
        ("c1\u{85}.txt", "c1_.txt"),
        ("line\u{2028}sep", "line_sep"),
        ("para\u{2029}sep", "para_sep"),
        // Bidirectional-text controls.
        ("invoice\u{202e}fdp.exe", "invoice_fdp.exe"),
        ("a\u{200e}b", "a_b"),
        ("a\u{200f}b", "a_b"),
        ("a\u{202a}b", "a_b"),
        ("a\u{202b}b", "a_b"),
        ("a\u{202c}b", "a_b"),
        ("a\u{202d}b", "a_b"),
        ("a\u{2066}b", "a_b"),
        ("a\u{2067}b", "a_b"),
        ("a\u{2068}b", "a_b"),
        ("a\u{2069}b", "a_b"),
        // Trailing spaces and dots.
        ("name.", "name_"),
        ("name..", "name__"),
        ("name ", "name_"),
        ("name . .", "name____"),
        ("...", "___"),
        (" ", "_"),
        ("   ", "___"),
        (". .", "___"),
        ("x.txt.", "x.txt_"),
        ("x.txt ", "x.txt_"),
        // Windows device names, any case, with or without extension.
        ("CON", "CON_"),
        ("con", "con_"),
        ("Con", "Con_"),
        ("PRN", "PRN_"),
        ("AUX", "AUX_"),
        ("NUL", "NUL_"),
        ("nul", "nul_"),
        ("COM1", "COM1_"),
        ("com9", "com9_"),
        ("LPT1", "LPT1_"),
        ("lpt9", "lpt9_"),
        ("CON.txt", "CON_.txt"),
        ("nul.tar.gz", "nul_.tar.gz"),
        ("aux.c", "aux_.c"),
        ("COM1.log", "COM1_.log"),
        ("LPT3.prn", "LPT3_.prn"),
        ("COM\u{b9}", "COM\u{b9}_"),
        ("LPT\u{b2}.txt", "LPT\u{b2}_.txt"),
        ("CONIN$", "CONIN$_"),
        ("CONOUT$", "CONOUT$_"),
        ("CON .txt", "CON _.txt"),
        ("CON.", "CON_"),
        ("NUL ", "NUL_"),
        // Device-name lookalikes that are fine.
        ("CONSOLE", "CONSOLE"),
        ("COM0", "COM0"),
        ("COM10", "COM10"),
        ("LPT", "LPT"),
        ("NULL", "NULL"),
        ("AUXILIARY.txt", "AUXILIARY.txt"),
        ("my.CON", "my.CON"),
        ("_CON", "_CON"),
        // Unicode normalization: decomposed forms become composed (NFC).
        ("e\u{301}.txt", "\u{e9}.txt"),
        ("A\u{30a}", "\u{c5}"),
        // Lookalike separators are not separators and stay as they are.
        ("a\u{2215}b", "a\u{2215}b"),
        ("a\u{ff0f}b", "a\u{ff0f}b"),
        ("a\u{29f8}b", "a\u{29f8}b"),
        ("..\u{2024}", "..\u{2024}"),
        ("\u{ff0e}\u{ff0e}", "\u{ff0e}\u{ff0e}"),
        // URL-style and shell-style tricks are just names.
        ("%2e%2e%2f", "%2e%2e%2f"),
        ("..%5c..%5c", "..%5c..%5c"),
        ("$(rm -rf ~)", "$(rm -rf ~)"),
        ("`id`", "`id`"),
        ("; shutdown -h now", "; shutdown -h now"),
        ("file.txt\u{a0}", "file.txt\u{a0}"),
        ("desktop.ini", "desktop.ini"),
        ("autorun.inf", "autorun.inf"),
        ("Thumbs.db", "Thumbs.db"),
        (".tunnel-partial", ".tunnel-partial"),
    ];

    #[test]
    fn rule_3_hostile_name_table() {
        assert!(TABLE.len() >= 100, "the table has {} rows", TABLE.len());
        for (raw, expected) in TABLE {
            let safe = clean(raw);
            assert_eq!(&safe.name, expected, "sanitizing {raw:?}");
            assert!(
                is_clean_component(&safe.name),
                "{:?} is not clean",
                safe.name
            );
            let normalized: String = raw.nfc().collect();
            assert_eq!(
                safe.renamed,
                safe.name != normalized,
                "renamed flag for {raw:?}"
            );
        }
    }

    #[test]
    fn bidi_controls_raise_a_warning() {
        assert!(clean("invoice\u{202e}fdp.exe").bidi);
        assert!(!clean("invoice.pdf").bidi);
    }

    #[test]
    fn long_names_are_shortened_keeping_the_extension() {
        let long = format!("{}.jpeg", "x".repeat(400));
        let safe = clean(&long);
        assert_eq!(safe.name.len(), 255);
        assert!(safe.name.ends_with(".jpeg"));
        assert!(safe.renamed);

        let multibyte = format!("{}.txt", "é".repeat(300));
        let safe = clean(&multibyte);
        assert!(safe.name.len() <= 255);
        assert!(safe.name.ends_with(".txt"));

        let no_ext = "y".repeat(1000);
        assert_eq!(clean(&no_ext).name.len(), 255);

        let dots = format!("{}. ", "z".repeat(300));
        let safe = clean(&dots);
        assert!(safe.name.len() <= 255 && is_clean_component(&safe.name));
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered_name("photo.jpg", 2), "photo (2).jpg");
        assert_eq!(numbered_name("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered_name("README", 2), "README (2)");
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc (2)");
        let long = format!("{}.txt", "x".repeat(251));
        let numbered = numbered_name(&long, 12);
        assert!(numbered.len() <= 255 && numbered.ends_with(" (12).txt"));
        assert!(is_clean_component(&numbered));
    }

    #[test]
    fn fold_key_ignores_case_and_normalization() {
        assert_eq!(fold_key("Photo.JPG"), fold_key("photo.jpg"));
        assert_eq!(fold_key("E\u{301}"), fold_key("\u{e9}"));
        assert_ne!(fold_key("a"), fold_key("b"));
    }

    #[test]
    fn executable_warnings() {
        for name in [
            "setup.exe",
            "a.MSI",
            "run.bat",
            "x.ps1",
            "tool.sh",
            "App.dmg",
            "lib.dll",
            "a.lnk",
        ] {
            assert!(is_executable_name(name), "{name}");
        }
        for name in ["photo.jpg", "notes.txt", "exe", "archive.tar.gz", ".sh"] {
            assert!(!is_executable_name(name), "{name}");
        }
        assert!(is_double_extension("report.pdf.exe"));
        assert!(is_double_extension("photo.jpg.scr"));
        assert!(is_double_extension("Invoice.DOCX.Bat"));
        assert!(!is_double_extension("setup.exe"));
        assert!(!is_double_extension("report.pdf"));
        assert!(!is_double_extension("my.long-name-part.exe"));
    }

    #[test]
    fn join_keeps_paths_inside_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        let path = dest.join(&["a", "b", "c.txt"]).unwrap();
        assert!(path.starts_with(dest.root()));
        for bad in [
            &[".."][..],
            &["a", "..", "b"],
            &["."],
            &["a/b"],
            &["a\\b"],
            &[""],
            &["C:"],
            &["CON"],
            &["x."],
        ] {
            assert!(
                matches!(dest.join(bad), Err(PlaceError::UnsafeName)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn ensure_folder_creates_levels_and_refuses_files_in_the_way() {
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        let folder = dest.ensure_folder(&["one", "two", "three"]).unwrap();
        assert!(folder.is_dir());
        assert_eq!(
            dest.ensure_folder(&["one", "two", "three"]).unwrap(),
            folder
        );
        assert_eq!(dest.ensure_folder(&[]).unwrap(), dest.root());
        std::fs::write(dest.root().join("file"), b"x").unwrap();
        assert!(matches!(
            dest.ensure_folder(&["file", "sub"]),
            Err(PlaceError::NotAFolder)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rule_7_never_enters_a_symbolic_link() {
        let outside = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), dest.root().join("link")).unwrap();
        assert!(matches!(
            dest.ensure_folder(&["link"]),
            Err(PlaceError::Link)
        ));
        assert!(matches!(
            dest.ensure_folder(&["link", "sub"]),
            Err(PlaceError::Link)
        ));
        std::fs::create_dir(dest.root().join("real")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dest.root().join("real").join("deep")).unwrap();
        assert!(matches!(
            dest.ensure_folder(&["real", "deep", "x"]),
            Err(PlaceError::Link)
        ));
        assert_eq!(
            std::fs::read_dir(outside.path()).unwrap().count(),
            0,
            "nothing was created outside"
        );
    }

    fn part(dir: &Path, content: &[u8]) -> PathBuf {
        let path = dir.join(format!("{}.part", content.len()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn rule_8_keep_both_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        let folder = dest.ensure_folder(&["in"]).unwrap();
        let staging = dest.ensure_folder(&[".tunnel-partial"]).unwrap();

        let first = dest
            .place(
                &part(&staging, b"1"),
                &folder,
                "a.txt",
                ConflictRule::KeepBoth,
            )
            .unwrap();
        assert_eq!(
            first,
            Placed::Saved {
                path: folder.join("a.txt"),
                numbered: false
            }
        );
        let second = dest
            .place(
                &part(&staging, b"22"),
                &folder,
                "a.txt",
                ConflictRule::KeepBoth,
            )
            .unwrap();
        assert_eq!(
            second,
            Placed::Saved {
                path: folder.join("a (2).txt"),
                numbered: true
            }
        );
        let third = dest
            .place(
                &part(&staging, b"333"),
                &folder,
                "a.txt",
                ConflictRule::KeepBoth,
            )
            .unwrap();
        assert_eq!(
            third,
            Placed::Saved {
                path: folder.join("a (3).txt"),
                numbered: true
            }
        );
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"1");
        assert_eq!(std::fs::read(folder.join("a (2).txt")).unwrap(), b"22");
        assert_eq!(
            std::fs::read_dir(&staging).unwrap().count(),
            0,
            "part files are consumed"
        );
    }

    #[test]
    fn rule_8_replace_and_skip() {
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        let folder = dest.ensure_folder(&["in"]).unwrap();
        let staging = dest.ensure_folder(&[".tunnel-partial"]).unwrap();
        std::fs::write(folder.join("a.txt"), b"old").unwrap();

        let skipped = dest
            .place(
                &part(&staging, b"new!"),
                &folder,
                "a.txt",
                ConflictRule::Skip,
            )
            .unwrap();
        assert_eq!(skipped, Placed::Skipped);
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        let replaced = dest
            .place(
                &part(&staging, b"newer"),
                &folder,
                "a.txt",
                ConflictRule::Replace,
            )
            .unwrap();
        assert_eq!(
            replaced,
            Placed::Saved {
                path: folder.join("a.txt"),
                numbered: false
            }
        );
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"newer");

        // A folder is never replaced by a file.
        std::fs::create_dir(folder.join("thing")).unwrap();
        let kept = dest
            .place(
                &part(&staging, b"file"),
                &folder,
                "thing",
                ConflictRule::Replace,
            )
            .unwrap();
        assert_eq!(
            kept,
            Placed::Saved {
                path: folder.join("thing (2)"),
                numbered: true
            }
        );
        assert!(folder.join("thing").is_dir());

        // Skip with nothing in the way saves normally.
        let saved = dest
            .place(
                &part(&staging, b"fresh!"),
                &folder,
                "b.txt",
                ConflictRule::Skip,
            )
            .unwrap();
        assert_eq!(
            saved,
            Placed::Saved {
                path: folder.join("b.txt"),
                numbered: false
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn replace_removes_a_link_instead_of_writing_through_it() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = Destination::open(dir.path()).unwrap();
        let staging = dest.ensure_folder(&[".tunnel-partial"]).unwrap();
        std::os::unix::fs::symlink(&victim, dest.root().join("a.txt")).unwrap();

        dest.place(
            &part(&staging, b"new"),
            dest.root(),
            "a.txt",
            ConflictRule::Replace,
        )
        .unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        assert_eq!(std::fs::read(dest.root().join("a.txt")).unwrap(), b"new");
        assert!(
            !std::fs::symlink_metadata(dest.root().join("a.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // Keep both treats a dangling link as "name taken".
        std::os::unix::fs::symlink(outside.path().join("missing"), dest.root().join("b.txt"))
            .unwrap();
        let placed = dest
            .place(
                &part(&staging, b"data"),
                dest.root(),
                "b.txt",
                ConflictRule::KeepBoth,
            )
            .unwrap();
        assert_eq!(
            placed,
            Placed::Saved {
                path: dest.root().join("b (2).txt"),
                numbered: true
            }
        );
        assert!(!outside.path().join("missing").exists());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// The path sanitizer never yields a path outside the destination, for
        /// any input.
        #[test]
        fn sanitized_paths_never_leave_the_destination(
            components in proptest::collection::vec(
                prop_oneof![
                    "\\PC{0,40}",
                    "[./\\\\: ]{0,6}",
                    "(\\.\\.|CON|nul|com1|\\.|[a-z]{1,8})(\\.[a-z]{1,3})?",
                    proptest::collection::vec(any::<char>(), 0..24).prop_map(|c| c.into_iter().collect::<String>()),
                ],
                0..8,
            )
        ) {
            let root = Path::new(if cfg!(windows) { r"\\?\C:\dest" } else { "/dest" });
            let dest = Destination { root: root.to_path_buf() };
            match sanitize_path(&components) {
                Err(_) => {}
                Ok(safe) => {
                    let names: Vec<&str> = safe.iter().map(|s| s.name.as_str()).collect();
                    for name in &names {
                        prop_assert!(is_clean_component(name), "{:?}", name);
                    }
                    let path = dest.join(&names).expect("clean components always join");
                    prop_assert!(path.starts_with(root));
                    prop_assert_eq!(path.components().count(), root.components().count() + names.len());
                    for component in path.strip_prefix(root).unwrap().components() {
                        prop_assert!(matches!(component, std::path::Component::Normal(_)));
                    }
                }
            }
        }

        #[test]
        fn sanitizer_is_idempotent(raw in "\\PC{1,60}") {
            if let Ok(first) = sanitize_component(&raw) {
                let second = sanitize_component(&first.name).unwrap();
                prop_assert_eq!(&second.name, &first.name);
                prop_assert!(!second.renamed);
            }
        }

        #[test]
        fn numbered_names_stay_clean(raw in "\\PC{1,300}", n in 2u32..100_000) {
            if let Ok(safe) = sanitize_component(&raw) {
                let numbered = numbered_name(&safe.name, n);
                prop_assert!(is_clean_component(&numbered), "{:?}", numbered);
            }
        }
    }
}
