use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

use crate::id_map::IdMap;
use crate::leap_client::{LeapEvent, LeapRequest};
use crate::ra2_protocol::{Ra2Command, Ra2Event};
use crate::translator;

/// Run the bridge, wiring telnet ↔ translator ↔ LEAP.
pub async fn run(
    config: crate::config::Config,
    certs_dir: std::path::PathBuf,
) -> Result<()> {
    let id_map = Arc::new(IdMap::from_zones(&config.zones));

    // Channels: telnet → bridge (RA2 commands)
    let (ra2_cmd_tx, mut ra2_cmd_rx) = mpsc::channel::<Ra2Command>(256);

    // Channels: bridge → telnet (RA2 events, broadcast to all clients)
    let (ra2_event_tx, _) = broadcast::channel::<Ra2Event>(256);

    // Channels: bridge → LEAP (LEAP requests)
    let (leap_req_tx, leap_req_rx) = mpsc::channel::<LeapRequest>(256);

    // Channels: LEAP → bridge (LEAP events)
    let (leap_event_tx, mut leap_event_rx) = broadcast::channel::<LeapEvent>(256);

    // Start telnet server
    let telnet_event_tx = ra2_event_tx.clone();
    let telnet_port = config.telnet.port;
    tokio::spawn(async move {
        if let Err(e) = crate::telnet_server::run(telnet_port, ra2_cmd_tx, telnet_event_tx).await {
            tracing::error!("Telnet server error: {}", e);
        }
    });

    // Start LEAP client
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

    info!("Bridge running");

    // Translation loop: forward between RA2 and LEAP
    loop {
        tokio::select! {
            // RA2 command from telnet → translate → send to LEAP
            Some(cmd) = ra2_cmd_rx.recv() => {
                if let Some(req) = translator::ra2_to_leap(&cmd, &id_map) {
                    if let Err(e) = leap_req_tx.send(req).await {
                        warn!("Failed to send LEAP request: {}", e);
                    }
                } else {
                    match &cmd {
                        Ra2Command::Monitoring { mon_type, enable } => {
                            info!("Monitoring type {} {}", mon_type,
                                if *enable { "enabled" } else { "disabled" });
                        }
                        _ => {
                            warn!("No LEAP translation for command: {:?}", cmd);
                        }
                    }
                }
            }
            // LEAP event → translate → broadcast to telnet clients
            Ok(event) = leap_event_rx.recv() => {
                if let Some(ra2_event) = translator::leap_to_ra2(&event, &id_map) {
                    let _ = ra2_event_tx.send(ra2_event);
                }
            }
        }
    }
}
