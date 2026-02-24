mod bridge;
mod config;
mod id_map;
mod leap_client;
mod leap_pairing;
mod ra2_protocol;
mod telnet_server;
mod translator;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ra-bridge", about = "RadioRA 3 → RadioRA 2 protocol relay")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pair with a Lutron RadioRA 3 processor
    Pair {
        /// Processor IP address
        #[arg(long)]
        host: String,
        /// Directory to save certificates
        #[arg(long, default_value = "certs")]
        certs_dir: PathBuf,
    },
    /// Run the RA2↔RA3 bridge relay
    Run {
        /// Path to config.toml
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Directory containing pairing certificates
        #[arg(long, default_value = "certs")]
        certs_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Pair { host, certs_dir } => {
            leap_pairing::pair(&host, &certs_dir).await?;
        }
        Commands::Run { config: config_path, certs_dir } => {
            let cfg = config::Config::load(&config_path)?;
            tracing::info!(
                "Loaded config: {} zones, LEAP at {}:{}",
                cfg.zones.len(),
                cfg.processor.host,
                cfg.processor.leap_port,
            );
            bridge::run(cfg, certs_dir).await?;
        }
    }

    Ok(())
}
