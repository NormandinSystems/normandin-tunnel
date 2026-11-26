# Status

What is built, what is not, and how it was checked. Version 0.1.0.

The project was planned in 15 parts. Here is where each one stands.

| Part | Status | Notes |
|---|---|---|
| 0 Kickoff | Done | Workspace, CI for Windows and Linux, docs |
| 1 Codes and crypto | Done | Test vectors in `docs/PROTOCOL.md`, cross-checked by `docs/test_vectors.py` |
| 2 Wire protocol | Done | Six fuzz targets, run nightly in CI for ten minutes each |
| 3 Rendezvous | Done | Pairing needs no server by default (meeting point on the relay network); the rendezvous server is optional. Both tested |
| 4 Secure session | Done | Pairing, channel binding, safety code, text |
| 5 Transfer engine | Done | Manifests, offers, data streams, path safety, conflicts, pause, cancel, reconnect |
| 6 Command line | Done | `tunnel send`, `tunnel receive`; `tunnel devices` is a stub |
| 7 Servers | Partly | `deploy/` is written but has not been run on a real server |
| 8 Hardening | Partly | Hostile-peer harness passes; fuzzers run nightly but not yet for hours on end |
| 9 Desktop app | Done for Windows and Linux | English and French; see "Not in this version" |
| 10 Trusted devices | Not started | |
| 11 Android | Not started | |
| 12 iOS | Not started | |
| 13 Release pipeline | Partly | Unsigned installers and a draft release from a tag; no signing, updater or SBOM |
| 14 Final review | Not started | No outside security review |

## Not in this version

* macOS, Android and iOS builds.
* Trusted devices ("Remember this device"), and with them: reconnecting
  without a code, and resuming a transfer after an app restart.
* Code signing, the self-updater, and the daily update check.
* "Prevent sleep during transfers", "start at login", "export diagnostics".
* The Windows Explorer and share-menu entries.
* Pairing on a network with no internet access, without a pairing server on
  that network (no local discovery yet).
* Relay-only data path testing: in the automated tests the two engines always
  reach each other directly once paired.
* The macOS quarantine attribute.
* An accessibility audit with a screen reader and an automated checker. The
  UI is built with labels, roles, keyboard focus and reduced-motion support,
  but it has not been audited.

## How it was checked

Every push runs `.github/workflows/ci.yml` on `ubuntu-24.04` and
`windows-2025`:

* Formatting, Clippy with warnings as errors, and the whole test suite: unit
  and property tests, the integration tests (two engines and a local
  rendezvous server over loopback), and the hostile-peer harness.
* The installers are built on both systems: NSIS `.exe` and `.msi` on Windows;
  `.deb`, `.rpm` and AppImage on Linux.
* Pairing with no server is tested through relays started inside the test,
  including a stranger who guesses, stalls, sends garbage, or pretends to be
  the device showing the code (`tests/no_server.rs`).
* Separately, two engines with untouched default settings pair and send a file
  through the real public relays from the CI machines. This step depends on
  servers the project does not run, so it reports its result without failing
  the build.
* The packaged app is started on both systems. The check passes only once the
  web view has loaded the interface and the interface has called the engine.
  A screenshot of the running app is kept with each run
  (`docs/screenshots/windows.png` is one of them).
* Licences, advisories and sources (cargo-deny), and a scan for secrets.

By hand, on Linux only: the real app paired with the command-line app, both
showed the same safety code, and a folder with nested folders, an empty
folder, a Unicode file name and a 50 MB file arrived byte for byte identical.

Not checked: two separate machines on a LAN or across the internet (the
public-relay check runs both engines on one machine), a relay-only data path,
the installers themselves being installed, and Windows beyond the start-up
check above. No person has used the Windows build yet.

## Measured

See `docs/BENCHMARKS.md`.
