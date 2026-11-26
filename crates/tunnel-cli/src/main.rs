//! `tunnel`: Normandin Tunnel on the command line.
//!
//! The same engine as the desktop app, for servers, NAS boxes, Raspberry Pi and
//! scripts.
//!
//! ```text
//! tunnel send <paths…>          show a code, then send files and folders
//! tunnel receive <code>         enter a code, then receive
//! ```
//!
//! Exit codes: 0 everything arrived, 1 the session failed, 2 the transfer was
//! declined, cancelled or incomplete, 130 interrupted.

#![forbid(unsafe_code)]

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use tokio::sync::broadcast;
use tunnel_core::{
    Config, DeviceType, Event, OfferView, OfferWarning, PathKind, RelayChoice, SessionState,
    TransferState, TransferView, Tunnel,
};

/// Send files, folders and text between two devices with a one-time code.
#[derive(Debug, Parser)]
#[command(name = "tunnel", version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: Common,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Args)]
struct Common {
    /// Print one JSON object per event on standard output instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Answer yes to every question: the safety code and incoming offers.
    /// Only use this when you control both devices.
    #[arg(long, global = true)]
    yes: bool,

    /// Your own pairing server (overrides the saved setting). Without one,
    /// devices meet on the relay network and no server is needed.
    #[arg(long, global = true, value_name = "URL", env = "TUNNEL_RENDEZVOUS")]
    rendezvous: Option<String>,

    /// Relay server to use (repeatable; overrides the saved setting).
    #[arg(long, global = true, value_name = "URL")]
    relay: Vec<String>,

    /// Never use a relay: direct connections only.
    #[arg(long, global = true, conflicts_with = "relay")]
    no_relay: bool,

    /// Name shown to the other device (overrides the saved setting).
    #[arg(long, global = true, value_name = "NAME")]
    name: Option<String>,

    /// Folder for settings and history.
    #[arg(long, global = true, value_name = "DIR", env = "TUNNEL_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show a code, wait for the other device, then send.
    Send {
        /// Files and folders to send.
        paths: Vec<PathBuf>,
        /// Also send this text message.
        #[arg(long, value_name = "TEXT")]
        text: Option<String>,
    },
    /// Enter a code from the other device, then receive what it sends.
    Receive {
        /// The code, e.g. 4821-orbit-velvet-candle (or a tunnel:// link).
        code: String,
        /// Folder to save into.
        #[arg(long, value_name = "DIR", default_value = ".")]
        to: PathBuf,
        /// Stop after the first transfer instead of waiting for more.
        #[arg(long)]
        once: bool,
    },
    /// List and remove trusted devices.
    Devices,
}

const EXIT_OK: u8 = 0;
const EXIT_FAILED: u8 = 1;
const EXIT_INCOMPLETE: u8 = 2;
const EXIT_INTERRUPTED: u8 = 130;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TUNNEL_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .init();
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("tunnel: {error:#}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

struct Ui {
    json: bool,
    yes: bool,
    live: bool,
}

impl Ui {
    fn say(&self, text: impl AsRef<str>) {
        if !self.json {
            self.clear_progress();
            println!("{}", text.as_ref());
        }
    }

    fn event(&self, event: &Event) {
        if self.json
            && let Ok(line) = serde_json::to_string(event)
        {
            println!("{line}");
        }
    }

    fn clear_progress(&self) {
        if self.live {
            eprint!("\r\x1b[2K");
        }
    }

    fn progress(&self, transfer: &TransferView) {
        if self.json || !self.live {
            return;
        }
        let percent = if transfer.bytes_total == 0 {
            100.0
        } else {
            transfer.bytes_done as f64 * 100.0 / transfer.bytes_total as f64
        };
        let eta = transfer
            .eta_secs
            .map(|s| format!(", {} left", duration(s)))
            .unwrap_or_default();
        eprint!(
            "\r\x1b[2K  {percent:5.1}%  {} of {}  {}/s{eta}",
            size(transfer.bytes_done),
            size(transfer.bytes_total),
            size(transfer.speed),
        );
        let _ = std::io::stderr().flush();
    }

    /// Asks a yes/no question. `--yes` answers for the person.
    async fn confirm(&self, question: &str) -> Result<bool> {
        if self.yes {
            return Ok(true);
        }
        self.clear_progress();
        eprint!("{question} [y/N] ");
        let _ = std::io::stderr().flush();
        let answer = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|_| line)
        })
        .await?
        .context("could not read the answer")?;
        Ok(matches!(
            answer.trim().to_lowercase().as_str(),
            "y" | "yes" | "o" | "oui"
        ))
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

