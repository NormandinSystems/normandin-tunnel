# Hosting your own servers

**You do not need any of this to use Tunnel.** Out of the box, devices pair
and connect through the public relays of the iroh project and no server of
yours is involved. Host your own when you want to depend on nobody else, when
you expect many users at once, or for a network with no internet access.

There are two small services: a **relay** that carries the pairing messages
and forwards sealed packets when two devices cannot reach each other directly,
and an optional **rendezvous server** that hands out code numbers and passes
the pairing messages itself. Neither ever sees code words, keys or file
contents.

* **Relay only** is enough: set the relay address on both devices and they
  pair through it.
* **Relay and rendezvous server**: codes get short server-assigned numbers
  that cannot collide, and strangers cannot probe for codes in use.

> The files in `deploy/` are a starting point. They have not yet been run on
> a real server. Check each step when you first bring them up, and check the
> relay settings against the iroh-relay documentation for the pinned version.

## What you need

* One Linux server with a public IPv4 address, 2 vCPU and 2–4 GB of RAM, from
  a provider with generous monthly bandwidth: relayed transfers pass through it.
* A domain you control, with `rendezvous.<domain>` and `relay.<domain>`
  pointing at the server.
* Docker with the Compose plugin.
* Ports 80 and 443 open (TCP), and 443 UDP.

Static-site hosts cannot run these services: they need long-lived
connections, so they need a real server.

## Bring it up

```sh
cd deploy
cp .env.example .env      # set DOMAIN=your-domain.com
docker compose up -d --build
curl https://rendezvous.your-domain.com/healthz    # -> ok
```

## Point the apps at it

Desktop app: Settings > Advanced > "Use my own servers".

* Pairing server address: `wss://rendezvous.your-domain.com/v1`
* Relay addresses: `https://relay.your-domain.com`

Command line: `--rendezvous wss://rendezvous.your-domain.com/v1 --relay https://relay.your-domain.com`
(or the `TUNNEL_RENDEZVOUS` environment variable).

Both devices must use the same rendezvous server if one is set. Without one,
both devices must share at least one relay, because that is where they meet.

To make your servers the built-in default, change `Settings::default()` in
`crates/tunnel-core/src/settings.rs`.

## A pairing server on your own network

For a network with no internet access, on one machine or one LAN. The engine
is built for this (it does not wait for a relay it cannot reach), but it has
not yet been tried on a real offline network:

```sh
cargo run -p tunnel-rendezvous -- --listen 0.0.0.0:8080
# on each device:
tunnel --rendezvous ws://192.168.1.10:8080/v1 send ./some-folder
```

Plain `ws://` is accepted only for loopback and private-network addresses.

## What the rendezvous server keeps

Nothing on disk. In memory: rooms (for at most 10 minutes), and IP addresses
in the rate-limit table (forgotten after 10 minutes). Its log holds counts
and error totals only. The bundled Caddy configuration writes no access log.

## Keeping the relay in step

Keep the relay on the same iroh version as the app (`iroh = "=1.5.0"` in the
workspace `Cargo.toml`, `n0computer/iroh-relay:v1.5.0` in the compose file).
Upgrade both together.

The simple setup here runs the relay behind Caddy without QUIC address
discovery. Devices still connect and hole-punch; discovery of public
addresses is somewhat less precise. To enable it the relay needs its own
certificate and port: see the comment in `deploy/iroh-relay.toml`.
