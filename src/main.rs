mod bridge;
mod config;
mod discover;
mod id_map;
mod leap_client;
mod leap_pairing;
mod ra2_protocol;
mod savant_client;
mod savant_discover;
mod savant_id_map;
mod savant_translator;
mod serve;
mod state;
mod telnet_server;
mod translator;
mod web;
mod web_log_layer;

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
    /// Start the web management server (replaces pair + run for remote deployments)
    Serve {
        /// Path to config.toml
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Directory containing pairing certificates
        #[arg(long, default_value = "certs")]
        certs_dir: PathBuf,
        /// Web server port
        #[arg(long, default_value_t = 8080)]
        web_port: u16,
    },
    /// Multi-site dev server for managing multiple RA3 site profiles
    Dev {
        /// Directory containing site profiles
        #[arg(long, default_value = "sites")]
        sites_dir: PathBuf,
        /// Web server port
        #[arg(long, default_value_t = 8080)]
        web_port: u16,
    },
    /// Discover Savant devices and add them to config
    SavantDiscover {
        /// Savant Smart Host IP address
        #[arg(long)]
        host: String,
        /// Savant WebSocket port
        #[arg(long, default_value_t = 8480)]
        port: u16,
        /// Starting RA2 ID for Savant zones
        #[arg(long, default_value_t = 200)]
        start_id: u32,
        /// Path to config.toml
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let (log_tx, _) = tokio::sync::broadcast::channel::<String>(256);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
    let web_layer = web_log_layer::WebLogLayer::new(log_tx.clone());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(web_layer)
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
                "Loaded config: {} LEAP zones, {} Savant zones, LEAP at {}:{}",
                cfg.zones.len(),
                cfg.savant_zones.len(),
                cfg.processor.host,
                cfg.processor.leap_port,
            );
            bridge::run(cfg, certs_dir).await?;
        }
        Commands::Serve { config: config_path, certs_dir, web_port } => {
            serve::serve(config_path, certs_dir, web_port, log_tx).await?;
        }
        Commands::Dev { sites_dir, web_port } => {
            serve::serve_dev(sites_dir, web_port, log_tx).await?;
        }
        Commands::SavantDiscover { host, port, start_id, config: config_path } => {
            info!("Discovering Savant devices at {}:{}...", host, port);
            let (savant_config, savant_zones) = savant_discover::discover_zones(&host, port, start_id).await?;
            info!("Found {} Savant zones", savant_zones.len());

            // Merge into existing config or create new one
            let mut cfg = if config_path.exists() {
                config::Config::load(&config_path)?
            } else {
                config::Config {
                    processor: config::ProcessorConfig {
                        host: String::new(),
                        leap_port: 8081,
                    },
                    telnet: Default::default(),
                    web: Default::default(),
                    zones: vec![],
                    savant: None,
                    savant_zones: vec![],
                }
            };

            cfg.savant = Some(savant_config);
            cfg.savant_zones = savant_zones;

            if let Err(e) = cfg.validate() {
                anyhow::bail!("Config validation failed: {}", e);
            }

            cfg.save(&config_path)?;
            info!("Wrote {}", config_path.display());
        }
    }

    Ok(())
}
