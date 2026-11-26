//! `tunnel-rendezvous`: the pairing server.
//!
//! Run it behind a reverse proxy that terminates TLS (see `deploy/`), or
//! directly on a private network for testing.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use tunnel_rendezvous::{Config, Rendezvous};

/// Normandin Tunnel rendezvous server. Keeps everything in memory.
#[derive(Debug, Parser)]
#[command(name = "tunnel-rendezvous", version)]
struct Args {
    /// Address to listen on.
    #[arg(long, env = "TUNNEL_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    /// Take client addresses from X-Forwarded-For. Only behind your own proxy.
    #[arg(long, env = "TUNNEL_TRUST_FORWARDED_FOR")]
    trust_forwarded_for: bool,

    /// Seconds between status lines in the log (0 turns them off).
    #[arg(long, env = "TUNNEL_STATS_INTERVAL", default_value_t = 300)]
    stats_interval: u64,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let config = Config {
        trust_forwarded_for: args.trust_forwarded_for,
        ..Config::default()
    };
    let server = Rendezvous::new(config);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %listener.local_addr()?, "rendezvous server ready");

    if args.stats_interval > 0 {
        let server = server.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(args.stats_interval));
            tick.tick().await;
            loop {
                tick.tick().await;
                // Counts and error totals only: no addresses, no message contents.
                let s = server.stats();
                tracing::info!(
                    rooms = s.rooms,
                    connections = s.connections,
                    created = s.created,
                    joined = s.joined,
                    errors = s.errors,
                    "status"
                );
            }
        });
    }

    tokio::select! {
        result = server.serve(listener) => result,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
            Ok(())
        }
    }
}
