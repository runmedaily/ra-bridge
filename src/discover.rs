use std::path::Path;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::config::{Config, ProcessorConfig, TelnetConfig, ZoneMapping};
use crate::leap_client::{LeapHeader, LeapRequest};

/// Query the processor for all areas and their zones, returning mappings with sequential RA2 IDs.
pub async fn discover_zones(
    host: &str,
    port: u16,
    certs_dir: &Path,
) -> Result<Vec<ZoneMapping>> {
    // Fetch all areas
    let area_req = LeapRequest {
        communique_type: "ReadRequest".into(),
        header: LeapHeader {
            url: "/area".into(),
            client_tag: None,
            extra: Default::default(),
        },
        body: None,
    };

    let area_resp = crate::leap_client::one_shot_request(host, port, certs_dir, &area_req)
        .await
        .context("Failed to read /area")?;

    let areas = area_resp.body["Areas"]
        .as_array()
        .context("Response body missing 'Areas' array")?;

    let mut zones = Vec::new();
    let mut ra2_id: u32 = 1;

    for area in areas {
        let area_href = area["href"].as_str().unwrap_or_default();
        let area_name = area["Name"].as_str().unwrap_or("Unknown Area");

        if area_href.is_empty() {
            continue;
        }

        // Fetch zones for this area
        let zone_url = format!("{}/associatedzone", area_href);
        let zone_req = LeapRequest {
            communique_type: "ReadRequest".into(),
            header: LeapHeader {
                url: zone_url.clone(),
                client_tag: None,
                extra: Default::default(),
            },
            body: None,
        };

        let zone_resp = match crate::leap_client::one_shot_request(host, port, certs_dir, &zone_req).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Failed to read {}: {}", zone_url, e);
                continue;
            }
        };

        let zone_array = match zone_resp.body["Zones"].as_array() {
            Some(arr) => arr,
            None => continue,
        };

        for zone in zone_array {
            let zone_href = zone["href"].as_str().unwrap_or_default();
            let zone_name = zone["Name"].as_str().unwrap_or("Unknown Zone");

            if zone_href.is_empty() {
                continue;
            }

            let name = format!("{} \u{2500} {}", area_name, zone_name);
            info!("  [{}] {} → {}", ra2_id, name, zone_href);

            zones.push(ZoneMapping {
                ra2_id,
                leap_href: zone_href.to_string(),
                name,
            });
            ra2_id += 1;
        }
    }

    Ok(zones)
}

/// Write the config file. Backs up existing file to `.bak` if present.
pub fn write_config(
    path: &Path,
    host: &str,
    port: u16,
    zones: &[ZoneMapping],
) -> Result<()> {
    if path.exists() {
        let bak = path.with_extension("toml.bak");
        std::fs::copy(path, &bak)
            .with_context(|| format!("Failed to back up {} to {}", path.display(), bak.display()))?;
        warn!("Backed up existing config to {}", bak.display());
    }

    let config = Config {
        processor: ProcessorConfig {
            host: host.to_string(),
            leap_port: port,
        },
        telnet: TelnetConfig::default(),
        zones: zones.to_vec(),
    };

    let toml_str = toml::to_string_pretty(&config).context("Failed to serialize config")?;
    std::fs::write(path, &toml_str)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(())
}
