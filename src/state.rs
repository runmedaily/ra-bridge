use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tokio::time::Instant;

use crate::config::Config;
use crate::leap_client::LeapRequest;
use crate::savant_client::SavantRequest;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state")]
pub enum BridgeStatus {
    Stopped,
    Starting,
    Running,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state")]
pub enum PairingStatus {
    Idle,
    GeneratingKeys,
    ConnectingToProcessor,
    WaitingForButtonPress { elapsed: u64, timeout: u64 },
    ButtonPressed,
    ReceivingCertificate,
    VerifyingPairing,
    DiscoveringZones,
    Complete { zone_count: usize },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state")]
pub enum SavantDiscoveryStatus {
    Idle,
    Connecting,
    Enumerating { device_count: usize },
    Complete { zone_count: usize },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteInfo {
    pub name: String,
    pub has_config: bool,
    pub has_certs: bool,
    pub active: bool,
}

pub struct AppState {
    pub config: Arc<RwLock<Option<Config>>>,
    pub bridge_status: watch::Sender<BridgeStatus>,
    pub pairing_status: watch::Sender<PairingStatus>,
    pub savant_discovery_status: watch::Sender<SavantDiscoveryStatus>,
    pub zone_levels: Arc<RwLock<HashMap<u32, f64>>>,
    pub bridge_started_at: RwLock<Option<Instant>>,
    pub leap_req_tx: RwLock<Option<mpsc::Sender<LeapRequest>>>,
    pub savant_req_tx: RwLock<Option<mpsc::Sender<SavantRequest>>>,
    pub bridge_shutdown: RwLock<Option<mpsc::Sender<()>>>,

    // Swappable paths (RwLock for dev mode site switching)
    pub config_path: RwLock<PathBuf>,
    pub certs_dir: RwLock<PathBuf>,

    // Multi-site (dev mode only)
    pub sites_dir: Option<PathBuf>,
    pub active_site: RwLock<Option<String>>,
    pub dev_mode: bool,

    // Log broadcast for web UI
    pub log_tx: broadcast::Sender<String>,
}

impl AppState {
    pub fn new(config_path: PathBuf, certs_dir: PathBuf, log_tx: broadcast::Sender<String>) -> Arc<Self> {
        let (bridge_status, _) = watch::channel(BridgeStatus::Stopped);
        let (pairing_status, _) = watch::channel(PairingStatus::Idle);
        let (savant_discovery_status, _) = watch::channel(SavantDiscoveryStatus::Idle);

        Arc::new(Self {
            config: Arc::new(RwLock::new(None)),
            bridge_status,
            pairing_status,
            savant_discovery_status,
            zone_levels: Arc::new(RwLock::new(HashMap::new())),
            bridge_started_at: RwLock::new(None),
            leap_req_tx: RwLock::new(None),
            savant_req_tx: RwLock::new(None),
            bridge_shutdown: RwLock::new(None),
            config_path: RwLock::new(config_path),
            certs_dir: RwLock::new(certs_dir),
            sites_dir: None,
            active_site: RwLock::new(None),
            dev_mode: false,
            log_tx,
        })
    }

    pub fn new_dev(sites_dir: PathBuf, log_tx: broadcast::Sender<String>) -> Arc<Self> {
        let (bridge_status, _) = watch::channel(BridgeStatus::Stopped);
        let (pairing_status, _) = watch::channel(PairingStatus::Idle);
        let (savant_discovery_status, _) = watch::channel(SavantDiscoveryStatus::Idle);

        Arc::new(Self {
            config: Arc::new(RwLock::new(None)),
            bridge_status,
            pairing_status,
            savant_discovery_status,
            zone_levels: Arc::new(RwLock::new(HashMap::new())),
            bridge_started_at: RwLock::new(None),
            leap_req_tx: RwLock::new(None),
            savant_req_tx: RwLock::new(None),
            bridge_shutdown: RwLock::new(None),
            config_path: RwLock::new(PathBuf::from("config.toml")),
            certs_dir: RwLock::new(PathBuf::from("certs")),
            sites_dir: Some(sites_dir),
            active_site: RwLock::new(None),
            dev_mode: true,
            log_tx,
        })
    }

    pub async fn list_sites(&self) -> Vec<SiteInfo> {
        let sites_dir = match &self.sites_dir {
            Some(d) => d,
            None => return vec![],
        };

        let active = self.active_site.read().await.clone();

        let mut sites = Vec::new();
        let entries = match std::fs::read_dir(sites_dir) {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let site_path = entry.path();
            let has_config = site_path.join("config.toml").exists();
            let has_certs = site_path.join("certs/ra-bridge.crt").exists()
                && site_path.join("certs/ra-bridge.key").exists()
                && site_path.join("certs/ca.crt").exists();
            let is_active = active.as_deref() == Some(&name);

            sites.push(SiteInfo {
                name,
                has_config,
                has_certs,
                active: is_active,
            });
        }

        sites.sort_by(|a, b| a.name.cmp(&b.name));
        sites
    }
}
