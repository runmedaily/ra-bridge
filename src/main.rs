mod bridge;
mod config;
mod discover;
mod id_map;
mod leap_client;
mod leap_pairing;
mod ra2_protocol;
mod telnet_server;
mod translator;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Parser)]
#[command(name = "ra-bridge", about = "RadioRA 3 → RadioRA 2 protocol relay")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pair with a Lutron RadioRA 3 processor, then discover zones
    Pair {
        /// Processor IP address
        #[arg(long)]
        host: String,
        /// Directory to save certificates
        #[arg(long, default_value = "certs")]
        certs_dir: PathBuf,
        /// Path to write config.toml after discovery
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// LEAP port on the processor
        #[arg(long, default_value_t = 8081)]
        leap_port: u16,
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
        Commands::Pair { host, certs_dir, config: config_path, leap_port } => {
            leap_pairing::pair(&host, &certs_dir).await?;

            info!("Discovering zones...");
            let zones = discover::discover_zones(&host, leap_port, &certs_dir).await?;
            info!("Found {} zones", zones.len());

            discover::write_config(&config_path, &host, leap_port, &zones)?;
            info!("Wrote {}", config_path.display());
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
