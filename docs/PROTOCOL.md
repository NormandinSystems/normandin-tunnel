# Normandin Tunnel protocol, version 1

This is the byte-exact description of what two devices exchange, and of the
optional rendezvous server. The code in `crates/tunnel-crypto` and
`crates/tunnel-proto` is the reference; the test vectors at the end are
checked by `cargo test -p tunnel-crypto`.

New here? [HOW_IT_WORKS.md](HOW_IT_WORKS.md) explains the same thing in plain
language.

The cipher suite is fixed for version 1: SPAKE2 on the Ed25519 group,
HKDF-SHA256, HMAC-SHA256, XChaCha20-Poly1305, BLAKE3, and TLS 1.3 inside iroh's
QUIC. Changing any of it means a new protocol version.

## 1. Connection code

`<nameplate>-<word>-<word>-<word>`, for example `4821-orbit-velvet-candle`.

* **Nameplate**: decimal 1 to 9999. Public. It only says where the two
  devices meet. With no pairing server (the default) the device showing the
  code picks four random digits, 1000 to 9999 (section 3). A rendezvous server
  assigns it instead (section 3a).
* **Words**: three words drawn uniformly (rejection sampling on 16 bits from
  the OS generator) from EFF Short Wordlist #2: 1,296 words, each with a unique
  three-letter prefix. About 31 bits. The list is embedded and never changes.
* **Typing** is forgiving: any case; any run of non-alphanumeric characters
  separates parts; a word may be shortened to its first three or more letters.
  The one hyphenated word, `yo-yo`, may be typed `yo-yo`, `yoyo` or `yo yo`.
* **QR form**: `tunnel://pair?c=<code>`, plus `&s=<rendezvous host>` when a
  rendezvous server was used. If the host differs from the one in settings,
  the app asks before using it.
* A code works once and expires after 10 minutes.

## 2. Pairing

Roles: **A** chose "Show a code", **B** chose "Enter a code".

The two devices need somewhere to pass the messages below to each other. By
default that is a meeting point on the relay network (section 3). With a
rendezvous server configured it is a room on that server (section 3a). The
steps and their guarantees are the same either way, and neither place is
trusted.

1. A has a nameplate (four digits it picks itself, or a number the rendezvous
   server assigns) and picks the three words. It starts SPAKE2 as side A and
   makes its SPAKE2 message available at the meeting place.
2. B arrives with the code, reads A's message, starts SPAKE2 as side B and
   answers with its own message.

SPAKE2 (RustCrypto `spake2` 0.4, `Ed25519Group`):

| Input | Value |
|---|---|
| password | `normandin-tunnel/v1\|<nameplate>\|<w1>-<w2>-<w3>` (ASCII, canonical spelling, nameplate without leading zeros) |
| identity A | `tunnel-v1-A` |
| identity B | `tunnel-v1-B` |

Each side's SPAKE2 message is 33 bytes (side byte, then the point). The output
is the 32-byte key **K**.

3. Transcript hash, with the nameplate and both lengths as big-endian u16:

   `T = SHA-256("tunnel-v1-transcript" ‖ nameplate ‖ len(A msg) ‖ A msg ‖ len(B msg) ‖ B msg)`

4. Key schedule: HKDF-SHA256, input key material K, salt T. One 32-byte expand
   per label, each label prefixed `tunnel v1 `:

   `confirm A`, `confirm B`, `mailbox A->B`, `mailbox B->A`, `bind`, `safety`, `trust`

5. Key confirmation: each side sends `HMAC-SHA256(own confirm key, "confirm" ‖ T)`
   and checks the other's in constant time. A mismatch, or a confirmation
   that never arrives, ends the pairing: the code is dead.
6. Sealed hello: XChaCha20-Poly1305 under the sender's mailbox key.
   Nonce: 16 zero bytes, then a per-direction counter as big-endian u64
   (starting at 0). Associated data: `T ‖ sender role byte` (`A` or `B`).
   The plaintext is the `Hello` structure (section 4), `postcard`-encoded.
7. Both leave the meeting place: the meeting point goes off the network, or
   the server deletes the room. K, the confirm keys and the mailbox keys are
   wiped.
8. B dials A's endpoint ID over iroh with ALPN `tunnel/control/1`, using the
   addresses from the hello. A accepts a connection only from the endpoint ID
   it was told.
