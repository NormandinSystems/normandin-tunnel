//! Settings the person can change. Stored as one JSON document in SQLite.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tunnel_proto::hello::sanitize_device_name;
use tunnel_proto::limits;

use crate::pathsafe::ConflictRule;
use crate::platform;

/// The placeholder address that version 0.1.0 shipped as its default pairing
/// service. It was never deployed, so stored settings that still carry it are
/// read as "no pairing server".
const RETIRED_RENDEZVOUS_URL: &str = "wss://rendezvous.normandinsystems.com/v1";

/// How long transfer history is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRetention {
    /// Keep no history.
    Off,
    /// Keep 7 days.
    Days7,
    /// Keep 30 days.
    Days30,
    /// Keep 90 days.
    Days90,
    /// Keep everything.
    Forever,
}

impl HistoryRetention {
    /// The retention window in milliseconds, if there is one.
    pub fn window_ms(self) -> Option<i64> {
        const DAY: i64 = 86_400_000;
        match self {
            HistoryRetention::Off => Some(0),
            HistoryRetention::Days7 => Some(7 * DAY),
            HistoryRetention::Days30 => Some(30 * DAY),
            HistoryRetention::Days90 => Some(90 * DAY),
            HistoryRetention::Forever => None,
        }
    }
}

/// Every setting, with its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // General
    /// Name shown to the other device.
    pub device_name: String,
    /// `system`, `en` or `fr`.
    pub language: String,
    /// `system`, `light` or `dark`.
    pub theme: String,
    /// Show notifications for offers and finished transfers.
    pub notifications: bool,
    /// The first-run welcome has been completed.
    pub onboarded: bool,

    // Receiving
    /// Where received files are saved.
    pub save_dir: PathBuf,
    /// Ask where to save for every offer.
    pub ask_where_to_save: bool,
    /// What to do when a file already exists.
    pub conflict_rule: ConflictRule,
    /// Restore each file's modified time.
    pub keep_modified_times: bool,
    /// Restore the executable permission (Linux and macOS).
    pub keep_executable: bool,
    /// Mark received files as downloaded so the OS warns before running them.
    pub mark_downloaded: bool,

    // Privacy
    /// How long history is kept.
    pub history: HistoryRetention,
    /// Keep file names in history.
    pub history_names: bool,

    // Advanced
    /// Your own rendezvous server (`wss://host/v1`). Empty, the default, means
    /// no server: devices meet on the relay network.
    pub rendezvous_url: String,
    /// Relay servers. Empty means the default relays.
    pub relay_urls: Vec<String>,
    /// Most entries accepted in one offer.
    pub max_entries: u64,
    /// Include file names in logs.
    pub debug_logging: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: platform::default_device_name(),
            language: "system".into(),
            theme: "system".into(),
            notifications: true,
            onboarded: false,
            save_dir: platform::default_save_dir(),
            ask_where_to_save: false,
            conflict_rule: ConflictRule::KeepBoth,
            keep_modified_times: true,
            keep_executable: true,
            mark_downloaded: true,
            history: HistoryRetention::Days30,
            history_names: true,
            rendezvous_url: String::new(),
            relay_urls: Vec::new(),
            max_entries: limits::DEFAULT_MAX_ENTRIES,
            debug_logging: false,
        }
    }
}

impl Settings {
    /// Clamps every field to its allowed range. Applied to anything read from
    /// disk or sent by the UI.
    pub fn normalized(mut self) -> Self {
        self.device_name = sanitize_device_name(&self.device_name);
        if !matches!(self.language.as_str(), "system" | "en" | "fr") {
            self.language = "system".into();
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".into();
        }
        if self.save_dir.as_os_str().is_empty() || !self.save_dir.is_absolute() {
            self.save_dir = platform::default_save_dir();
        }
        self.rendezvous_url = self.rendezvous_url.trim().to_string();
        if self.rendezvous_url == RETIRED_RENDEZVOUS_URL {
            self.rendezvous_url.clear();
        }
        self.relay_urls = self
            .relay_urls
            .iter()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
            .take(8)
            .collect();
        self.max_entries = self.max_entries.clamp(1, limits::HARD_MAX_ENTRIES);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_design() {
        let s = Settings::default();
        assert_eq!(s.conflict_rule, ConflictRule::KeepBoth);
        assert!(s.keep_modified_times && s.keep_executable && s.mark_downloaded);
        assert!(!s.ask_where_to_save && s.notifications && s.history_names);
        assert_eq!(s.history, HistoryRetention::Days30);
        assert_eq!(s.max_entries, 1_000_000);
        // No pairing server and the default relays: works out of the box.
        assert!(s.rendezvous_url.is_empty() && s.relay_urls.is_empty());
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        let s: Settings = serde_json::from_str(r#"{"theme":"dark","from_the_future":1}"#).unwrap();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.language, "system");
    }

    #[test]
    fn normalization_clamps_bad_values() {
        let s = Settings {
            device_name: "\u{202e}".into(),
            language: "xx".into(),
            theme: "neon".into(),
            save_dir: PathBuf::from("relative/dir"),
            rendezvous_url: "   ".into(),
            relay_urls: vec![" ".into(), " https://relay.example.com ".into()],
            max_entries: u64::MAX,
            ..Settings::default()
        }
        .normalized();
        assert_eq!(s.device_name, "Unnamed device");
        assert_eq!(s.language, "system");
        assert_eq!(s.theme, "system");
        assert!(s.save_dir.is_absolute());
        assert_eq!(s.rendezvous_url, "");
        assert_eq!(s.relay_urls, vec!["https://relay.example.com".to_string()]);
        assert_eq!(s.max_entries, limits::HARD_MAX_ENTRIES);
    }

    #[test]
    fn no_pairing_server_is_the_default_and_the_old_placeholder_is_dropped() {
        assert_eq!(Settings::default().rendezvous_url, "");
        let old = Settings {
            rendezvous_url: " wss://rendezvous.normandinsystems.com/v1 ".into(),
            ..Settings::default()
        }
        .normalized();
        assert_eq!(old.rendezvous_url, "");
        let own = Settings {
            rendezvous_url: "wss://pair.example.org/v1".into(),
            ..Settings::default()
        }
        .normalized();
        assert_eq!(own.rendezvous_url, "wss://pair.example.org/v1");
    }
}
