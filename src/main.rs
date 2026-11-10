use anyhow::Result;
use clap::Parser;
use noratunnel::{run, Cli};
use tokio::{signal, sync::broadcast};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<bool>(1);

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");
        tokio::spawn(async move {
            tokio::select! {
                res = signal::ctrl_c() => {
                    match res {
                        Ok(()) => tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown"),
                        Err(e) => tracing::error!("Failed to listen for SIGINT: {:?}", e),
                    }
                }
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM from systemd, initiating graceful shutdown");
                }
            }
            if let Err(e) = shutdown_tx.send(true) {
                tracing::warn!("Failed to broadcast shutdown signal: {:?}", e);
            }
        });
    }

    #[cfg(not(unix))]
    {
        tokio::spawn(async move {
            if let Err(e) = signal::ctrl_c().await {
                tracing::error!("Failed to listen for ctrl-c signal: {:?}", e);
            } else {
                tracing::info!("Received Ctrl+C, initiating graceful shutdown");
            }
            if let Err(e) = shutdown_tx.send(true) {
                tracing::warn!("Failed to broadcast shutdown signal: {:?}", e);
            }
        });
    }

    #[cfg(feature = "console")]
    {
        console_subscriber::init();

        tracing::info!("console_subscriber enabled");
    }
    #[cfg(not(feature = "console"))]
    {
        let is_atty = atty::is(atty::Stream::Stdout);

        let level = "info"; // if RUST_LOG not present, use `info` level
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::from(level)),
            )
            .with_ansi(is_atty)
            .init();
    }

    run(args, shutdown_rx).await
}