9. Channel binding. B opens the control stream. The first frame each side
   sends is `Bind { mac }`:

   `mac = HMAC-SHA256(bind key, "bind" ‖ sender role byte ‖ A's endpoint ID ‖ B's endpoint ID ‖ T)`

   A wrong value closes the connection.
10. Safety code: the first 8 bytes of
    `HMAC-SHA256(safety key, "safety" ‖ A's endpoint ID ‖ B's endpoint ID)`
    as a big-endian u64, modulo 1,000,000, shown as six digits (`418 293`).
    Each side sends `SafetyConfirmed` when its person taps "Codes match". The
    session opens when both have. Until then every other message is a protocol
    error.

Pairing messages are a `postcard`-encoded enum
`PairingMessage { Pake(bytes), Confirm([u8;32]), Hello(bytes) }`. At a meeting
point each one is a length-prefixed frame on a stream (section 3). In a
rendezvous room each one is the body of a `post`, base64-encoded (section 3a).

## 3. Pairing without a server (the default)

The pairing messages of section 2 need somewhere to pass between two devices
that know nothing about each other but the code. By default that place is a
**meeting point** on the relay network, and no server of ours is involved.

**The meeting point.** Its endpoint key is computed from the nameplate alone:

```
key = HKDF-SHA256(salt = "normandin-tunnel/v1/meeting-point",
                  ikm  = nameplate as decimal text,
                  info = "ed25519 secret key")        32 bytes, an Ed25519 secret key
```

The words are not an input. The key is therefore public knowledge for anyone
who knows or guesses the four digits, and it is treated that way: it is an
address, not a credential.

**A, showing the code:**

1. Picks the nameplate and three words, and computes its SPAKE2 message.
2. Opens an endpoint with the meeting-point key, ALPN `tunnel/pair/1`, on the
   relays in use, and waits until one relay is connected. Only then is the code
   shown. If no relay can be reached within 15 seconds the app says so and no
   code is shown.
3. For every device that connects: opens one stream and sends its SPAKE2
   message. At most 8 devices may be waiting; more are closed with `busy`.
   A device that sends nothing valid within 20 seconds is dropped.
4. The **first** device to send a well-formed pairing message is the one
   attempt this code gets. Every other device, waiting or arriving later, is
   closed with `in use`. From here the steps of section 2 run on that stream.
5. Closes the meeting point when pairing ends, either way, and when the code
   expires or is cancelled.

**B, typing the code:**

1. Computes the same key, and from it the meeting point's endpoint ID.
2. Dials that ID through every relay it is configured with. It does not know
   which relay A is on; the one A is connected to delivers the first packet.
3. Waits up to 12 seconds. No answer while B itself has a relay connection is
   reported as "No device is showing that code"; no relay connection as
   "Can't reach the pairing network".
4. Reads A's SPAKE2 message from the stream A opens, answers with its own, and
   continues with section 2.

Frames on the stream are the `PairingMessage`s of section 2 in the framing of
section 5: a 4-byte big-endian length, then at most 4 KiB of postcard. Close
codes: 0 done, 1 in use, 2 busy, 3 too slow. Before closing, each side
finishes its stream and waits (up to 5 seconds) for the other to have received
everything, because closing a QUIC connection discards undelivered data.

**What this place is trusted with: nothing.** Anyone can dial the meeting
point, and anyone can open an endpoint with the same key and answer in A's
place. That is the same position a hostile rendezvous server is in, and the
same things stop it:

* SPAKE2 messages are independent of the words. A stranger who is greeted
  learns nothing from the greeting.
* A answers one device, once (`PakeState::finish` consumes the state). A wrong
  answer, or one that is never confirmed, ends the code. That is one guess in
  about 2.2 billion per code.
* An impostor answering B gets B's SPAKE2 message and confirmation, which
  check exactly one guess, and never the sealed hello.
* The session does not run on this connection. It runs between two other
  endpoints with fresh random keys, named in the sealed hellos and tied to the
  pairing by the `Bind` tags (section 2, step 9).

What a stranger **can** do is spoil a code: connect first and answer with a
wrong guess, so that A has to make a new one. They need the four digits at the
moment the code is on screen. See `THREAT_MODEL.md`, T14.

