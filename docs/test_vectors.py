#!/usr/bin/env python3
"""Independent reference for the Normandin Tunnel v1 key schedule.

Uses only hashlib/hmac from the standard library, plus PyNaCl for the one
XChaCha20-Poly1305 vector. Run it to regenerate the vectors checked by
`cargo test -p tunnel-crypto` and printed in docs/PROTOCOL.md:

    python3 docs/test_vectors.py [output-directory]
"""
import hashlib
import hmac
import sys
from pathlib import Path

SHARED = bytes([0x11]) * 32          # K, the SPAKE2 output
NAMEPLATE = 42
MSG_A = b"A" + bytes(range(0, 32))   # A's SPAKE2 message (33 bytes)
MSG_B = b"B" + bytes(range(32, 64))  # B's SPAKE2 message (33 bytes)
ID_A = bytes([0xA1]) * 32            # A's endpoint ID
ID_B = bytes([0xB2]) * 32            # B's endpoint ID


def mac(key: bytes, *parts: bytes) -> bytes:
    return hmac.new(key, b"".join(parts), hashlib.sha256).digest()


def u16(n: int) -> bytes:
    return n.to_bytes(2, "big")


transcript = hashlib.sha256(
    b"tunnel-v1-transcript" + u16(NAMEPLATE) + u16(len(MSG_A)) + MSG_A + u16(len(MSG_B)) + MSG_B
).digest()

# HKDF-SHA256: extract with salt = T, then one 32-byte expand per label.
prk = mac(transcript, SHARED)


def key(label: str) -> bytes:
    return mac(prk, ("tunnel v1 " + label).encode(), b"\x01")


keys = {label: key(label) for label in
        ["confirm A", "confirm B", "mailbox A->B", "mailbox B->A", "bind", "safety", "trust"]}

out = {
    "transcript.hex": transcript.hex(),
    "confirm_a.hex": mac(keys["confirm A"], b"confirm", transcript).hex(),
    "confirm_b.hex": mac(keys["confirm B"], b"confirm", transcript).hex(),
    "bind_a.hex": mac(keys["bind"], b"bind", b"A", ID_A, ID_B, transcript).hex(),
    "bind_b.hex": mac(keys["bind"], b"bind", b"B", ID_A, ID_B, transcript).hex(),
    "safety.txt": "%06d" % (int.from_bytes(mac(keys["safety"], b"safety", ID_A, ID_B)[:8], "big") % 1_000_000),
    "trust.hex": keys["trust"].hex(),
}

# Meeting-point key for nameplate 42 (pairing without a rendezvous server):
# HKDF-SHA256 with a fixed salt over the decimal nameplate, one 32-byte block.
meeting_prk = hmac.new(b"normandin-tunnel/v1/meeting-point", b"42", hashlib.sha256).digest()
out["meeting_key_42.hex"] = hmac.new(meeting_prk, b"ed25519 secret key" + b"\x01", hashlib.sha256).hexdigest()

try:
    from nacl.bindings import crypto_aead_xchacha20poly1305_ietf_encrypt as seal
    nonce = bytes(16) + (0).to_bytes(8, "big")
    out["sealed_hello_a.hex"] = seal(b"hello", transcript + b"A", nonce, keys["mailbox A->B"]).hex()
except ImportError:
    print("PyNaCl not installed: skipping sealed_hello_a.hex", file=sys.stderr)

for name, value in out.items():
    print(f"{name:20} {value}")
if len(sys.argv) > 1:
    directory = Path(sys.argv[1])
    for name, value in out.items():
        (directory / name).write_text(value + "\n")
