# Changelog

Notable changes in each version. Newest first.

## 0.1.0 — 26 November 2026

The first public preview. For Windows and Linux. Free and open source under
the MIT or Apache-2.0 licence, at your choice.

### What is in it

* Desktop app in English and French, with light and dark themes.
* Pairing with a one-time code (`4821-orbit-velvet-candle`) that works once
  and expires after 10 minutes. No account and no server to set up: devices
  meet through public relays.
* A 6-digit safety code that both people confirm before anything is sent.
* Files, folders and text in either direction. The receiver sees each offer,
  with warnings for programs, scripts and misleading names, and accepts or
  declines.
* Direct connections when the networks allow it, with an encrypted relay as
  fallback.
* Every file checked against a BLAKE3 fingerprint before it is saved.
* Pause, resume and cancel. A dropped connection reconnects for up to two
  minutes and continues where it stopped.
* A command-line program, `tunnel`, that works with the desktop app.
* An optional pairing server, and settings to use your own relays.
* History kept only on the device, for 30 days by default.

### Known limits

* No independent security review.
* Installers are not code-signed.
* No macOS, Android or iOS version.
* No automatic updates.
* Both devices need internet access to pair.
* A transfer cannot be continued after the session has ended.

See [docs/PROGRESS.md](docs/PROGRESS.md) for the full list.
