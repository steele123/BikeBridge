//! Native BikeBridge daemon and small status/device-list client.
mod config;

use anyhow::{Context, Result, bail};
use bikebridge_ble::{NativeBackend, Scanner};
use bikebridge_server::AppState;
use clap::{Parser, Subcommand};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "bikebridge",
    version,
    about = "One simple local API for cycling hardware (FTMS trainers, controller inputs, recorder/replay, and mocks)"
)]
struct Cli {
    /// TOML config file (defaults to ./bikebridge.toml when present).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Loopback IP address; remote access is unavailable.
    #[arg(long, global = true)]
    host: Option<IpAddr>,
    /// API port.
    #[arg(long, global = true)]
    port: Option<u16>,
    /// Tracing filter, e.g. info or bikebridge_server=debug.
    #[arg(long, global = true)]
    log_level: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the foreground daemon; stop with Ctrl+C.
    Run {
        /// Enable an already-connected MockTrainer and MockController.
        #[arg(long)]
        mock: bool,
        /// Enumerate adapters but wait for an explicit scan request.
        #[arg(long)]
        no_auto_scan: bool,
        /// Zero-based adapter index; defaults to the first powered-on adapter.
        #[arg(long)]
        adapter_index: Option<usize>,
        /// Record the entire daemon session to a new .biketrace file.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Stop a recording cleanly after this many seconds (otherwise Ctrl+C).
        #[arg(long, requires = "record")]
        duration: Option<f64>,
    },
    /// Serve a trace without Bluetooth. Starts paused unless --autoplay is given.
    Replay {
        trace: PathBuf,
        /// Playback speed multiplier, from 0.1 to 16.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Begin immediately instead of waiting for replay-control start.
        #[arg(long)]
        autoplay: bool,
    },
    /// Start, pause, or restart the running replay daemon.
    ReplayControl {
        #[arg(value_parser = ["start", "pause", "restart"])]
        action: String,
    },
    /// Validate a trace and print metadata without starting a daemon.
    TraceInfo { trace: PathBuf },
    /// Fetch status from the running daemon.
    Status,
    /// List devices from the running daemon.
    Devices,
    /// Connect a discovered FTMS trainer or OpenBikeControl bridge by its device ID.
    Connect { device_id: String },
    /// Disconnect a device using its BikeBridge device ID.
    Disconnect { device_id: String },
    /// List Bluetooth adapters from the running daemon.
    Adapters,
    /// Start the daemon's continuous BLE scan (use --stop to stop it).
    Scan {
        /// Stop scanning; discovered devices remain listed.
        #[arg(long)]
        stop: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = config::Config::load(cli.config.as_deref())?;
    if let Some(host) = cli.host {
        config.server.host = host;
    }
    if let Some(port) = cli.port {
        config.server.port = port;
    }
    if let Some(level) = cli.log_level {
        config.logging.level = level;
    }
    config.validate()?;
    let filter = tracing_subscriber::EnvFilter::try_new(&config.logging.level)
        .context("Invalid logging filter")?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let address = SocketAddr::new(config.server.host, config.server.port);
    match cli.command {
        Commands::Run {
            mock,
            no_auto_scan,
            adapter_index,
            record,
            duration,
        } => {
            if let Some(seconds) = duration
                && (!seconds.is_finite() || !(0.1..=604800.0).contains(&seconds))
            {
                bail!("Recording duration must be between 0.1 and 604800 seconds");
            }
            let mut state = AppState::new(mock, config.trainer)?;
            let listener = TcpListener::bind(address)
                .await
                .context("Cannot bind BikeBridge API listener")?;
            let recorder = match record {
                Some(path) => {
                    let recorder = bikebridge_trace::Recorder::start(
                        &path,
                        state.events.clone(),
                        state.devices().await,
                    )?;
                    println!(
                        "Recording to {}. Stop cleanly to finalize the trace.",
                        path.display()
                    );
                    Some(recorder)
                }
                None => None,
            };
            if !mock {
                let scanner = Scanner::spawn_configured(
                    NativeBackend::default(),
                    state.events.clone(),
                    adapter_index.or(config.bluetooth.adapter_index),
                    config.trainer,
                )
                .await?;
                if config.bluetooth.auto_scan
                    && !no_auto_scan
                    && let Err(error) = scanner.start().await
                {
                    tracing::warn!(code = ?error.code, message = %error.message, "Initial scan unavailable; API remains available");
                }
                state = state.with_scanner(scanner);
            }
            tracing::info!(version = env!("CARGO_PKG_VERSION"), %address, mock, "BikeBridge started");
            println!(
                "BikeBridge {}\n\nHTTP: http://{address}\nWebSocket: ws://{address}/ws",
                env!("CARGO_PKG_VERSION")
            );
            if mock {
                println!("Connected: mock-trainer, mock-controller");
            } else {
                println!(
                    "BLE discovery enabled. Use `bikebridge devices`, then `bikebridge connect <device-id>` for FTMS telemetry or OpenBikeControl inputs."
                );
            }
            serve_daemon(listener, state, recorder, duration).await?;
        }
        Commands::Replay {
            trace,
            speed,
            autoplay,
        } => {
            let trace =
                tokio::task::spawn_blocking(move || bikebridge_trace::Trace::load(trace)).await??;
            let state = AppState::replay(trace, speed)?;
            let listener = TcpListener::bind(address)
                .await
                .context("Cannot bind replay API listener")?;
            if autoplay {
                state.replay_action("start").await?;
            }
            println!(
                "BikeBridge replay at http://{address} (Bluetooth disabled).\nConnect and subscribe, then run: bikebridge replay-control start"
            );
            serve_daemon(listener, state, None, None).await?;
        }
        Commands::ReplayControl { action } => {
            print_endpoint(address, "POST", &format!("/api/replay/{action}")).await?;
        }
        Commands::TraceInfo { trace } => {
            let trace =
                tokio::task::spawn_blocking(move || bikebridge_trace::Trace::load(trace)).await??;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "header":trace.header(), "events":trace.records().len(), "durationUs":trace.duration_us(), "complete":true
                }))?
            );
        }
        Commands::Status => print_endpoint(address, "GET", "/api/status").await?,
        Commands::Devices => print_endpoint(address, "GET", "/api/devices").await?,
        Commands::Connect { ref device_id } | Commands::Disconnect { ref device_id } => {
            // IDs are opaque path segments, never arbitrary URLs or HTTP request text.
            if device_id.is_empty()
                || device_id.len() > 128
                || !device_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                bail!("Invalid device ID. Copy the id from `bikebridge devices`.");
            }
            let action = if matches!(cli.command, Commands::Connect { .. }) {
                "connect"
            } else {
                "disconnect"
            };
            print_endpoint(
                address,
                "POST",
                &format!("/api/devices/{device_id}/{action}"),
            )
            .await?;
        }
        Commands::Adapters => print_endpoint(address, "GET", "/api/adapters").await?,
        Commands::Scan { stop } => {
            print_endpoint(
                address,
                "POST",
                if stop {
                    "/api/scan/stop"
                } else {
                    "/api/scan/start"
                },
            )
            .await?
        }
    }
    Ok(())
}

