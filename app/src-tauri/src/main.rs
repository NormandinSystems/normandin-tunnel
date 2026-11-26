//! Normandin Tunnel desktop app.

// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

fn main() {
    tunnel_app::run();
}
