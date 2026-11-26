# Development

How to build, run and test Normandin Tunnel from source.

## What you need

* **Rust.** The version in `rust-toolchain.toml` installs itself the first
  time you run `cargo`, if you have [rustup](https://rustup.rs).
* **Node.js 22**, for the desktop app's interface.
* **Linux only:** WebKitGTK 4.1 and friends.

  ```sh
  sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev \
    libayatana-appindicator3-dev libxdo-dev libssl-dev patchelf
  ```

* **Windows only:** the WebView2 runtime. It is part of Windows 11; on
  Windows 10 the installer fetches it.

## Build

```sh
# Desktop app. Installers land in target/release/bundle/
cd app
npm ci
npm run tauri build

# Command-line app and the optional pairing server
cargo build --release -p tunnel-cli -p tunnel-rendezvous
```

To work on the app with live reload: `cd app && npm run tauri dev`.

To look at the interface in a browser with sample data and no engine, run
`npm run dev` in `app/` and open the address it prints with `?demo=home`,
`?demo=code`, `?demo=safety` or `?demo=session` added. Add `&theme=dark` or
`&lang=fr` to try the other theme or language. The screenshots in the README
are made this way; the sample data is in `app/src/lib/mock.ts`.

## Test

```sh
cargo test             # unit, property, integration, no-server, hostile peer
cd app && npm test     # translations and formatting helpers
```

What the suites cover:

* **Unit and property tests** in each crate: codes, the key schedule against
  fixed vectors, framing, limits, and the path-safety rules against a table of
  hostile names.
* **Integration** (`tests/integration.rs`): two real engines and a local
  pairing server over loopback. Files, folder trees, thousands of small files,
  Unicode names, name collisions, conflict rules, pause, cancel, a cut
  connection that reconnects and resumes, wrong and expired codes.
* **No server** (`tests/no_server.rs`): pairing through relays only. The
  meeting point is found whichever relay it is on; wrong words fail on both
  sides and use up the code; a stranger who knows only the number can lurk,
  send garbage, guess once, or pretend to be the other device, and gets
  nothing.
* **Hostile peer** (`tests/evil_peer.rs`): a fake peer with the right code
  sends `..` and `/` in names, reserved Windows names, ten million entries,
  oversized frames, a wrong binding, extra bytes past a file's size, a wrong
  hash, and requests for files it was never offered. Passing means nothing is
  written outside the destination folder and nothing crashes.

Tests that need more than a normal run:

```sh
# 5 GiB file and 100,000 small files, in release mode
cargo test --release -p tunnel-tests --test integration -- --ignored --nocapture

# Two engines with default settings through the real public relays
cargo test -p tunnel-tests --test no_server -- --ignored --nocapture default_settings
```

Fuzz targets are in `fuzz/` (`cargo fuzz run control_frame`, and so on). They
need a nightly toolchain and `cargo-fuzz`.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same on Windows and Linux, builds the installers, starts the
packaged app to check that it comes up, and checks dependency licences and
advisories with `cargo-deny`.

## Layout

```
crates/tunnel-proto        wire format, limits, versions
crates/tunnel-crypto       codes, SPAKE2, key schedule, safety code
crates/tunnel-core         sessions, transfers, path safety, storage
crates/tunnel-cli          the `tunnel` command
crates/tunnel-rendezvous   the optional pairing server
app/                       desktop app (Tauri 2, React, TypeScript)
tests/                     integration tests and the hostile-peer harness
fuzz/                      cargo-fuzz targets
deploy/                    Docker Compose for your own servers
docs/                      everything in the documentation index
```

[DESIGN.md](DESIGN.md) explains how these fit together.

## Ground rules in the code

* No `unsafe`. The workspace forbids it.
* Everything that arrives from the network has a size limit, checked before
  anything is allocated. The limits are in `crates/tunnel-proto/src/limits.rs`.
* Names from the other device go through `crates/tunnel-core/src/pathsafe.rs`
  before they touch the disk. Nothing else builds a destination path.
* Logs leave out file names, paths, codes and keys unless the person turned
  on "Include file names in logs".
* The interface can only call the commands listed in
  `app/src-tauri/build.rs`.
* Every user-facing string exists in both `app/src/i18n/en.json` and
  `fr.json`; a test fails if they drift apart.