Relays: the default ones are the public relays run by the iroh project. With "Use my own servers" and relay addresses set,
the meeting point lives on those relays instead and nothing else is needed.
A relay sees the two IP addresses, the meeting point's ID (so, in effect, the
nameplate), and encrypted packets.

## 3a. Rendezvous server (optional)

Used instead of section 3 when a pairing server address is set. Both devices
must use the same one.

WebSocket at `wss://<host>/v1`, plus `GET /healthz`. One JSON text message
per frame, at most 8 KiB from a client.

| Client message | Meaning |
|---|---|
| `{"type":"create"}` | Open a room |
| `{"type":"join","nameplate":42}` | Join a room |
| `{"type":"post","body":"<base64>"}` | Pass one message (at most 4 KiB before base64) |
| `{"type":"close"}` | Leave; the room closes |

| Server message | Meaning |
|---|---|
| `{"type":"created","nameplate":42,"expires_in":600}` | The room exists |
| `{"type":"joined"}` | You are in |
| `{"type":"peer_joined"}` | The other device arrived |
| `{"type":"message","body":"<base64>"}` | A message from the other device |
| `{"type":"peer_left"}` | The other device left; the room is closed |
| `{"type":"error","code":"..."}` | `unknown_code`, `code_in_use`, `expired`, `rate_limited`, `too_large`, `bad_request`, `server_busy` |

Rooms hold 2 members and 16 messages. Messages posted before the second
device arrives are queued. A room closes when either side leaves or
disconnects, or after 10 minutes. A freed nameplate rests for 10 minutes.
Nameplates come from 1–999 while fewer than half of those are in use,
otherwise from 1000–9999. Per IP address: 20 creates and 20 joins per minute,
10 open connections. 10,000 rooms at most. Nothing is written to disk.

## 4. Hello

`postcard`-encoded struct, in this field order:

| Field | Type | Limit |
|---|---|---|
| versions | list of u16 | 1 to 16 |
| endpoint_id | 32 bytes | this session's fresh public key |
| relay_url | optional string | 255 bytes |
| direct_addrs | list of socket addresses | 32 |
| device_name | string | 64 characters |
| device_type | enum: Desktop, Phone, Tablet, Server | |
| app_version | string | 32 bytes |
| capabilities | u64 bit set | none defined in version 1 |

Both sides use the highest protocol version they share. No shared version:
"The other device needs an update."

## 5. Control stream

Each message is a 4-byte big-endian length, then a `postcard`-encoded enum.
Frames are capped at 1 MiB; the length is checked before anything is
allocated. An unknown message type, trailing bytes or a field over its limit
closes the session.

Variants, in wire order (the variant index is the first byte):

| # | Message | Direction | Purpose |
|---|---|---|---|
| 0 | `Bind { mac }` | both | Channel binding |
| 1 | `SafetyConfirmed` | both | The person tapped "Codes match" |
| 2 | `Offer { transfer_id, file_count, folder_count, total_bytes, manifest_pages }` | sender → receiver | Announces a transfer |
| 3 | `ManifestPage { transfer_id, page, entries }` | sender → receiver | Up to 1 MiB of entries |
| 4 | `Accept { transfer_id }` | receiver → sender | The destination stays private |
| 5 | `Decline { transfer_id, reason }` | receiver → sender | Declined, Invalid, Busy, NoSpace |
| 6 | `Resume { transfer_id, files }` | receiver → sender | After a reconnect: partial offsets |
| 7 | `Pause { transfer_id, reason }` | either | |
| 8 | `Unpause { transfer_id }` | either | |
| 9 | `Cancel { transfer_id, reason }` | either | |
| 10 | `Completed { transfer_id, received, skipped, failed }` | receiver → sender | Final tally |
| 11 | `Text { text }` | either | UTF-8, at most 64 KiB |
| 12 | `TrustOffer { device_key }` | both | Reserved for trusted devices |
| 13 | `TrustAccept { device_key }` | both | Reserved for trusted devices |
| 14 | `Bye` | either | Clean close |
| 15 | `Error { code, message }` | either | Coded error; the session closes |

A manifest entry is `{ file_id: u32, kind: File|Folder, path: [string], size: u64, modified_ms: i64, executable: bool }`.
File IDs count from 0 with no gaps across pages. A folder is listed before
anything inside it. Paths are lists of name components, never strings with
separators. Hashes are not in the manifest.

