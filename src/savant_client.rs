use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::config::SavantZoneMapping;

#[derive(Debug, Clone)]
pub enum SavantRequest {
    SetLoad {
        address: String,
        load_offset: usize,
        level: f64,
    },
    #[allow(dead_code)]
    QueryLoad {
        address: String,
        load_offset: usize,
    },
}

#[derive(Debug, Clone)]
pub enum SavantEvent {
    LoadLevel {
        address: String,
        load_offset: usize,
        level: f64,
    },
}

/// Run the Savant WebSocket client. Reconnects with exponential backoff.
pub async fn run(
    host: String,
    port: u16,
    zones: Vec<SavantZoneMapping>,
    mut req_rx: mpsc::Receiver<SavantRequest>,
    event_tx: broadcast::Sender<SavantEvent>,
) -> Result<()> {
    let mut backoff = 1u64;
    let max_backoff = 60u64;

    loop {
        match connect_and_run(&host, port, &zones, &mut req_rx, &event_tx).await {
            Ok(()) => {
                info!("Savant connection closed gracefully");
                break;
            }
            Err(e) => {
                error!(
                    "Savant connection error: {}. Reconnecting in {}s...",
                    e, backoff
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
    Ok(())
}

async fn connect_and_run(
    host: &str,
    port: u16,
    zones: &[SavantZoneMapping],
    req_rx: &mut mpsc::Receiver<SavantRequest>,
    event_tx: &broadcast::Sender<SavantEvent>,
) -> Result<()> {
    let url = format!("ws://{}:{}", host, port);
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "savant_protocol".parse().unwrap());

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("Failed to connect to Savant WebSocket")?;

    info!("Connected to Savant host at {}:{}", host, port);
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Step 1: Send session/devicePresent
    let device_present = serde_json::json!({
        "messages": [{
            "protocolVersion": "0.1",
            "device": {
                "name": "Linux",
                "version": "1.0",
                "app": "ra-bridge",
                "ip": "0.0.0.0",
                "model": "ra-bridge"
            }
        }],
        "URI": "session/devicePresent"
    });
    ws_tx
        .send(Message::Text(serde_json::to_string(&device_present)?.into()))
        .await?;
    info!("Sent session/devicePresent");

    // Wait for session/deviceRecognized
    loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    let uri = msg["URI"].as_str().unwrap_or_default();
                    if uri.contains("deviceRecognized") {
                        info!("Savant session established");
                        break;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err(anyhow::anyhow!("Connection closed during handshake"));
            }
            Some(Err(e)) => return Err(e.into()),
            _ => {}
        }
    }

    // Step 2: Request initial state for each unique module address
    let mut seen_addresses = std::collections::HashSet::new();
    for z in zones {
        if seen_addresses.insert(z.address.clone()) {
            let get_state = serde_json::json!({
                "messages": [{}],
                "URI": format!("state/module/{}/get", z.address)
            });
            ws_tx
                .send(Message::Text(serde_json::to_string(&get_state)?.into()))
                .await?;
        }
    }
    info!(
        "Requested initial state for {} modules",
        seen_addresses.len()
    );

    // Enter main loop — poll module state periodically (session/ping and
    // state/register are rejected by this firmware, so we poll instead)
    let poll_interval = tokio::time::Duration::from_secs(30);
    let mut poll_timer = tokio::time::interval(poll_interval);
    poll_timer.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_savant_message(&text, zones, event_tx);
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(anyhow::anyhow!("Savant WebSocket closed"));
                    }
                    Some(Err(e)) => return Err(e.into()),
                    _ => {}
                }
            }
            Some(req) = req_rx.recv() => {
                let msg = encode_request(&req);
                info!("Savant TX: {}", serde_json::to_string(&msg)?);
                ws_tx.send(Message::Text(serde_json::to_string(&msg)?.into())).await?;
            }
            _ = poll_timer.tick() => {
                // Poll all tracked modules for current state
                for addr in &seen_addresses {
                    let get_state = serde_json::json!({
                        "messages": [{}],
                        "URI": format!("state/module/{}/get", addr)
                    });
                    ws_tx.send(Message::Text(serde_json::to_string(&get_state)?.into())).await?;
                }
            }
        }
    }
}

