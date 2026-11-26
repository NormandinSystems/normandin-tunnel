# Benchmarks

Results are added per release. Run the large tests with:

```sh
cargo test --release -p tunnel-tests --test integration -- --ignored --nocapture
```

They cover requirement R2 (one large file) and R4 (100,000 small files) over
loopback, with relays off.

| Version | Machine | 5 GiB sparse file | 100,000 files of 10 KB |
|---|---|---|---|
| 0.1.0 | Linux cloud VM, 2 cores, 7 GB RAM, both ends on the same machine | 16.7 s (322 MB/s) | 38.1 s |

Both ends share two cores here, so these are floor figures for the engine
(encryption, hashing twice, disk writes), not network figures. No run over a
real LAN, the internet or a relay has been measured yet.