async fn serve_daemon(
    listener: TcpListener,
    state: AppState,
    recorder: Option<bikebridge_trace::Recorder>,
    duration: Option<f64>,
) -> Result<()> {
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(error) = shutdown_signal().await {
            tracing::error!(%error, "Signal handler failed; stopping daemon");
        }
        stop.cancel();
    });
    let serving = bikebridge_server::serve(listener, state, shutdown.clone());
    tokio::pin!(serving);
    let limit = async {
        match duration {
            Some(seconds) => tokio::time::sleep(Duration::from_secs_f64(seconds)).await,
            None => std::future::pending().await,
        }
    };
    let monitor = async {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if recorder.as_ref().is_some_and(|r| r.has_failed()) {
                tracing::error!(
                    "Recording failed; stopping daemon. The incomplete trace cannot be replayed."
                );
                break;
            }
        }
    };
    let result = tokio::select! {
        result = &mut serving => result,
        _ = limit => { shutdown.cancel(); serving.await },
        _ = monitor => { shutdown.cancel(); serving.await },
    };
    signal_task.abort();
    if let Some(recorder) = recorder {
        let summary = tokio::task::spawn_blocking(move || recorder.finish()).await??;
        println!(
            "Trace finalized: {} events, {:.3} seconds",
            summary.events,
            summary.duration_us as f64 / 1_000_000.0
        );
    }
    result.context("API server failed")?;
    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

// Tiny bounded HTTP/1.0 client for the daemon's JSON snapshot and scan endpoints.
// Avoid pulling a general-purpose HTTP/TLS stack into a local CLI.
async fn print_endpoint(address: SocketAddr, method: &str, path: &str) -> Result<()> {
    let response = tokio::time::timeout(Duration::from_secs(30), async {
        let mut stream = TcpStream::connect(address)
            .await
            .context("Cannot reach BikeBridge. Start `bikebridge run` first (or `run --mock` for simulated devices).")?;
        stream
            .write_all(
                format!("{method} {path} HTTP/1.0\r\nHost: {address}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut bytes = Vec::new();
        stream.take(1_048_577).read_to_end(&mut bytes).await?;
        if bytes.len() > 1_048_576 {
            bail!("Response exceeded size limit");
        }
        String::from_utf8(bytes).context("Invalid HTTP response")
    })
    .await
    .context("API request timed out")??;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .context("Malformed HTTP response")?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("HTTP/1.0 200 ") || line.starts_with("HTTP/1.1 200 "))
    {
        bail!("API request failed: {body}");
    }
    let value: serde_json::Value = serde_json::from_str(body).context("Malformed API JSON")?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
