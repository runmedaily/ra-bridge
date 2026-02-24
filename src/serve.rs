use std::path::PathBuf;

use anyhow::Result;
use tracing::info;

use tokio::sync::broadcast;

use crate::state::{AppState, BridgeStatus};

pub async fn serve(config_path: PathBuf, certs_dir: PathBuf, web_port: u16, log_tx: broadcast::Sender<String>) -> Result<()> {
    let state = AppState::new(config_path.clone(), certs_dir.clone(), log_tx);

    // Try loading existing config
    let has_config = if config_path.exists() {
        match crate::config::Config::load(&config_path) {
            Ok(cfg) => {
                info!(
                    "Loaded config: {} zones, processor at {}",
                    cfg.zones.len(),
                    cfg.processor.host,
                );
                *state.config.write().await = Some(cfg);
                true
            }
            Err(e) => {
                tracing::warn!("Failed to load config: {}", e);
                false
            }
        }
    } else {
        false
    };

    // Check if certs exist
    let has_certs = certs_dir.join("ra-bridge.crt").exists()
        && certs_dir.join("ra-bridge.key").exists()
        && certs_dir.join("ca.crt").exists();

    // Auto-start bridge if config + certs exist
    if has_config && has_certs {
        info!("Config and certs found, auto-starting bridge...");
        let config = state.config.read().await.clone().unwrap();
        let _ = state.bridge_status.send(BridgeStatus::Starting);

        match crate::bridge::start(
            config,
            certs_dir.clone(),
            state.zone_levels.clone(),
            state.bridge_status.clone(),
        )
        .await
        {
            Ok(handle) => {
                *state.leap_req_tx.write().await = handle.leap_req_tx;
                *state.savant_req_tx.write().await = handle.savant_req_tx;
                *state.bridge_shutdown.write().await = Some(handle.shutdown_tx);
                *state.bridge_started_at.write().await = Some(tokio::time::Instant::now());
                info!("Bridge auto-started");
            }
            Err(e) => {
                let _ = state
                    .bridge_status
                    .send(BridgeStatus::Error { message: e.to_string() });
                tracing::error!("Failed to auto-start bridge: {}", e);
            }
        }
    } else if has_config {
        // Config exists but no LEAP certs — check if Savant-only config
        let config = state.config.read().await.clone().unwrap();
        if config.has_savant() && !config.has_leap() {
            info!("Savant-only config found, auto-starting bridge...");
            let _ = state.bridge_status.send(BridgeStatus::Starting);
            // Use a dummy certs_dir; LEAP won't be started
            match crate::bridge::start(
                config,
                certs_dir.clone(),
                state.zone_levels.clone(),
                state.bridge_status.clone(),
            )
            .await
            {
                Ok(handle) => {
                    *state.leap_req_tx.write().await = handle.leap_req_tx;
                    *state.savant_req_tx.write().await = handle.savant_req_tx;
                    *state.bridge_shutdown.write().await = Some(handle.shutdown_tx);
                    *state.bridge_started_at.write().await = Some(tokio::time::Instant::now());
                    info!("Savant-only bridge auto-started");
                }
                Err(e) => {
                    let _ = state
                        .bridge_status
                        .send(BridgeStatus::Error { message: e.to_string() });
                    tracing::error!("Failed to auto-start Savant bridge: {}", e);
                }
            }
        } else {
            info!("No certs found — web UI will show setup wizard");
        }
    } else {
        info!("No config/certs found — web UI will show setup wizard");
    }

    // Start web server
    let app = crate::web::router(state);
    let addr = format!("0.0.0.0:{}", web_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Web server listening on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

pub async fn serve_dev(sites_dir: PathBuf, web_port: u16, log_tx: broadcast::Sender<String>) -> Result<()> {
    // Ensure sites directory exists
    std::fs::create_dir_all(&sites_dir)?;

    let state = AppState::new_dev(sites_dir.clone(), log_tx);

    // Auto-activate if exactly one site exists
    let sites = state.list_sites().await;
    if sites.len() == 1 {
        let site_name = &sites[0].name;
        info!("Single site found, auto-activating: {}", site_name);
        activate_site(&state, site_name).await?;
    } else if sites.is_empty() {
        info!("No sites found — create one via the web UI");
    } else {
        info!("{} sites found — select one via the web UI", sites.len());
    }

    // Start web server
    let app = crate::web::router(state);
    let addr = format!("0.0.0.0:{}", web_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Dev server listening on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

/// Activate a site: swap paths, load config, optionally auto-start bridge.
pub async fn activate_site(state: &AppState, site_name: &str) -> Result<()> {
    let sites_dir = state
        .sites_dir
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not in dev mode"))?;

    let site_path = sites_dir.join(site_name);
    if !site_path.exists() {
        anyhow::bail!("Site directory does not exist: {}", site_name);
    }

    // 1. Stop current bridge if running
    {
        let shutdown = state.bridge_shutdown.write().await.take();
        if let Some(tx) = shutdown {
            let _ = tx.send(()).await;
            *state.leap_req_tx.write().await = None;
            *state.savant_req_tx.write().await = None;
            *state.bridge_started_at.write().await = None;
            let _ = state.bridge_status.send(BridgeStatus::Stopped);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }

    // 2. Swap paths
    let config_path = site_path.join("config.toml");
    let certs_dir = site_path.join("certs");
    *state.config_path.write().await = config_path.clone();
    *state.certs_dir.write().await = certs_dir.clone();

    // 3. Clear zone levels (stale data from previous site)
    state.zone_levels.write().await.clear();

    // 4. Load new config
    let has_config = if config_path.exists() {
        match crate::config::Config::load(&config_path) {
            Ok(cfg) => {
                info!(
                    "Site '{}': loaded config — {} zones, processor at {}",
                    site_name,
                    cfg.zones.len(),
                    cfg.processor.host,
                );
                *state.config.write().await = Some(cfg);
                true
            }
            Err(e) => {
                tracing::warn!("Site '{}': failed to load config: {}", site_name, e);
                *state.config.write().await = None;
                false
            }
        }
    } else {
        *state.config.write().await = None;
        false
    };

    // 5. Set active site
    *state.active_site.write().await = Some(site_name.to_string());

    // 6. Auto-start bridge if config + certs exist
    let has_certs = certs_dir.join("ra-bridge.crt").exists()
        && certs_dir.join("ra-bridge.key").exists()
        && certs_dir.join("ca.crt").exists();

    if has_config && has_certs {
        info!("Site '{}': auto-starting bridge...", site_name);
        let config = state.config.read().await.clone().unwrap();
        let _ = state.bridge_status.send(BridgeStatus::Starting);

        match crate::bridge::start(
            config,
            certs_dir,
            state.zone_levels.clone(),
            state.bridge_status.clone(),
        )
        .await
        {
            Ok(handle) => {
                *state.leap_req_tx.write().await = handle.leap_req_tx;
                *state.savant_req_tx.write().await = handle.savant_req_tx;
                *state.bridge_shutdown.write().await = Some(handle.shutdown_tx);
                *state.bridge_started_at.write().await = Some(tokio::time::Instant::now());
                info!("Site '{}': bridge started", site_name);
            }
            Err(e) => {
                let _ = state
                    .bridge_status
                    .send(BridgeStatus::Error { message: e.to_string() });
                tracing::error!("Site '{}': failed to start bridge: {}", site_name, e);
            }
        }
    } else {
        info!(
            "Site '{}': no config/certs — needs pairing",
            site_name
        );
    }

    Ok(())
}
