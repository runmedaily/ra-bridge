use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use serde::Deserialize;
use tracing::info;

use crate::leap_client::{LeapHeader, LeapRequest};
use crate::savant_client::SavantRequest;
use crate::state::{AppState, BridgeStatus, PairingStatus, SavantDiscoveryStatus};

static INDEX_HTML: &str = include_str!("../../templates/index.html");

pub async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let bridge_status = state.bridge_status.borrow().clone();
    let config = state.config.read().await;
    let zone_count = config.as_ref().map(|c| c.zones.len()).unwrap_or(0);
    let savant_zone_count = config.as_ref().map(|c| c.savant_zones.len()).unwrap_or(0);
    let processor_host = config.as_ref().map(|c| c.processor.host.clone());
    let savant_host = config
        .as_ref()
        .and_then(|c| c.savant.as_ref().map(|s| s.host.clone()));
    drop(config);

    let uptime_secs = {
        let started = state.bridge_started_at.read().await;
        started.map(|t| t.elapsed().as_secs())
    };

    let active_site = state.active_site.read().await.clone();

    Json(serde_json::json!({
        "bridge": bridge_status,
        "zone_count": zone_count,
        "savant_zone_count": savant_zone_count,
        "processor_host": processor_host,
        "savant_host": savant_host,
        "uptime_secs": uptime_secs,
        "has_config": state.config.read().await.is_some(),
        "active_site": active_site,
        "dev_mode": state.dev_mode,
    }))
}

pub async fn zones(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let config = state.config.read().await;
    let levels = state.zone_levels.read().await;

    let mut zones: Vec<serde_json::Value> = Vec::new();

    if let Some(c) = config.as_ref() {
        // LEAP zones
        for z in &c.zones {
            let level = levels.get(&z.ra2_id).copied().unwrap_or(0.0);
            zones.push(serde_json::json!({
                "ra2_id": z.ra2_id,
                "leap_href": z.leap_href,
                "name": z.name,
                "level": level,
                "backend": "leap",
            }));
        }
        // Savant zones
        for z in &c.savant_zones {
            let level = levels.get(&z.ra2_id).copied().unwrap_or(0.0);
            zones.push(serde_json::json!({
                "ra2_id": z.ra2_id,
                "name": z.name,
                "room": z.room,
                "level": level,
                "backend": "savant",
            }));
        }
    }

    Json(serde_json::json!({ "zones": zones }))
}

#[derive(Deserialize)]
pub struct SetLevelRequest {
    level: f64,
}

enum ZoneTarget {
    Leap { href: String },
    Savant { address: String, load_offset: usize },
}

