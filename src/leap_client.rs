use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_rustls::TlsConnector;
use tracing::{error, info, warn};

/// A LEAP request to send to the processor.
#[derive(Debug, Clone, Serialize)]
pub struct LeapRequest {
    #[serde(rename = "CommuniqueType")]
    pub communique_type: String,
    #[serde(rename = "Header")]
    pub header: LeapHeader,
    #[serde(rename = "Body", skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeapHeader {
    #[serde(rename = "Url")]
    pub url: String,
    #[serde(rename = "ClientTag", skip_serializing_if = "Option::is_none")]
    pub client_tag: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A LEAP event received from the processor.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct LeapEvent {
    #[serde(rename = "CommuniqueType")]
    pub communique_type: String,
    #[serde(rename = "Header")]
    pub header: LeapEventHeader,
    #[serde(rename = "Body", default)]
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct LeapEventHeader {
    #[serde(rename = "Url", default)]
    pub url: String,
    #[serde(rename = "StatusCode", default)]
    pub status_code: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Build a TLS connector using certificates from the certs directory.
pub fn build_leap_tls_connector(certs_dir: &Path) -> Result<TlsConnector> {
    let ca_path = certs_dir.join("ca.crt");
    let cert_path = certs_dir.join("ra-bridge.crt");
    let key_path = certs_dir.join("ra-bridge.key");

    let mut root_store = rustls::RootCertStore::empty();
    let ca_pem = std::fs::read(&ca_path)
        .with_context(|| format!("Failed to read CA cert: {}", ca_path.display()))?;
    let mut reader = BufReader::new(ca_pem.as_slice());
    let ca_certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
    for cert in ca_certs {
        root_store.add(cert)?;
    }

    // Also add well-known Lutron CAs as fallback trust anchors
    let extra_cas = [
        crate::leap_pairing::LUTRON_ROOT_CA_PEM,
        crate::leap_pairing::LAP_CA_PEM,
    ];
    for ca in extra_cas {
        let mut r = BufReader::new(ca.as_bytes());
        if let Ok(certs) = rustls_pemfile::certs(&mut r).collect::<Result<Vec<_>, _>>() {
            for cert in certs {
                let _ = root_store.add(cert);
            }
        }
    }

    let cert_pem = std::fs::read(&cert_path)
        .with_context(|| format!("Failed to read client cert: {}", cert_path.display()))?;
    let mut cert_reader = BufReader::new(cert_pem.as_slice());
    let client_certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    let key_pem = std::fs::read(&key_path)
        .with_context(|| format!("Failed to read client key: {}", key_path.display()))?;

    // Try PKCS8 first, then fall back to RSA PKCS1
    let client_key = {
        let mut r = BufReader::new(key_pem.as_slice());
        let keys: Vec<_> = rustls_pemfile::pkcs8_private_keys(&mut r).collect();
        if let Some(Ok(key)) = keys.into_iter().next() {
            rustls::pki_types::PrivateKeyDer::Pkcs8(key)
        } else {
            let mut r = BufReader::new(key_pem.as_slice());
            let keys: Vec<_> = rustls_pemfile::rsa_private_keys(&mut r).collect();
            let rsa_key = keys
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("No private key found in {}", key_path.display()))?
                .context("Failed to parse private key")?;
            rustls::pki_types::PrivateKeyDer::Pkcs1(rsa_key)
        }
    };

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_certs, client_key)
        .context("Failed to build LEAP TLS config")?;

    Ok(TlsConnector::from(Arc::new(config)))
}

/// Run the LEAP client. Sends requests from `req_rx`, publishes events on `event_tx`.
/// Reconnects with exponential backoff on disconnect.
pub async fn run(
    host: String,
    port: u16,
    certs_dir: std::path::PathBuf,
    mut req_rx: mpsc::Receiver<LeapRequest>,
    event_tx: broadcast::Sender<LeapEvent>,
) -> Result<()> {
    let mut backoff = 1u64;
    let max_backoff = 60u64;

    loop {
        match connect_and_run(&host, port, &certs_dir, &mut req_rx, &event_tx).await {
            Ok(()) => {
                info!("LEAP connection closed gracefully");
                break;
            }
            Err(e) => {
                error!("LEAP connection error: {}. Reconnecting in {}s...", e, backoff);
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
    certs_dir: &Path,
    req_rx: &mut mpsc::Receiver<LeapRequest>,
    event_tx: &broadcast::Sender<LeapEvent>,
) -> Result<()> {
    let connector = build_leap_tls_connector(certs_dir)?;
    let tcp = TcpStream::connect((host, port)).await?;
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| {
            rustls::pki_types::ServerName::IpAddress(
                host.parse::<std::net::IpAddr>()
                    .expect("Invalid host address")
                    .into(),
            )
        });
    let tls = connector.connect(server_name, tcp).await?;
    info!("Connected to LEAP processor at {}:{}", host, port);

    let (reader, mut writer) = tokio::io::split(tls);
    let mut reader = tokio::io::BufReader::new(reader);

    // Subscribe to zone status events
    let subscribe = serde_json::json!({
        "CommuniqueType": "SubscribeRequest",
        "Header": {"Url": "/zone/status"}
    });
    let mut msg = serde_json::to_string(&subscribe)?;
    msg.push_str("\r\n");
    writer.write_all(msg.as_bytes()).await?;
    info!("Subscribed to zone status events");

    // Reset backoff on successful connection
    let mut line = String::new();

    loop {
        tokio::select! {
            // Read events from processor
            result = reader.read_line(&mut line) => {
                let n = result?;
                if n == 0 {
                    return Err(anyhow::anyhow!("LEAP connection closed"));
                }
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    match serde_json::from_str::<LeapEvent>(trimmed) {
                        Ok(event) => {
                            let _ = event_tx.send(event);
                        }
                        Err(e) => {
                            warn!("Failed to parse LEAP event: {} — line: {}", e, trimmed);
                        }
                    }
                }
                line.clear();
            }
            // Send requests to processor
            Some(req) = req_rx.recv() => {
                let mut msg = serde_json::to_string(&req)?;
                msg.push_str("\r\n");
                writer.write_all(msg.as_bytes()).await?;
            }
        }
    }
}
