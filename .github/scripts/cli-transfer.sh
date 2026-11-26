#!/usr/bin/env bash
# Sends a folder from one copy of the command-line program to another on the
# same machine, with default settings (so through the public relays), and
# checks that what arrived is identical.
#
# Usage: cli-transfer.sh <path-to-tunnel> [extra tunnel options...]
set -u
T="$1"; shift
work="$(mktemp -d)"; cd "$work"
mkdir -p src/inner dst
head -c 3000000 /dev/urandom > src/sample.bin
printf 'bonjour\n' > "src/inner/été.txt"
export TUNNEL_LOOPBACK=1
"$T" "$@" --data-dir "$work/a" --name Sender --json --yes send src > send.jsonl 2> send.err &
sender=$!
code=""
for _ in $(seq 60); do
  code=$(grep -o '"code":"[^"]*"' send.jsonl 2>/dev/null | head -1 | cut -d'"' -f4)
  [ -n "$code" ] && break
  kill -0 "$sender" 2>/dev/null || break
  sleep 1
done
if [ -z "$code" ]; then echo "No code was shown"; cat send.err; exit 1; fi
echo "A code was shown."
"$T" "$@" --data-dir "$work/b" --name Receiver --json --yes receive "$code" --to dst --once > recv.jsonl 2> recv.err
rc=$?
wait "$sender"; sc=$?
echo "receive exit $rc, send exit $sc"
if diff -r src dst/src; then echo "IDENTICAL"; else echo "DIFFERENT"; cat recv.err send.err; exit 1; fi
[ "$rc" -eq 0 ] && [ "$sc" -eq 0 ]