pub async fn set_zone_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(payload): Json<SetLevelRequest>,
) -> Response {
    let level = payload.level.clamp(0.0, 100.0);

    // Look up zone target while holding config lock, then release it
    let target = {
        let config_guard = state.config.read().await;
        let config = match config_guard.as_ref() {
            Some(c) => c,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "No config loaded" })),
                )
                    .into_response();
            }
        };

        if let Some(zone) = config.zones.iter().find(|z| z.ra2_id == id) {
            Some(ZoneTarget::Leap {
                href: zone.leap_href.clone(),
            })
        } else if let Some(zone) = config.savant_zones.iter().find(|z| z.ra2_id == id) {
            Some(ZoneTarget::Savant {
                address: zone.address.clone(),
                load_offset: zone.load_offset,
            })
        } else {
            None
        }
    };

    match target {
        Some(ZoneTarget::Leap { href }) => {
            info!("SetLevel zone={} level={} backend=LEAP href={}", id, level, href);
            let tx = state.leap_req_tx.read().await;
            let tx = match tx.as_ref() {
                Some(tx) => tx.clone(),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "Bridge not running" })),
                    )
                        .into_response();
                }
            };

            let req = LeapRequest {
                communique_type: "CreateRequest".to_string(),
                header: LeapHeader {
                    url: format!("{}/commandprocessor", href),
                    client_tag: None,
                    extra: serde_json::Map::new(),
                },
                body: Some(serde_json::json!({
                    "Command": {
                        "CommandType": "GoToLevel",
                        "Parameter": [{"Type": "Level", "Value": level}]
                    }
                })),
            };

            let _ = tx.send(req).await;
            state.zone_levels.write().await.insert(id, level);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Some(ZoneTarget::Savant {
            address,
            load_offset,
        }) => {
            info!("SetLevel zone={} level={} backend=Savant addr={} offset={}", id, level, address, load_offset);
            let tx = state.savant_req_tx.read().await;
            let tx = match tx.as_ref() {
                Some(tx) => tx.clone(),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "Bridge not running" })),
                    )
                        .into_response();
                }
            };

            let _ = tx
                .send(SavantRequest::SetLoad {
                    address,
                    load_offset,
                    level,
                })
                .await;
            state.zone_levels.write().await.insert(id, level);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Zone {} not found", id) })),
        )
            .into_response(),
    }
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> Response {
    let config = state.config.read().await;
    match config.as_ref() {
        Some(cfg) => match toml::to_string_pretty(cfg) {
            Ok(toml_str) => Json(serde_json::json!({ "config": toml_str })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
        None => Json(serde_json::json!({ "config": null })).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ConfigUpdate {
    config: String,
}

pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ConfigUpdate>,
) -> Response {
    let config_path = state.config_path.read().await.clone();
    match toml::from_str::<crate::config::Config>(&payload.config) {
        Ok(new_config) => {
            if let Err(e) = new_config.save(&config_path) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Failed to save: {}", e) })),
                )
                    .into_response();
            }
            *state.config.write().await = Some(new_config);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Invalid TOML: {}", e) })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct PairRequest {
    host: String,
    #[serde(default = "default_leap_port")]
    leap_port: u16,
    site_name: Option<String>,
}

fn default_leap_port() -> u16 {
    8081
}

pub async fn start_pair(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PairRequest>,
) -> Response {
    // Check if already pairing
    {
        let current = state.pairing_status.borrow().clone();
        match current {
            PairingStatus::Idle | PairingStatus::Complete { .. } | PairingStatus::Failed { .. } => {}
            _ => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": "Pairing already in progress" })),
                )
                    .into_response();
            }
        }
    }

    // In dev mode with site_name, resolve paths from sites_dir
    let (certs_dir, config_path) = if state.dev_mode {
        if let Some(ref site_name) = payload.site_name {
            let sites_dir = state.sites_dir.as_ref().unwrap();
            let site_path = sites_dir.join(site_name);
            let certs = site_path.join("certs");
            let config = site_path.join("config.toml");
            // Ensure certs dir exists
            let _ = std::fs::create_dir_all(&certs);
            (certs, config)
        } else {
            let certs = state.certs_dir.read().await.clone();
            let config = state.config_path.read().await.clone();
            (certs, config)
        }
    } else {
        let certs = state.certs_dir.read().await.clone();
        let config = state.config_path.read().await.clone();
        (certs, config)
    };

    let host = payload.host.clone();
    let leap_port = payload.leap_port;
    let status_tx = state.pairing_status.clone();
    let config_store = state.config.clone();

    tokio::spawn(async move {
        match crate::leap_pairing::pair_with_progress(
            &host,
            &certs_dir,
            &config_path,
            leap_port,
            status_tx.clone(),
        )
        .await
        {
            Ok(()) => {
                // Reload config after successful pairing
                if let Ok(cfg) = crate::config::Config::load(&config_path) {
                    *config_store.write().await = Some(cfg);
                }
                info!("Pairing completed successfully via web UI");
            }
            Err(e) => {
                let _ = status_tx.send(PairingStatus::Failed {
                    message: e.to_string(),
                });
                tracing::error!("Pairing failed: {}", e);
            }
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub async fn discover(State(state): State<Arc<AppState>>) -> Response {
    let config = state.config.read().await;
    let (host, leap_port) = match config.as_ref() {
        Some(cfg) => (cfg.processor.host.clone(), cfg.processor.leap_port),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "No config loaded — pair first" })),
            )
                .into_response();
        }
    };
    drop(config);

    let certs_dir = state.certs_dir.read().await.clone();
    let config_path = state.config_path.read().await.clone();
    let config_store = state.config.clone();

    match crate::discover::discover_zones(&host, leap_port, &certs_dir).await {
        Ok(zones) => {
            let zone_count = zones.len();
            if let Err(e) = crate::discover::write_config(&config_path, &host, leap_port, &zones) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Failed to write config: {}", e) })),
                )
                    .into_response();
            }
            // Reload
            if let Ok(cfg) = crate::config::Config::load(&config_path) {
                *config_store.write().await = Some(cfg);
            }
            Json(serde_json::json!({ "ok": true, "zone_count": zone_count })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Discovery failed: {}", e) })),
        )
            .into_response(),
    }
}