fn handle_savant_message(
    text: &str,
    zones: &[SavantZoneMapping],
    event_tx: &broadcast::Sender<SavantEvent>,
) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let uri = msg["URI"].as_str().unwrap_or_default();

    // Log rejected messages for debugging
    if uri == "messageReject" {
        if let Some(messages) = msg.get("messages").and_then(|m| m.as_array()) {
            for body in messages {
                let rejected_uri = body["URI"].as_str().unwrap_or("?");
                let reason = body["RejectReason"].as_str().unwrap_or("?");
                warn!("Savant rejected {}: {}", rejected_uri, reason);
            }
        }
        return;
    }

    // Handle state/set echo, state/update, and state/module/*/get responses
    if uri == "state/set" || uri.contains("state/update") || uri.contains("state/module/") {
        if let Some(messages) = msg.get("messages").and_then(|m| m.as_array()) {
            for body in messages {
                parse_state_body(body, uri, zones, event_tx);
            }
        }
    }
}

fn parse_state_body(
    body: &serde_json::Value,
    uri: &str,
    zones: &[SavantZoneMapping],
    event_tx: &broadcast::Sender<SavantEvent>,
) {
    let state_str = body.get("state").and_then(|s| s.as_str()).unwrap_or("");

    // Handle "load.XXXX" format (set echo / set confirmation)
    // Reverse the hex key: load_key = (address << 16) | offset
    if let Some(hex_key) = state_str.strip_prefix("load.") {
        if let Ok(load_key) = u32::from_str_radix(hex_key, 16) {
            let int_address = load_key >> 16;
            let load_offset = (load_key & 1023) as usize;
            let address = format!("{:03X}", int_address);

            // Parse level from value: "100%.0" → 100.0, or just "100.0"
            if let Some(value_str) = body.get("value").and_then(|v| v.as_str()) {
                let numeric = value_str.replace('%', "");
                // Take the part before the dot-separated fade value
                let level_str = numeric.split('.').next().unwrap_or(&numeric);
                if let Ok(level) = level_str.trim().parse::<f64>() {
                    info!("Savant set-echo: load.{} → addr={} offset={} level={:.1}%",
                        hex_key, address, load_offset, level);
                    emit_if_tracked(&address, load_offset, level, zones, event_tx);
                }
            }
        }
        return;
    }

    // Handle "module.XXX" format (poll response) or URI-based address
    let address = state_str
        .strip_prefix("module.")
        .map(|s| s.to_string())
        .or_else(|| {
            uri.strip_prefix("state/module/")
                .and_then(|rest| rest.split('/').next())
                .map(|s| s.to_string())
        });

    let address = match address {
        Some(a) => a,
        None => return,
    };

    // Parse CSV value: "100,0,-1,-1,-1,..." (0-100 scale, -1 = unused)
    if let Some(value_str) = body.get("value").and_then(|v| v.as_str()) {
        for (i, val_str) in value_str.split(',').enumerate() {
            if let Ok(level) = val_str.trim().parse::<f64>() {
                if level < 0.0 {
                    continue; // -1 = unused slot
                }
                emit_if_tracked(&address, i, level, zones, event_tx);
            }
        }
    }
}

fn emit_if_tracked(
    address: &str,
    load_offset: usize,
    level: f64,
    zones: &[SavantZoneMapping],
    event_tx: &broadcast::Sender<SavantEvent>,
) {
    // Only emit for zones we actually track
    let tracked = zones
        .iter()
        .find(|z| z.address == address && z.load_offset == load_offset);
    if let Some(z) = tracked {
        info!(
            "Savant RX: addr={} offset={} level={:.1}% (zone {} '{}')",
            address, load_offset, level, z.ra2_id, z.name
        );
        let _ = event_tx.send(SavantEvent::LoadLevel {
            address: address.to_string(),
            load_offset,
            level,
        });
    }
}

fn encode_request(req: &SavantRequest) -> serde_json::Value {
    match req {
        SavantRequest::SetLoad {
            address,
            load_offset,
            level,
        } => {
            // Savant load key: (address_int << 16 | load_offset).toString(16)
            // Matches the web UI's getSetStateValue() formula
            let int_address = u32::from_str_radix(address, 16).unwrap_or(0);
            let load_key = (int_address << 16) | (*load_offset as u32 & 1023);
            let hex_key = format!("{:x}", load_key);

            // Value format: "<level>%.0" (0-100 scale, .0 = instant/no fade)
            serde_json::json!({
                "messages": [{
                    "state": format!("load.{}", hex_key),
                    "value": format!("{}%.0", level.round() as u32)
                }],
                "URI": "state/set"
            })
        }
        SavantRequest::QueryLoad {
            address,
            load_offset: _,
        } => {
            serde_json::json!({
                "messages": [{}],
                "URI": format!("state/module/{}/get", address)
            })
        }
    }
}
