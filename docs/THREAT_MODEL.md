# Threat model

The network, the relays, the place where two devices meet to pair (a meeting
point on the relay network, or a rendezvous server) are all assumed hostile. Only
the two devices and the people holding them are trusted, and even then the
receiver treats every incoming offer as untrusted until a person accepts it
and each file passes its checks.

**What is protected:** file contents and names, the receiver's file system,
session keys, and who-talks-to-whom metadata.

## Threats and controls

Each row names where the control lives and the test that exercises it.

| # | Threat | Control | Tested by |
|---|---|---|---|
| T1 | Network eavesdropper | QUIC with TLS 1.3 end to end (iroh); SPAKE2 reveals nothing about the code | `tunnel-crypto` pairing tests |
| T2 | Hostile rendezvous server, or an impostor at the meeting point | Never sees the words (the meeting point's key comes from the public number only); one guess per code; key confirmation; sealed hello; channel binding | `tampered_pairing_is_caught`, `a_wrong_code_gets_exactly_one_guess`, `an_impostor_at_the_meeting_point_cannot_open_a_session`, `meeting_key_depends_only_on_the_nameplate` |
| T3 | Hostile relay | Packets are encrypted to the peer's key by QUIC; the relay cannot read or alter them | iroh's guarantee; the session only accepts the endpoint ID named in the sealed hello |
| T4 | Someone sees the code and joins first | Code works once; safety code and device name on both screens; "code already used" for the real friend | `a_third_device_is_told…`, rendezvous `a_third_device_is_refused…` |
| T5 | Code guessing | One guess per code: the device showing the code answers one device, once, and the code dies on any failed or missing confirmation | `a_wrong_code_fails_on_both_sides_and_kills_the_code`, `wrong_words_fail_on_both_sides_and_use_up_the_code`, `a_stranger_gets_one_guess_and_cannot_stall_to_get_more`, rendezvous rate-limit tests |
| T6 | Harmful paths from a peer | Path-safety rules below | `pathsafe` table and property tests; `traversal_and_separator_names…`; `reserved_and_deceptive_names…` |
| T7 | Malware from a peer | Receiver must accept; executable and double-extension warnings; downloaded-file mark; nothing is ever opened or run | `offer_shows_names_and_warnings…` |
| T8 | Resource exhaustion | Frame, entry, manifest and offer caps; free-space check; timeouts; bounded queues; manifests on disk, not in memory | `ten_million_entries_and_a_petabyte…`, `oversized_and_garbage_frames…`, `more_than_four_waiting_offers…` |
| T9 | Parser bugs | Rust; `unsafe` forbidden in our crates; size checks before allocating; property tests; fuzz targets | `decoder_never_panics` tests; `fuzz/` |
| T10 | Lost or stolen device | No long-lived secrets exist yet: every session uses a fresh key and a fresh code | — (trusted devices are not implemented) |
| T11 | Supply-chain attack | Locked dependencies; cargo-deny; pinned iroh; GitHub Actions pinned by commit hash | CI `supply-chain` job |
| T12 | Fake update | Not applicable yet: the app does not update itself | — |
| T13 | Session tracking | Fresh endpoint key per session; nothing published to any lookup service; a meeting point exists only while its code is on screen; the server keeps no logs of contents and IPs only in memory for rate limits | `cancelling_takes_the_code_off_the_network`, rendezvous `addresses_are_forgotten_after_ten_minutes` |
| T14 | A stranger spoils codes (denial of service) | Not prevented, only limited. Anyone who knows the four digits while a code is on screen can answer first with a wrong guess; the code then fails closed and a new one must be made. Silent or malformed connections do not use up the code; at most 8 may wait and each is dropped after 20 seconds | `a_silent_stranger_does_not_keep_the_real_device_out`, `garbage_from_a_stranger_does_not_use_up_the_code` |

## Path-safety rules (receiver side, never relaxed)

Implemented in `crates/tunnel-core/src/pathsafe.rs` and applied to every
manifest entry in `Store::insert_incoming_page`.

1. Paths arrive as lists of name components, never as strings with separators.
2. The whole offer is rejected if any component is empty, `.` or `..`, or
   contains `/` or NUL. A well-behaved sender can never produce these.
3. A component that is legal on the sender but unsafe here is renamed and
   listed in the transfer details: it contains `\ : < > " | ? *` or control
   characters; it contains bidirectional-text controls; it ends with a space
   or dot; it is a Windows device name in any case (CON, PRN, AUX, NUL,
   COM1–COM9, LPT1–LPT9, with or without an extension); or it is over 255
   bytes. Each bad character becomes `_`, device names get a `_`, long names
   are shortened keeping the extension. These Windows-safe rules apply on
   every platform, because files often move to Windows later.
4. At most 64 levels deep. On Windows, extended-length paths are used and a
   path over 32,000 characters is refused.
5. Names are normalized to Unicode NFC. Names that collide when case is
   ignored get ` (2)`.
6. The joined path must be inside the destination, as text and after resolving
   the folder's real path.
7. Folders are created one level at a time. Anything on the way that is a
   symbolic link, junction or reparse point is refused, never entered.
8. Nothing is overwritten silently. The conflict rule decides: keep both
   (default), replace, or skip.
9. No special permission bits are ever set. The executable bit is restored
   only on Unix, only when the sender flagged it, and only while the setting
   is on.
10. A received file is never opened, previewed, unpacked or run. The app can
    open the *folder* in the file manager when asked.

In addition, every entry must sit under a folder the sender already listed,
file IDs must count up without gaps, and the receiver's own totals must match
the offer.

## Consent

No byte touches the receiver's disk before the receiver accepts. Until
then the manifest lives in a scratch database in the app's own data folder.
Nothing is accepted, and no file request is answered, before both people have
confirmed the safety code.

## What a completed transfer guarantees

Every file outside `.tunnel-partial` is complete and matched the BLAKE3 hash
the sender sent at the end of that file's stream. The hash protects against
corruption and truncation. It is not a signature: the sender is whoever
passed the code and the safety-code check.

## Known gaps

* Pairing without a server depends on relays this project does not run (by
  default the iroh project's public ones). If they are down or blocked, no
  code can be made until own relays or an own pairing server are set. They see
  both IP addresses and which four-digit number is in use, never the words.
* Those public relays are, in their operators' words, for development, testing
  and hobby projects: shared, rate-limited, with no uptime promise and no
  promise to keep working with older iroh versions. This app pins one iroh
  version, so a future change on their side could stop an installed copy from
  pairing until it is updated or pointed at other relays. Anything beyond
  occasional use should run its own relays (`SELF_HOSTING.md`).
* The four-digit number has 9,000 values. Two devices that pick the same one
  within the same ten minutes collide and both codes fail; with few users this
  is rare, with thousands at once a pairing server (which hands numbers out)
  is the answer. The same small range is what makes T14 cheap for an attacker
  who is willing to probe all of them.
* Two devices on a network with no internet access cannot pair without a
  pairing server on that network: there is no local discovery yet.

* No outside security review has been done. This is the main reason the
  release is called a preview.
* The fuzz targets run every night in CI, ten minutes each. The first run
  found nothing, but they have not been run for hours on end.
* macOS quarantine attributes are not set (macOS is not built yet).
* The check-then-create steps of rule 7 are not atomic. A second local
  program that swaps a folder for a link at exactly the right moment is
  outside this threat model; a remote peer cannot do it.
* Builds are not code-signed.