pub async fn bridge_start(State(state): State<Arc<AppState>>) -> Response {
    // Check current status
    {
        let current = state.bridge_status.borrow().clone();
        if matches!(current, BridgeStatus::Running | BridgeStatus::Starting) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "Bridge already running" })),
            )
                .into_response();
        }
    }

    let config = state.config.read().await.clone();
    let config = match config {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "No config loaded" })),
            )
                .into_response();
        }
    };

    let _ = state.bridge_status.send(BridgeStatus::Starting);

    let certs_dir = state.certs_dir.read().await.clone();
    let zone_levels = state.zone_levels.clone();
    let bridge_status_tx = state.bridge_status.clone();

    match crate::bridge::start(config, certs_dir, zone_levels, bridge_status_tx).await {
        Ok(handle) => {
            *state.leap_req_tx.write().await = handle.leap_req_tx;
            *state.savant_req_tx.write().await = handle.savant_req_tx;
            *state.bridge_shutdown.write().await = Some(handle.shutdown_tx);
            *state.bridge_started_at.write().await = Some(tokio::time::Instant::now());
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => {
            let _ = state
                .bridge_status
                .send(BridgeStatus::Error { message: e.to_string() });
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to start: {}", e) })),
            )
                .into_response()
        }
    }
}

pub async fn bridge_stop(State(state): State<Arc<AppState>>) -> Response {
    let shutdown = state.bridge_shutdown.write().await.take();
    match shutdown {
        Some(tx) => {
            let _ = tx.send(()).await;
            *state.leap_req_tx.write().await = None;
            *state.savant_req_tx.write().await = None;
            *state.bridge_started_at.write().await = None;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Bridge not running" })),
        )
            .into_response(),
    }
}

pub async fn bridge_restart(State(state): State<Arc<AppState>>) -> Response {
    // Stop first
    {
        let shutdown = state.bridge_shutdown.write().await.take();
        if let Some(tx) = shutdown {
            let _ = tx.send(()).await;
            *state.leap_req_tx.write().await = None;
            *state.savant_req_tx.write().await = None;
            *state.bridge_started_at.write().await = None;
        }
    }

    // Small delay to let things clean up
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Start again
    bridge_start(State(state)).await
}

pub async fn export_xml(State(state): State<Arc<AppState>>) -> Response {
    let config = state.config.read().await;
    match config.as_ref() {
        Some(cfg) => {
            let xml = super::xml_export::generate_xml(&cfg.zones, &cfg.savant_zones);
            (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/xml; charset=utf-8",
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    "attachment; filename=\"DbXmlInfo.xml\"",
                )],
                xml,
            )
                .into_response()
        }
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No config loaded" })),
        )
            .into_response(),
    }
}

// --- Savant discovery endpoints ---

#[derive(Deserialize)]
pub struct SavantDiscoverRequest {
    host: String,
    #[serde(default = "default_savant_port")]
    port: u16,
    #[serde(default = "default_savant_start_id")]
    start_id: u32,
}

fn default_savant_port() -> u16 {
    8480
}

fn default_savant_start_id() -> u32 {
    200
}

pub async fn savant_discover(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SavantDiscoverRequest>,
) -> Response {
    // Check if already discovering
    {
        let current = state.savant_discovery_status.borrow().clone();
        match current {
            SavantDiscoveryStatus::Connecting | SavantDiscoveryStatus::Enumerating { .. } => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": "Savant discovery already in progress" })),
                )
                    .into_response();
            }
            _ => {}
        }
    }

    let host = payload.host.clone();
    let port = payload.port;
    let start_id = payload.start_id;
    let status_tx = state.savant_discovery_status.clone();
    let config_store = state.config.clone();
    let config_path = state.config_path.read().await.clone();

    tokio::spawn(async move {
        let _ = status_tx.send(SavantDiscoveryStatus::Connecting);

        match crate::savant_discover::discover_zones(&host, port, start_id).await {
            Ok((savant_config, savant_zones)) => {
                let zone_count = savant_zones.len();
                let _ = status_tx.send(SavantDiscoveryStatus::Enumerating {
                    device_count: zone_count,
                });
                let mut config = config_store
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(|| crate::config::Config {
                        processor: crate::config::ProcessorConfig {
                            host: String::new(),
                            leap_port: 8081,
                        },
                        telnet: Default::default(),
                        web: Default::default(),
                        zones: vec![],
                        savant: None,
                        savant_zones: vec![],
                    });

                config.savant = Some(savant_config);
                config.savant_zones = savant_zones;

                // Validate no ID conflicts
                if let Err(e) = config.validate() {
                    let _ = status_tx.send(SavantDiscoveryStatus::Failed { message: e });
                    return;
                }

                // Save
                if let Err(e) = config.save(&config_path) {
                    let _ = status_tx.send(SavantDiscoveryStatus::Failed {
                        message: e.to_string(),
                    });
                    return;
                }

                *config_store.write().await = Some(config);
                let _ = status_tx.send(SavantDiscoveryStatus::Complete { zone_count });
                info!(
                    "Savant discovery complete: {} zones saved to config",
                    zone_count
                );
            }
            Err(e) => {
                let _ = status_tx.send(SavantDiscoveryStatus::Failed {
                    message: e.to_string(),
                });
                tracing::error!("Savant discovery failed: {}", e);
            }
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "ok": true }))).into_response()
}