fn qr(text: &str) -> Option<String> {
    use qrcode::render::unicode;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

async fn run(cli: Cli) -> Result<u8> {
    let Common {
        json,
        yes,
        rendezvous,
        relay,
        no_relay,
        name,
        data_dir,
    } = cli.common;
    if let Commands::Devices = cli.command {
        println!(
            "Trusted devices arrive with \"Remember this device\", which this version does not include yet."
        );
        println!("Every session uses a fresh one-time code.");
        return Ok(EXIT_OK);
    }

    let relay = if no_relay {
        Some(RelayChoice::Disabled)
    } else if relay.is_empty() {
        None
    } else {
        Some(RelayChoice::Custom(relay))
    };
    let mut config = Config {
        device_type: DeviceType::Server,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        relay,
        rendezvous_url: rendezvous,
        include_loopback: std::env::var_os("TUNNEL_LOOPBACK").is_some(),
        ..Config::default()
    };
    if let Some(dir) = data_dir {
        config.data_dir = dir;
    }
    let tunnel = Tunnel::open(config).context("could not open the Tunnel data folder")?;
    if let Some(name) = name {
        let mut settings = tunnel.settings();
        settings.device_name = name;
        tunnel.save_settings(settings)?;
    }
    let ui = Ui {
        json,
        yes,
        live: !json && std::io::stderr().is_terminal(),
    };
    let events = tunnel.subscribe();

    let work = async {
        match cli.command {
            Commands::Send { paths, text } => send(&tunnel, events, &ui, paths, text).await,
            Commands::Receive { code, to, once } => {
                receive(&tunnel, events, &ui, &code, to, once).await
            }
            Commands::Devices => Ok(EXIT_OK),
        }
    };
    let outcome = tokio::select! {
        outcome = work => outcome,
        _ = tokio::signal::ctrl_c() => {
            ui.clear_progress();
            Ok(EXIT_INTERRUPTED)
        }
    };
    tunnel.end_session().await;
    outcome
}

async fn next(events: &mut broadcast::Receiver<Event>, ui: &Ui) -> Result<Event> {
    loop {
        match events.recv().await {
            Ok(event) => {
                ui.event(&event);
                return Ok(event);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => bail!("the engine stopped"),
        }
    }
}

/// Handles everything up to an open session. Returns the other device's name.
async fn open_session(
    events: &mut broadcast::Receiver<Event>,
    ui: &Ui,
    tunnel: &Tunnel,
) -> Result<String> {
    loop {
        let Event::Session { session } = next(events, ui).await? else {
            continue;
        };
        match session {
            SessionState::ShowingCode { code, uri, .. } => {
                ui.say(format!("\nCode:  {code}\n"));
                if !ui.json
                    && let Some(picture) = qr(&uri)
                {
                    println!("{picture}");
                }
                ui.say(
                    "On the other device, choose \"Enter a code\" (or run: tunnel receive <code>).",
                );
                ui.say(
                    "The code works once and expires in 10 minutes. Waiting for the other device…",
                );
            }
            SessionState::Securing => ui.say("Securing the connection…"),
            SessionState::Connecting => ui.say("Connecting to the other device…"),
            SessionState::VerifySafety {
                safety_code,
                peer,
                confirmed: false,
                ..
            } => {
                ui.say(format!(
                    "\nConnected to: {} ({})",
                    peer.name, peer.device_type
                ));
                ui.say(format!("Safety code:  {safety_code}\n"));
                let question = "Does the other device show the same safety code?";
                let matches = ui.confirm(question).await?;
                tunnel.confirm_safety(matches).await?;
                if matches {
                    ui.say("Waiting for the other person to confirm…");
                }
            }
            SessionState::Active { peer, path } => {
                let via = match path {
                    PathKind::Relay => " (through a relay; transfers may be slower)",
                    _ => "",
                };
                ui.say(format!("Session open with {}{via}.", peer.name));
                return Ok(peer.name);
            }
            SessionState::Ended { error, .. } => match error {
                Some(error) => bail!("{}", error.message),
                None => bail!("The session ended before it opened."),
            },
            _ => {}
        }
    }
}

fn report(ui: &Ui, transfer: &TransferView) -> u8 {
    ui.clear_progress();
    let what = match transfer.state {
        TransferState::Done => "Done",
        TransferState::Declined => "Declined by the other device",
        TransferState::Cancelled => "Cancelled",
        _ => "Failed",
    };
    let mut line = format!(
        "{what}: {} file(s), {}",
        transfer.files_done,
        size(transfer.bytes_total)
    );
    if transfer.files_skipped > 0 {
        line.push_str(&format!(", {} skipped", transfer.files_skipped));
    }
    if transfer.files_failed > 0 {
        line.push_str(&format!(", {} failed", transfer.files_failed));
    }
    if let Some(folder) = &transfer.folder {
        line.push_str(&format!(" -> {folder}"));
    }
    ui.say(line);
    if let Some(error) = &transfer.error {
        ui.say(format!("  {}", error.message));
    }
    for issue in &transfer.issues {
        let detail = issue
            .detail
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        ui.say(format!(
            "  {}: {} [{}]{detail}",
            issue.kind, issue.path, issue.reason
        ));
    }
    if transfer.issues_truncated {
        ui.say("  … and more.");
    }
    let clean = transfer.state == TransferState::Done
        && transfer.files_failed == 0
        && transfer.files_skipped == 0;
    if clean { EXIT_OK } else { EXIT_INCOMPLETE }
}

async fn send(
    tunnel: &Tunnel,
    mut events: broadcast::Receiver<Event>,
    ui: &Ui,
    paths: Vec<PathBuf>,
    text: Option<String>,
) -> Result<u8> {
    if paths.is_empty() && text.is_none() {
        bail!("nothing to send: give at least one path or --text");
    }
    for path in &paths {
        if std::fs::symlink_metadata(path).is_err() {
            bail!("{} does not exist", path.display());
        }
    }
    tunnel.create_code().await?;
    open_session(&mut events, ui, tunnel).await?;

    if let Some(text) = text {
        tunnel.send_text(text).await?;
        ui.say("Text sent.");
    }
    if paths.is_empty() {
        // Give the message a moment to leave before the session closes.
        tokio::time::sleep(Duration::from_millis(300)).await;
        return Ok(EXIT_OK);
    }
    let id = tunnel.send_paths(paths).await?;
    ui.say("Preparing…");
    let mut announced = false;
    loop {
        match next(&mut events, ui).await? {
            Event::Transfer { transfer } if transfer.id == id => {
                if transfer.state == TransferState::Offered && !announced {
                    announced = true;
                    ui.say(format!(
                        "Offered {} file(s), {}. Waiting for the other device to accept…",
                        transfer.files_total,
                        size(transfer.bytes_total)
                    ));
                }
                if transfer.state.is_final() {
                    return Ok(report(ui, &transfer));
                }
                if transfer.state == TransferState::Active {
                    ui.progress(&transfer);
                }
            }
            // This command only sends: anything offered back is declined.
            Event::Offer { offer } => {
                let _ = tunnel.respond_offer(&offer.id, false, None).await;
            }
            Event::Text { message } if message.from_peer => {
                ui.say(format!("Message: {}", message.text))
            }
            Event::Session {
                session: SessionState::Ended { error, .. },
            } => {
                bail!(
                    "{}",
                    error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "The other device ended the session.".into())
                );
            }
            _ => {}
        }
    }
}

fn describe(offer: &OfferView) -> String {
    let mut text = format!(
        "\n{} wants to send {} file(s) in {} folder(s), {}:\n",
        offer.peer.name,
        offer.files,
        offer.folders,
        size(offer.bytes)
    );
    for name in &offer.names {
        text.push_str(&format!("  {name}\n"));
    }
    let listed = offer.names.len() as u64;
    if offer.files + offer.folders > listed {
        text.push_str(&format!(
            "  … and {} more\n",
            offer.files + offer.folders - listed
        ));
    }
    for warning in &offer.warnings {
        let line = match warning {
            OfferWarning::Executables { count } => {
                format!("{count} program(s) or script(s). Only open them if you trust the sender.")
            }
            OfferWarning::DoubleExtensions { count } => {
                format!(
                    "{count} file(s) disguise a program behind another extension (like report.pdf.exe)."
                )
            }
            OfferWarning::BidiNames { count } => {
                format!("{count} name(s) contain hidden text-direction characters.")
            }
            OfferWarning::Renamed { count } => {
                format!("{count} name(s) will be changed to be safe on this device.")
            }
            OfferWarning::LowSpace => "This will use most of the free space.".to_string(),
        };
        text.push_str(&format!("  Warning: {line}\n"));
    }
    text
}

async fn receive(
    tunnel: &Tunnel,
    mut events: broadcast::Receiver<Event>,
    ui: &Ui,
    code: &str,
    to: PathBuf,
    once: bool,
) -> Result<u8> {
    let folder = std::path::absolute(&to)
        .with_context(|| format!("{} is not a usable folder", to.display()))?;
    let check = tunnel.check_code(code)?;
    let allow_other_host = match &check.other_host {
        Some(host) => {
            let question = format!("This code uses a different pairing service ({host}). Use it?");
            if !ui.confirm(&question).await? {
                return Ok(EXIT_FAILED);
            }
            true
        }
        None => false,
    };
    tunnel.join_code(code, allow_other_host).await?;
    open_session(&mut events, ui, tunnel).await?;
    ui.say("Waiting for the other device to send something…");

    let mut worst = EXIT_OK;
    loop {
        match next(&mut events, ui).await? {
            Event::Offer { offer } => {
                ui.say(describe(&offer));
                let accept = ui
                    .confirm(&format!("Save to {}?", folder.display()))
                    .await?;
                match tunnel
                    .respond_offer(&offer.id, accept, Some(folder.clone()))
                    .await
                {
                    Ok(()) if accept => ui.say("Receiving…"),
                    Ok(()) => {
                        ui.say("Declined.");
                        worst = worst.max(EXIT_INCOMPLETE);
                        if once {
                            return Ok(worst);
                        }
                    }
                    Err(error) => {
                        ui.say(format!("Could not accept: {error}"));
                        let _ = tunnel.respond_offer(&offer.id, false, None).await;
                        worst = worst.max(EXIT_INCOMPLETE);
                    }
                }
            }
            Event::OfferBlocked { peer } => {
                ui.say(format!(
                    "Blocked an unsafe offer from {}. Nothing was saved.",
                    peer.name
                ));
                worst = worst.max(EXIT_INCOMPLETE);
            }
            Event::Transfer { transfer } => {
                if transfer.state.is_final() {
                    worst = worst.max(report(ui, &transfer));
                    if once {
                        return Ok(worst);
                    }
                    ui.say("Waiting for more… (Ctrl+C to stop)");
                } else if transfer.state == TransferState::Active {
                    ui.progress(&transfer);
                }
            }
            Event::Text { message } if message.from_peer => {
                ui.say(format!("Message: {}", message.text))
            }
            Event::Session {
                session: SessionState::Ended { error, .. },
            } => {
                ui.clear_progress();
                return match error {
                    Some(error) => bail!("{}", error.message),
                    None => Ok(worst),
                };
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations_read_naturally() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(12_400_000_000), "12.4 GB");
        assert_eq!(duration(5), "5 s");
        assert_eq!(duration(125), "2 min 5 s");
        assert_eq!(duration(7_260), "2 h 1 min");
    }

    #[test]
    fn command_line_parses() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        let cli = Cli::parse_from(["tunnel", "send", "a.txt", "dir", "--yes", "--json"]);
        assert!(cli.common.yes && cli.common.json);
        let cli = Cli::parse_from(["tunnel", "receive", "42-a-b-c", "--to", "/tmp/x", "--once"]);
        assert!(matches!(cli.command, Commands::Receive { once: true, .. }));
        assert!(
            Cli::try_parse_from(["tunnel", "send", "--relay", "https://r", "--no-relay"]).is_err()
        );
    }

    #[test]
    fn qr_codes_render() {
        let picture =
            qr("tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=rendezvous.example.com")
                .unwrap();
        assert!(picture.lines().count() > 10);
    }
}
