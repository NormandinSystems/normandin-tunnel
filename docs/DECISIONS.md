# Decisions

Choices made while building version 0.1, and the ones still open.

## Made

| Decision | Choice | Why |
|---|---|---|
| iroh version | Pinned to exactly 1.5.0 | An exact pin, so that an upgrade never happens by accident. Upgrade on purpose, with the full test suite. All iroh use is in `tunnel-core/src/net.rs` and `session.rs`. |
| Address publishing | None | Sessions exchange addresses in the sealed hello, and the meeting point is found by dialing every relay, so nothing needs to be published to DNS or pkarr. That removes a tracking surface (T13). |
| Relays | iroh's public relays by default | They are free and need no setup, which is what makes the app work out of the box. Their operators describe them as for development, testing and hobby projects, so this is a choice for a preview, not for heavy use. Self-hosted relays are a setting, and `deploy/` has a starting point. |
| Pairing without a server | The default. The device showing a code waits at a meeting point on the relay network whose key is derived from the code's four public digits | Early private builds needed a pairing server at an address that was never deployed, so nothing could pair out of the box. This needs no server of ours, and the meeting point is trusted exactly as little as a rendezvous server was. See `PROTOCOL.md` section 3. |
| Finding the meeting point | The typing device dials the derived ID through every relay it knows | It cannot know which relay the other device picked. iroh sends the first packets on every known path, so one of them arrives. Nothing is published to DNS or pkarr. |
| The number in the code | Four random digits, 1000–9999, picked by the device | Short enough to type. The security is in the three words; the number is only an address. Collisions and probing are discussed under "Known gaps" in `THREAT_MODEL.md`. |
| One wrong guess ends the code | Kept as it was with a server | A retry would be friendlier to typos, but the app already checks every word against the list before connecting, and "one guess per code" is the property the whole design leans on. |
| Rendezvous server | Optional; off unless an address is set | Still the right tool for a network without internet access, or for many users at once. A stored copy of the address used by early private builds is read as "none". |
| Plain `ws://` | Allowed only for loopback and private-network addresses | For testing and LAN self-hosting. Pairing security does not depend on the rendezvous transport, but metadata privacy does. |
| File hashing | BLAKE3 over the whole file, sent as a trailer | A 100 GB file is read once, and the first byte moves immediately. |
| Manifest storage | SQLite scratch database on disk, recreated at every start | A million entries do not fit comfortably in memory. |
| Manifest order | A folder must be listed before its contents | Lets the receiver map each entry to its (possibly renamed) parent without holding the tree in memory, and rejects malformed manifests early. |
| Requests that overtake `Accept` | The sender waits up to 3 s for the `Accept` | Streams are independent, so a file request can arrive before the `Accept` it depends on. |
| Resume | Within a session only | A partial file is resumed after a pause or a reconnect. Resuming after an app restart needs a way to recognise the same device again, which arrives with trusted devices. Partial folders are removed when a session ends. |
| RustCrypto versions | hkdf 0.12, hmac 0.12, sha2 0.10, chacha20poly1305 0.10 | The same generation `spake2` 0.4 itself builds on. |
| Licence allow-list | Permissive licences only (MIT, Apache-2.0, BSD, ISC and similar), plus MPL-2.0 and CDLA-Permissive-2.0; the full list is in `deny.toml` | Required by parts of the Tauri web-view stack and by the root-certificate data. GPL and AGPL stay denied. |
| Our own licence | MIT or Apache-2.0, at the user's choice | The usual pairing for Rust projects: as permissive as it gets, with Apache-2.0's explicit patent grant for those who want it. |
| `--yes` in the command line | Answers the safety-code question as well as offers | Scripts cannot answer either. Documented as "only when you control both devices". |

## Open

* **Own relays as the default.** The public relays are fine for a preview. A
  release meant for regular use should ship with relays this project runs.
* **Code signing.** Windows builds are unsigned today, so SmartScreen warns.
  SignPath Foundation offers free signing to open-source projects that
  qualify; Azure Trusted Signing and ordinary certificates are the paid
  routes.
* **An independent security review**, and what to do before one exists.
* **Third-party notices in the installers.** The licences of the libraries
  the app is built from are checked in CI, but their notices are not yet
  bundled with the installed program.
* **macOS and phones.** Not built. The core is written to be shared with
  them.