pub async fn savant_remove(State(state): State<Arc<AppState>>) -> Response {
    let config_path = state.config_path.read().await.clone();
    let mut config_guard = state.config.write().await;

    match config_guard.as_mut() {
        Some(config) => {
            config.savant = None;
            config.savant_zones.clear();

            if let Err(e) = config.save(&config_path) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Failed to save: {}", e) })),
                )
                    .into_response();
            }

            Json(serde_json::json!({ "ok": true })).into_response()
        }
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No config loaded" })),
        )
            .into_response(),
    }
}

// --- Site management endpoints (dev mode) ---

pub async fn list_sites(State(state): State<Arc<AppState>>) -> Response {
    if !state.dev_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Not in dev mode" })),
        )
            .into_response();
    }
    let sites = state.list_sites().await;
    let active = state.active_site.read().await.clone();
    Json(serde_json::json!({ "sites": sites, "active": active })).into_response()
}

#[derive(Deserialize)]
pub struct CreateSiteRequest {
    name: String,
}

pub async fn create_site(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSiteRequest>,
) -> Response {
    if !state.dev_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Not in dev mode" })),
        )
            .into_response();
    }

    let name = payload.name.trim().to_string();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid site name" })),
        )
            .into_response();
    }

    let sites_dir = state.sites_dir.as_ref().unwrap();
    let site_path = sites_dir.join(&name);

    if site_path.exists() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Site already exists" })),
        )
            .into_response();
    }

    if let Err(e) = std::fs::create_dir_all(site_path.join("certs")) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to create site: {}", e) })),
        )
            .into_response();
    }

    info!("Created site: {}", name);
    Json(serde_json::json!({ "ok": true, "name": name })).into_response()
}

pub async fn delete_site(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    if !state.dev_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Not in dev mode" })),
        )
            .into_response();
    }

    // Can't delete active site
    let active = state.active_site.read().await.clone();
    if active.as_deref() == Some(&name) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Cannot delete active site — switch to another first" })),
        )
            .into_response();
    }

    let sites_dir = state.sites_dir.as_ref().unwrap();
    let site_path = sites_dir.join(&name);

    if !site_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Site not found" })),
        )
            .into_response();
    }

    if let Err(e) = std::fs::remove_dir_all(&site_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to delete: {}", e) })),
        )
            .into_response();
    }

    info!("Deleted site: {}", name);
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub async fn activate_site(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    if !state.dev_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Not in dev mode" })),
        )
            .into_response();
    }

    match crate::serve::activate_site(&state, &name).await {
        Ok(()) => {
            info!("Activated site: {}", name);
            Json(serde_json::json!({ "ok": true, "active": name })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct RenameSiteRequest {
    new_name: String,
}

pub async fn rename_site(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(payload): Json<RenameSiteRequest>,
) -> Response {
    if !state.dev_mode {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Not in dev mode" })),
        )
            .into_response();
    }

    let new_name = payload.new_name.trim().to_string();
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\\') || new_name.starts_with('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid site name" })),
        )
            .into_response();
    }

    let sites_dir = state.sites_dir.as_ref().unwrap();
    let old_path = sites_dir.join(&name);
    let new_path = sites_dir.join(&new_name);

    if !old_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Site not found" })),
        )
            .into_response();
    }

    if new_path.exists() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "A site with that name already exists" })),
        )
            .into_response();
    }

    if let Err(e) = std::fs::rename(&old_path, &new_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to rename: {}", e) })),
        )
            .into_response();
    }

    // If this was the active site, update paths
    let mut active = state.active_site.write().await;
    if active.as_deref() == Some(name.as_str()) {
        *active = Some(new_name.clone());
        drop(active);
        *state.config_path.write().await = new_path.join("config.toml");
        *state.certs_dir.write().await = new_path.join("certs");
    }

    info!("Renamed site: {} → {}", name, new_name);
    Json(serde_json::json!({ "ok": true, "new_name": new_name })).into_response()
}
