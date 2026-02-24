use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::config::{SavantConfig, SavantZoneMapping};

/// Discover Savant devices and loads via WebSocket, returning config and zone mappings.
pub async fn discover_zones(
    host: &str,
    port: u16,
    start_id: u32,
) -> Result<(SavantConfig, Vec<SavantZoneMapping>)> {
    let url = format!("ws://{}:{}", host, port);
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "savant_protocol".parse().unwrap());

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("Failed to connect to Savant WebSocket")?;

    info!("Connected to Savant at {}:{} for discovery", host, port);
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send session/devicePresent
    let device_present = serde_json::json!({
        "messages": [{
            "protocolVersion": "0.1",
            "device": {
                "name": "Linux",
                "version": "1.0",
                "app": "ra-bridge-discover",
                "ip": "0.0.0.0",
                "model": "ra-bridge"
            }
        }],
        "URI": "session/devicePresent"
    });
    ws_tx
        .send(Message::Text(serde_json::to_string(&device_present)?.into()))
        .await?;

    // Wait for session acknowledgment
    let timeout = tokio::time::Duration::from_secs(10);
    tokio::time::timeout(timeout, async {
        loop {
            match ws_rx.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                        let uri = msg["URI"].as_str().unwrap_or_default();
                        if uri.contains("deviceRecognized") {
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
            Ok::<(), anyhow::Error>(())
                .ok();
        }
        Ok(())
    })
    .await
    .context("Timeout waiting for Savant session")??;

    info!("Savant session established, requesting device config");

    // Request lighting device configuration
    let get_config = serde_json::json!({
        "messages": [{}],
        "URI": "lighting/config/device/get"
    });
    ws_tx
        .send(Message::Text(serde_json::to_string(&get_config)?.into()))
        .await?;

    // Collect responses for a few seconds
    let mut zones = Vec::new();
    let mut ra2_id = start_id;

    let collect_timeout = tokio::time::Duration::from_secs(10);
    let _ = tokio::time::timeout(collect_timeout, async {
        loop {
            match ws_rx.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                        let uri = msg["URI"].as_str().unwrap_or_default();
                        if uri.contains("lighting/config") || uri.contains("device") {
                            if let Some(messages) = msg.get("messages").and_then(|m| m.as_array()) {
                                for body in messages {
                                    parse_device_config(body, &mut zones, &mut ra2_id);
                                }
                            }
                            // Got a config response, we can finish
                            if !zones.is_empty() {
                                break;
                            }
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            }
        }
    })
    .await;

    // If we didn't get config via the config endpoint, try getting state from known modules
    if zones.is_empty() {
        info!("No config endpoint response, attempting state-based discovery");
        // Try requesting all module states
        for addr_num in 1..=20u32 {
            let addr = format!("{:03}", addr_num);
            let get_state = serde_json::json!({
                "messages": [{}],
                "URI": format!("state/module/{}/get", addr)
            });
            ws_tx
                .send(Message::Text(serde_json::to_string(&get_state)?.into()))
                .await?;
        }

        let state_timeout = tokio::time::Duration::from_secs(5);
        let _ = tokio::time::timeout(state_timeout, async {
            loop {
                match ws_rx.next().await {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                            let uri = msg["URI"].as_str().unwrap_or_default();
                            if uri.contains("state/module/") {
                                if let Some(messages) = msg.get("messages").and_then(|m| m.as_array()) {
                                    for body in messages {
                                        parse_state_discovery(uri, body, &mut zones, &mut ra2_id);
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        })
        .await;
    }

    // Close connection
    let _ = ws_tx.send(Message::Close(None)).await;

    let savant_config = SavantConfig {
        host: host.to_string(),
        port,
    };

    info!("Savant discovery complete: {} zones found", zones.len());
    Ok((savant_config, zones))
}

fn parse_device_config(
    body: &serde_json::Value,
    zones: &mut Vec<SavantZoneMapping>,
    ra2_id: &mut u32,
) {
    // If body contains a "devices" array, process each device
    if let Some(devices) = body.get("devices").and_then(|d| d.as_array()) {
        for device in devices {
            parse_single_device(device, zones, ra2_id);
        }
        return;
    }
    // Otherwise treat the body itself as a device object
    parse_single_device(body, zones, ra2_id);
}

fn parse_single_device(
    device: &serde_json::Value,
    zones: &mut Vec<SavantZoneMapping>,
    ra2_id: &mut u32,
) {
    // Get module address: try explicit address fields, then convert id to 3-digit hex
    let address = if let Some(addr) = device["address"]
        .as_str()
        .or_else(|| device["moduleAddress"].as_str())
    {
        addr.to_string()
    } else if let Some(id_str) = device["id"].as_str() {
        if let Ok(id_num) = id_str.parse::<u32>() {
            format!("{:03X}", id_num)
        } else {
            return;
        }
    } else if let Some(id_num) = device["id"].as_u64() {
        format!("{:03X}", id_num)
    } else {
        return;
    };

    let room = device["room"]
        .as_str()
        .or_else(|| device["area"].as_str())
        .or_else(|| device["location"].as_str())
        .unwrap_or("Unknown Room")
        .to_string();

    // Try "load" (documented) or "loads" (fallback) array within device
    let loads = device
        .get("load")
        .and_then(|l| l.as_array())
        .or_else(|| device.get("loads").and_then(|l| l.as_array()));

    if let Some(loads) = loads {
        for (i, load) in loads.iter().enumerate() {
            let load_name = load["name"]
                .as_str()
                .or_else(|| load["label"].as_str())
                .unwrap_or(&format!("Load {}", i + 1))
                .to_string();

            let name = format!("{} \u{2500} {}", room, load_name);
            info!("  [{}] {} (addr={}, load={})", ra2_id, name, address, i);

            zones.push(SavantZoneMapping {
                ra2_id: *ra2_id,
                address: address.to_string(),
                load_offset: i,
                name,
                room: room.clone(),
            });
            *ra2_id += 1;
        }
    } else {
        // Single-load device
        let name = format!(
            "{} \u{2500} {}",
            room,
            device["name"].as_str().unwrap_or("Light")
        );
        info!("  [{}] {} (addr={}, load=0)", ra2_id, name, address);

        zones.push(SavantZoneMapping {
            ra2_id: *ra2_id,
            address: address.to_string(),
            load_offset: 0,
            name,
            room,
        });
        *ra2_id += 1;
    }
}

fn parse_state_discovery(
    url: &str,
    body: &serde_json::Value,
    zones: &mut Vec<SavantZoneMapping>,
    ra2_id: &mut u32,
) {
    // Extract address from URL: "state/module/001/get"
    let address = match url
        .strip_prefix("state/module/")
        .and_then(|rest| rest.split('/').next())
    {
        Some(addr) => addr.to_string(),
        None => return,
    };

    // Count loads from state CSV or loads array
    let load_count = if let Some(state_str) = body.get("state").and_then(|s| s.as_str()) {
        state_str.split(',').count()
    } else if let Some(loads) = body.get("loads").and_then(|l| l.as_array()) {
        loads.len()
    } else {
        return; // No state data, module probably doesn't exist
    };

    if load_count == 0 {
        return;
    }

    let room = body
        .get("room")
        .or_else(|| body.get("area"))
        .and_then(|r| r.as_str())
        .unwrap_or("Unknown Room")
        .to_string();

    for i in 0..load_count {
        let load_name = if let Some(loads) = body.get("loads").and_then(|l| l.as_array()) {
            loads
                .get(i)
                .and_then(|l| l.get("name").and_then(|n| n.as_str()))
                .unwrap_or(&format!("Load {}", i + 1))
                .to_string()
        } else {
            format!("Load {}", i + 1)
        };

        let name = format!("{} \u{2500} {}", room, load_name);
        warn!(
            "  [{}] {} (addr={}, load={}) — discovered via state probe",
            ra2_id, name, address, i
        );

        zones.push(SavantZoneMapping {
            ra2_id: *ra2_id,
            address: address.clone(),
            load_offset: i,
            name,
            room: room.clone(),
        });
        *ra2_id += 1;
    }
}
