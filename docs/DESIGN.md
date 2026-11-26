# Design overview

How the program is put together. For what the devices say to each other, see
[`PROTOCOL.md`](PROTOCOL.md); for the plain-language version, see
[`HOW_IT_WORKS.md`](HOW_IT_WORKS.md).

## Shape

Each device runs the same Rust core. Two cores meet once to agree on keys: at
a meeting point on the relay network by default, or in a room on a rendezvous
server if one is configured. After that they talk over an encrypted QUIC
connection ([iroh](https://iroh.computer)), directly when they can and through
a relay when a direct path is impossible.

| Crate | What it does |
|---|---|
| `tunnel-proto` | Message types, framing, size limits, version negotiation. No networking or disk access. |
| `tunnel-crypto` | Codes, word list, SPAKE2, key schedule, key confirmation, sealed hello, channel binding, safety code. |
| `tunnel-core` | Meeting point and rendezvous client, iroh endpoint, sessions, transfer engine, path safety, SQLite store. |
| `tunnel-cli` | The `tunnel` command-line app. |
| `tunnel-rendezvous` | The optional pairing server. Memory only. |
| `app/src-tauri` | Tauri 2 shell: the command allowlist and event forwarding. |
| `app/src` | React + TypeScript UI, English and French. |
| `tests` | Integration tests and the hostile-peer harness. |
| `fuzz` | cargo-fuzz targets. |

## Inside one device

The UI never touches the network or chooses raw paths. It calls a short
allowlist of commands (`create_code`, `join_code`, `confirm_safety`,
`send_paths`, `respond_offer`, `cancel_transfer`, …) and listens to one event
stream. The allowlist is declared in `app/src-tauri/build.rs` and granted in
`app/src-tauri/capabilities/default.json`; the Content-Security-Policy is in
`tauri.conf.json`.

A session is one task that owns all of its state
(`tunnel-core/src/session.rs`). Each transfer runs as its own tasks, connected
by bounded channels so a slow disk or a slow peer cannot exhaust memory:

* Sender: a blocking thread reads and hashes the file in 1 MiB buffers and
  hands chunks to the stream through a 4-slot channel.
* Receiver: the stream hands chunks to a blocking writer thread through an
  8-slot channel; the writer hashes while writing.

## Where to read next

* [`PROTOCOL.md`](PROTOCOL.md): every byte on the wire, with test vectors.
* [`THREAT_MODEL.md`](THREAT_MODEL.md): threats, controls and the tests behind them.
* [`PROGRESS.md`](PROGRESS.md): what is built and what is not.
* [`DECISIONS.md`](DECISIONS.md): choices made, and the ones still open.
* [`DEVELOPMENT.md`](DEVELOPMENT.md): building and testing.
* [`SELF_HOSTING.md`](SELF_HOSTING.md): running your own servers.
