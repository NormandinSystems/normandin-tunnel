# Release checklist

## Making the repository public (once)

- [ ] A licence is chosen and its file is in the repository root.
- [ ] The commit history has been looked over: author names, messages, and
      anything that should not be public.
- [ ] `SECURITY.md` works: private vulnerability reporting is switched on
      under Settings > Code security.
- [ ] The repository has a description and topics.
- [ ] A release exists, so that the download links in the README lead
      somewhere.

## Before tagging

- [ ] `cargo test` green on Windows and Linux in CI.
- [ ] `cargo test --release -- --ignored` run locally; numbers added to `docs/BENCHMARKS.md`.
- [ ] Each fuzz target run for one hour with no findings.
- [ ] `docs/PROGRESS.md`, `docs/DECISIONS.md` and `CHANGELOG.md` are current,
      and the user guide still matches the app.
- [ ] Version bumped in `Cargo.toml`, `app/package.json` and `app/src-tauri/tauri.conf.json`.

## Manual matrix

Each pair: pair with a code, confirm the safety code, send a folder each way,
send text, pause and resume, cancel one transfer, end the session.

| Pair | Same Wi-Fi | Different networks | Phone hotspot | Forced through relay |
|---|---|---|---|---|
| Windows app ↔ Linux app | | | | |
| Windows app ↔ Linux command line | | | | |
| Linux app ↔ Linux command line | | | | |
| Windows app ↔ Windows app | | | | |

Also check on each platform: first-run welcome, the firewall prompt on
Windows, drag and drop onto the window, notifications with the window in the
background, both languages, both themes, keyboard-only use.

## Signing (not set up yet)

- Windows: without signing, SmartScreen shows "Windows protected your PC".
  Routes: SignPath Foundation (free for open-source projects that qualify),
  Azure Trusted Signing, or an OV/EV certificate.
- Signing secrets belong in a protected GitHub environment that needs your
  approval, never in the repository.

## Publishing

Either push a tag `vX.Y.Z` and wait for the draft release to appear, or, on
GitHub, choose Releases > Draft a new release, type the new tag `vX.Y.Z`,
write the notes and publish. In the second case the installers are attached
to the release by the build about an hour later.

## After the draft release is created

- [ ] Download every file, check `SHA256SUMS.txt`.
- [ ] Install on a clean Windows machine and a clean Linux machine.
- [ ] Publish the draft.