The receiver validates every entry (see `docs/THREAT_MODEL.md`), computes its
own totals, and rejects the offer if they differ from the `Offer`.

## 6. Data streams

For each file the receiver opens a bidirectional stream and sends one frame
`Get { transfer_id, file_id, offset }` (same framing, capped at 1 KiB). The
sender answers with one `DataHeader` frame:

* `Ok { size, offset }`, followed by the bytes from `offset` to the end, then
  a 32-byte trailer: the BLAKE3 hash of the whole file. Nothing may follow.
* `Refused(reason)`: NotAccepted, UnknownFile, BadOffset, Unreadable, Changed, Paused.

The sender answers only for files of a transfer the receiver accepted. When
the offset is above zero the sender reads the skipped part only to hash it.
If the file's size or modified time differs from the manifest, before or
after reading, the file is refused or the stream is reset.

Stream reset codes: 1 changed, 2 unreadable, 3 paused, 4 cancelled,
5 protocol error, 6 write failed. Connection close codes: 0 bye, 1 protocol
error, 2 bind failed, 3 safety mismatch, 4 unexpected peer.

The receiver downloads up to 4 files of 8 MiB or more at once and up to 32
smaller ones. It writes each to `.tunnel-partial/<transfer id>/<file id>.part`
inside the destination, hashing as it writes; compares with the trailer;
flushes to disk; then moves the file into place. A hash mismatch deletes the
partial file and retries once.

## 7. Reconnecting

If the connection drops, B redials A for up to 2 minutes. Both repeat `Bind`
with the session's bind key. Each side then restates where every transfer
stands (`Resume`, `Pause`, `Completed`, `Decline` or `Cancel`), because
messages in flight when the connection dropped may not have arrived. The
receiver re-hashes each partial file to rebuild its hash state and asks for
the rest from that offset.

## 8. Limits

| Item | Limit |
|---|---|
| Control frame | 1 MiB |
| Manifest page | 1 MiB |
| Entries per offer | 1,000,000 (setting, up to 10,000,000) |
| Manifest storage per offer | 512 MiB |
| Single file | 1 TiB |
| Path depth | 64 levels |
| Text message | 64 KiB |
| Offers waiting for a decision | 4 per session |
| Transfers running at once | 8 per session |
| Pairing room lifetime | 10 minutes |
| Hello after confirmation | 30 seconds |
| Dialing the peer | 60 seconds |
| Bind frame | 15 seconds |
| Reconnect window | 2 minutes |

## 9. Test vectors

Inputs: K = 32 bytes of `0x11`; nameplate 42; A's SPAKE2 message = `"A"`
followed by bytes 0..31; B's = `"B"` followed by bytes 32..63; A's endpoint
ID = 32 bytes of `0xA1`; B's = 32 bytes of `0xB2`.

| Value | Hex |
|---|---|
| T | `e98af4bfe4fc7ff88cec7d2e94296e9d11dacb24f1a4f35f551c7a9ed88a2b06` |
| A's confirmation tag | `61bb2c57834c076fa335c6de88eb0885a16ef62c10f711341ea2578c1a3c9426` |
| B's confirmation tag | `7430c6e5a97a304774ea1c0943cdfa13a4387b62327078418d13f255aee8ceba` |
| Bind tag sent by A | `8d8a190ed487df6f9d8d44916451e4a7cc59855a312cc8428f7c823fbc407178` |
| Bind tag sent by B | `53c18e7d89b576be96717249cf935114537b5f80dcf4b395f811004a9d1c313e` |
| trust key | `ff609908b4c92f347238c3bc26cdd199a3f078f8dde6011ab0a9ec337e3e646a` |
| Safety code | `130819` |
| Meeting-point key, nameplate 42 | `3a44a754c6b40d0437ba2e81f232133f86f4cc007a1653baa00bbbed1c9cafde` |
| `"hello"` sealed by A (counter 0) | `39a20dda2e94376d0202b5831fe0af59ab8828593a` |

They were produced by an independent implementation,
[`docs/test_vectors.py`](test_vectors.py) (Python standard library, plus PyNaCl
for the sealed hello), and are compared against the Rust implementation by
the tests in `crates/tunnel-crypto/src/pairing.rs`.
