#![no_main]
//! The path sanitizer: whatever it accepts must be a clean, single name.

use libfuzzer_sys::fuzz_target;
use tunnel_core::pathsafe;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    // Split on a byte that cannot appear in a name so one input yields a path.
    let components: Vec<String> = text.split('\u{1f}').map(str::to_string).collect();
    if let Ok(safe) = pathsafe::sanitize_path(&components) {
        for name in &safe {
            assert!(pathsafe::is_clean_component(&name.name), "{:?}", name.name);
            assert!(!name.name.contains(['/', '\\']));
            assert!(name.name != "." && name.name != "..");
        }
    }
});
