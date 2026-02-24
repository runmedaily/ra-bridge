use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tracing::{info, warn};

use crate::id_map::IdMap;
use crate::leap_client::{LeapEvent, LeapRequest};
use crate::ra2_protocol::{Ra2Command, Ra2Event};
use crate::savant_client::{SavantEvent, SavantRequest};
use crate::savant_id_map::SavantIdMap;
use crate::{savant_translator, translator};

/// Handles returned from `start()` to control the bridge externally.
pub struct BridgeHandle {
    pub leap_req_tx: Option<mpsc::Sender<LeapRequest>>,
    pub savant_req_tx: Option<mpsc::Sender<SavantRequest>>,
    pub shutdown_tx: mpsc::Sender<()>,
}

/// Start the bridge as a background task. Returns a handle for external control.
pub async fn start(
    config: crate::config::Config,
    certs_dir: std::path::PathBuf,
    zone_levels: Arc<RwLock<HashMap<u32, f64>>>,
    bridge_status_tx: watch::Sender<crate::state::BridgeStatus>,
) -> Result<BridgeHandle> {
    let leap_id_map = Arc::new(IdMap::from_zones(&config.zones));
    let savant_id_map = Arc::new(SavantIdMap::from_zones(&config.savant_zones));

    // Channels: telnet → bridge (RA2 commands)
    let (ra2_cmd_tx, mut ra2_cmd_rx) = mpsc::channel::<Ra2Command>(256);

    // Channels: bridge → telnet (RA2 events, broadcast to all clients)
    let (ra2_event_tx, _) = broadcast::channel::<Ra2Event>(256);

    // Shutdown signal
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    // Start telnet server
    let telnet_event_tx = ra2_event_tx.clone();
    let telnet_port = config.telnet.port;
    tokio::spawn(async move {
        if let Err(e) =
            crate::telnet_server::run(telnet_port, ra2_cmd_tx, telnet_event_tx).await
        {
            tracing::error!("Telnet server error: {}", e);
        }
    });

    // Conditionally start LEAP client
    let leap_req_tx = if config.has_leap() {
        let (tx, leap_req_rx) = mpsc::channel::<LeapRequest>(256);
        let (leap_event_tx, mut leap_event_rx) = broadcast::channel::<LeapEvent>(256);

        let leap_host = config.processor.host.clone();
        let leap_port = config.processor.leap_port;
        tokio::spawn(async move {
            if let Err(e) =
                crate::leap_client::run(leap_host, leap_port, certs_dir, leap_req_rx, leap_event_tx)
                    .await
            {
                tracing::error!("LEAP client error: {}", e);
            }
        });

        // LEAP event forwarder
        let ra2_event_tx_leap = ra2_event_tx.clone();
        let zone_levels_leap = zone_levels.clone();
        let leap_id_map_clone = leap_id_map.clone();
        tokio::spawn(async move {
            loop {
                match leap_event_rx.recv().await {
                    Ok(event) => {
                        if let Some(ra2_event) = translator::leap_to_ra2(&event, &leap_id_map_clone)
                        {
                            let Ra2Event::OutputLevel { id, level } = &ra2_event;
                            zone_levels_leap.write().await.insert(*id, *level);
                            let _ = ra2_event_tx_leap.send(ra2_event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("LEAP event forwarder lagged by {}", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        info!("LEAP backend started ({} zones)", config.zones.len());
        Some(tx)
    } else {
        info!("LEAP backend skipped (no zones configured)");
        None
    };

    // Conditionally start Savant client
    let savant_req_tx = if config.has_savant() {
        let savant_cfg = config.savant.as_ref().unwrap();
        let (tx, savant_req_rx) = mpsc::channel::<SavantRequest>(256);
        let (savant_event_tx, mut savant_event_rx) = broadcast::channel::<SavantEvent>(256);

        let savant_host = savant_cfg.host.clone();
        let savant_port = savant_cfg.port;
        let savant_zones = config.savant_zones.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::savant_client::run(savant_host, savant_port, savant_zones, savant_req_rx, savant_event_tx)
                    .await
            {
                tracing::error!("Savant client error: {}", e);
            }
        });

        // Savant event forwarder
        let ra2_event_tx_savant = ra2_event_tx.clone();
        let zone_levels_savant = zone_levels.clone();
        let savant_id_map_clone = savant_id_map.clone();
        tokio::spawn(async move {
            loop {
                match savant_event_rx.recv().await {
                    Ok(event) => {
                        if let Some(ra2_event) =
                            savant_translator::savant_to_ra2(&event, &savant_id_map_clone)
                        {
                            let Ra2Event::OutputLevel { id, level } = &ra2_event;
                            zone_levels_savant.write().await.insert(*id, *level);
                            let _ = ra2_event_tx_savant.send(ra2_event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Savant event forwarder lagged by {}", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        info!(
            "Savant backend started ({} zones)",
            config.savant_zones.len()
        );
        Some(tx)
    } else {
        info!("Savant backend skipped (not configured)");
        None
    };

    let _ = bridge_status_tx.send(crate::state::BridgeStatus::Running);
    info!("Bridge running");

    // Clone senders for the handle before moving into spawn
    let handle_leap_req_tx = leap_req_tx.clone();
    let handle_savant_req_tx = savant_req_tx.clone();

    // Translation loop in background task — routes by ra2_id ownership
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(cmd) = ra2_cmd_rx.recv() => {
                    let id = match &cmd {
                        Ra2Command::SetOutput { id, .. } => Some(*id),
                        Ra2Command::QueryOutput { id } => Some(*id),
                        Ra2Command::Monitoring { .. } => None,
                    };

                    if let Some(id) = id {
                        // Route to the correct backend based on ra2_id ownership
                        if leap_id_map.ra2_to_leap(id).is_some() {
                            if let Some(ref tx) = leap_req_tx {
                                if let Some(req) = translator::ra2_to_leap(&cmd, &leap_id_map) {
                                    if let Err(e) = tx.send(req).await {
                                        warn!("Failed to send LEAP request: {}", e);
                                    }
                                }
                            }
                        } else if savant_id_map.ra2_to_savant(id).is_some() {
                            if let Some(ref tx) = savant_req_tx {
                                if let Some(req) = savant_translator::ra2_to_savant(&cmd, &savant_id_map) {
                                    if let Err(e) = tx.send(req).await {
                                        warn!("Failed to send Savant request: {}", e);
                                    }
                                }
                            }
                        } else {
                            warn!("No backend for ra2_id {}", id);
                        }
                    } else {
                        // Monitoring commands — log them
                        if let Ra2Command::Monitoring { mon_type, enable } = &cmd {
                            info!("Monitoring type {} {}",
                                mon_type, if *enable { "enabled" } else { "disabled" });
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Bridge shutting down");
                    let _ = bridge_status_tx.send(crate::state::BridgeStatus::Stopped);
                    break;
                }
            }
        }
    });

    Ok(BridgeHandle {
        leap_req_tx: handle_leap_req_tx,
        savant_req_tx: handle_savant_req_tx,
        shutdown_tx,
    })
}

/// Run the bridge (blocking). Used by the `run` CLI command for backward compatibility.
pub async fn run(
    config: crate::config::Config,
    certs_dir: std::path::PathBuf,
) -> Result<()> {
    let leap_id_map = Arc::new(IdMap::from_zones(&config.zones));
    let savant_id_map = Arc::new(SavantIdMap::from_zones(&config.savant_zones));

    // Channels: telnet → bridge (RA2 commands)
    let (ra2_cmd_tx, mut ra2_cmd_rx) = mpsc::channel::<Ra2Command>(256);

    // Channels: bridge → telnet (RA2 events, broadcast to all clients)
    let (ra2_event_tx, _) = broadcast::channel::<Ra2Event>(256);

    // Start telnet server
    let telnet_event_tx = ra2_event_tx.clone();
    let telnet_port = config.telnet.port;
    tokio::spawn(async move {
        if let Err(e) =
            crate::telnet_server::run(telnet_port, ra2_cmd_tx, telnet_event_tx).await
        {
            tracing::error!("Telnet server error: {}", e);
        }
    });

    // Conditionally start LEAP client
    let leap_req_tx = if config.has_leap() {
        let (tx, leap_req_rx) = mpsc::channel::<LeapRequest>(256);
        let (leap_event_tx, mut leap_event_rx) = broadcast::channel::<LeapEvent>(256);

        let leap_host = config.processor.host.clone();
        let leap_port = config.processor.leap_port;
        tokio::spawn(async move {
            if let Err(e) =
                crate::leap_client::run(leap_host, leap_port, certs_dir, leap_req_rx, leap_event_tx)
                    .await
            {
                tracing::error!("LEAP client error: {}", e);
            }
        });

        // LEAP event forwarder
        let ra2_event_tx_leap = ra2_event_tx.clone();
        let leap_id_map_clone = leap_id_map.clone();
        tokio::spawn(async move {
            loop {
                match leap_event_rx.recv().await {
                    Ok(event) => {
                        if let Some(ra2_event) = translator::leap_to_ra2(&event, &leap_id_map_clone) {
                            let _ = ra2_event_tx_leap.send(ra2_event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("LEAP event forwarder lagged by {}", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        info!("LEAP backend started ({} zones)", config.zones.len());
        Some(tx)
    } else {
        None
    };

    // Conditionally start Savant client
    let savant_req_tx = if config.has_savant() {
        let savant_cfg = config.savant.as_ref().unwrap();
        let (tx, savant_req_rx) = mpsc::channel::<SavantRequest>(256);
        let (savant_event_tx, mut savant_event_rx) = broadcast::channel::<SavantEvent>(256);

        let savant_host = savant_cfg.host.clone();
        let savant_port = savant_cfg.port;
        let savant_zones = config.savant_zones.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::savant_client::run(savant_host, savant_port, savant_zones, savant_req_rx, savant_event_tx)
                    .await
            {
                tracing::error!("Savant client error: {}", e);
            }
        });

        // Savant event forwarder
        let ra2_event_tx_savant = ra2_event_tx.clone();
        let savant_id_map_clone = savant_id_map.clone();
        tokio::spawn(async move {
            loop {
                match savant_event_rx.recv().await {
                    Ok(event) => {
                        if let Some(ra2_event) = savant_translator::savant_to_ra2(&event, &savant_id_map_clone) {
                            let _ = ra2_event_tx_savant.send(ra2_event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Savant event forwarder lagged by {}", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        info!(
            "Savant backend started ({} zones)",
            config.savant_zones.len()
        );
        Some(tx)
    } else {
        None
    };

    info!("Bridge running");

    // Translation loop
    loop {
        tokio::select! {
            Some(cmd) = ra2_cmd_rx.recv() => {
                let id = match &cmd {
                    Ra2Command::SetOutput { id, .. } => Some(*id),
                    Ra2Command::QueryOutput { id } => Some(*id),
                    Ra2Command::Monitoring { .. } => None,
                };

                if let Some(id) = id {
                    if leap_id_map.ra2_to_leap(id).is_some() {
                        if let Some(ref tx) = leap_req_tx {
                            if let Some(req) = translator::ra2_to_leap(&cmd, &leap_id_map) {
                                if let Err(e) = tx.send(req).await {
                                    warn!("Failed to send LEAP request: {}", e);
                                }
                            }
                        }
                    } else if savant_id_map.ra2_to_savant(id).is_some() {
                        if let Some(ref tx) = savant_req_tx {
                            if let Some(req) = savant_translator::ra2_to_savant(&cmd, &savant_id_map) {
                                if let Err(e) = tx.send(req).await {
                                    warn!("Failed to send Savant request: {}", e);
                                }
                            }
                        }
                    } else {
                        warn!("No backend for ra2_id {}", id);
                    }
                } else {
                    if let Ra2Command::Monitoring { mon_type, enable } = &cmd {
                        info!("Monitoring type {} {}",
                            mon_type, if *enable { "enabled" } else { "disabled" });
                    }
                }
            }
        }
    }
}
