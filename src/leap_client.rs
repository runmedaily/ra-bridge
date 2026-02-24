use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_rustls::TlsConnector;
use tracing::{error, info, warn};

/// TLS certificate verifier that validates the chain but skips hostname checking.
/// Lutron processors use DNS-based names in their certs (e.g. "radiora3-xxxx-server")
/// but are connected to by IP address.
#[derive(Debug)]
pub struct NoHostnameVerification {
    inner: Arc<WebPkiServerVerifier>,
}

impl NoHostnameVerification {
    pub fn new(root_store: Arc<rustls::RootCertStore>) -> Result<Self> {
        let inner = WebPkiServerVerifier::builder(root_store)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build WebPki verifier: {}", e))?;
        Ok(Self { inner })
    }
}

impl ServerCertVerifier for NoHostnameVerification {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        match self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Ok(v) => Ok(v),
            Err(ref e) if e.to_string().contains("not valid for name") => {
                Ok(ServerCertVerified::assertion())
            }
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

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

    let verifier = Arc::new(
        NoHostnameVerification::new(Arc::new(root_store))
            .context("Failed to build LEAP cert verifier")?,
    );

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(client_certs, client_key)
        .context("Failed to build LEAP TLS config")?;

    Ok(TlsConnector::from(Arc::new(config)))
}

/// Send a single LEAP request and return the response. Connects, sends, reads one line, disconnects.
pub async fn one_shot_request(
    host: &str,
    port: u16,
    certs_dir: &Path,
    request: &LeapRequest,
) -> Result<LeapEvent> {
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

    let (reader, mut writer) = tokio::io::split(tls);
    let mut reader = tokio::io::BufReader::new(reader);

    let mut msg = serde_json::to_string(request)?;
    msg.push_str("\r\n");
    writer.write_all(msg.as_bytes()).await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let event: LeapEvent = serde_json::from_str(line.trim())
        .with_context(|| format!("Failed to parse LEAP response: {}", line.trim()))?;

    Ok(event)
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

    let mut line = String::new();
    let ping_interval = tokio::time::Duration::from_secs(15);
    let mut ping_timer = tokio::time::interval(ping_interval);
    ping_timer.tick().await; // consume the immediate first tick

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
            // Keepalive ping every 15s
            _ = ping_timer.tick() => {
                let ping = serde_json::json!({
                    "CommuniqueType": "ReadRequest",
                    "Header": {"Url": "/server/1/status/ping"}
                });
                let mut msg = serde_json::to_string(&ping)?;
                msg.push_str("\r\n");
                writer.write_all(msg.as_bytes()).await?;
            }
        }
    }
}
