use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use rsa::pkcs8::EncodePrivateKey;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{info, warn};

const PAIRING_PORT: u16 = 8083;
const LEAP_PORT: u16 = 8081;
const BUTTON_TIMEOUT_SECS: u64 = 180;

// Lutron LAP CA certificate (Caseta Local Access Protocol Cert Authority)
pub(crate) const LAP_CA_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIEsjCCA5qgAwIBAgIBATANBgkqhkiG9w0BAQ0FADCBlzELMAkGA1UEBhMCVVMx
FTATBgNVBAgTDFBlbm5zeWx2YW5pYTElMCMGA1UEChMcTHV0cm9uIEVsZWN0cm9u
aWNzIENvLiwgSW5jLjEUMBIGA1UEBxMLQ29vcGVyc2J1cmcxNDAyBgNVBAMTK0Nh
c2V0YSBMb2NhbCBBY2Nlc3MgUHJvdG9jb2wgQ2VydCBBdXRob3JpdHkwHhcNMTUx
MDMxMDAwMDAwWhcNMzUxMDMxMDAwMDAwWjCBlzELMAkGA1UEBhMCVVMxFTATBgNV
BAgTDFBlbm5zeWx2YW5pYTElMCMGA1UEChMcTHV0cm9uIEVsZWN0cm9uaWNzIENv
LiwgSW5jLjEUMBIGA1UEBxMLQ29vcGVyc2J1cmcxNDAyBgNVBAMTK0Nhc2V0YSBM
b2NhbCBBY2Nlc3MgUHJvdG9jb2wgQ2VydCBBdXRob3JpdHkwggEiMA0GCSqGSIb3
DQEBAQUAA4IBDwAwggEKAoIBAQDamUREO0dENJxvxdbsDATdDFq+nXdbe62XJ4hI
t15nrUolwv7S28M/6uPPFtRSJW9mwvk/OKDlz0G2D3jw6SdzV3I7tNzvDptvbAL2
aDy9YNp9wTub/pLF6ONDa56gfAxsPQnMBwgoZlKqNQQsjykiyBv8FX42h3Nsa+Bl
q3hjnZEdOAkdn0rvCWD605c0+VWWOWm2vv7bwyOsfgsvCPxooAyBhTDeA0JPjVE/
wHPfiDF3WqA8JzWv4Ibvkg1g33oD6lG8LulWKDS9TPBYF+cvJ40aFPMreMoAQcrX
uD15vaS7iWXKI+anVrBpqE6pRkwLhR+moFjv5GZ+9oP8eawzAgMBAAGjggEFMIIB
ATAMBgNVHRMEBTADAQH/MB0GA1UdDgQWBBSB7qznOajKywOtZypVvV7ECAsgZjCB
xAYDVR0jBIG8MIG5gBSB7qznOajKywOtZypVvV7ECAsgZqGBnaSBmjCBlzELMAkG
A1UEBhMCVVMxFTATBgNVBAgTDFBlbm5zeWx2YW5pYTElMCMGA1UEChMcTHV0cm9u
IEVsZWN0cm9uaWNzIENvLiwgSW5jLjEUMBIGA1UEBxMLQ29vcGVyc2J1cmcxNDAy
BgNVBAMTK0Nhc2V0YSBMb2NhbCBBY2Nlc3MgUHJvdG9jb2wgQ2VydCBBdXRob3Jp
dHmCAQEwCwYDVR0PBAQDAgG+MA0GCSqGSIb3DQEBDQUAA4IBAQB9UDVi2DQI7vHp
F2Lape8SCtcdGEY/7BV4a3F+Xp9WxpE4bVtwoHlb+HG4tYQk9LO7jReE3VBmzvmU
aj+Y3xa25PSb+/q6U6MuY5OscyWo6ZGwtlsrWcP5xsey950WLwW6i8mfIkqFf6uT
gPbUjLsOstB4p7PQVpFgS2rP8h50Psue+XtUKRpR+JSBrHXKX9VuU/aM4PYexSvF
WSHa2HEbjvp6ccPm53/9/EtOtzcUMNspKt3YzABAoQ5/69nebRtC5lWjFI0Ga6kv
zKyu/aZJXWqskHkMz+Mbnky8tP37NmVkMnmRLCfdCG0gHiq/C2tjWDfPQID6HY0s
zq38av5E
-----END CERTIFICATE-----"#;

// LAP client certificate (Caseta Application)
pub(crate) const LAP_CERT_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIECjCCAvKgAwIBAgIBAzANBgkqhkiG9w0BAQ0FADCBlzELMAkGA1UEBhMCVVMx
FTATBgNVBAgTDFBlbm5zeWx2YW5pYTElMCMGA1UEChMcTHV0cm9uIEVsZWN0cm9u
aWNzIENvLiwgSW5jLjEUMBIGA1UEBxMLQ29vcGVyc2J1cmcxNDAyBgNVBAMTK0Nh
c2V0YSBMb2NhbCBBY2Nlc3MgUHJvdG9jb2wgQ2VydCBBdXRob3JpdHkwHhcNMTUx
MDMxMDAwMDAwWhcNMzUxMDMxMDAwMDAwWjB+MQswCQYDVQQGEwJVUzEVMBMGA1UE
CBMMUGVubnN5bHZhbmlhMSUwIwYDVQQKExxMdXRyb24gRWxlY3Ryb25pY3MgQ28u
LCBJbmMuMRQwEgYDVQQHEwtDb29wZXJzYnVyZzEbMBkGA1UEAxMSQ2FzZXRhIEFw
cGxpY2F0aW9uMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAyAOELqTw
WNkF8ofSYJ9QkOHAYMmkVSRjVvZU2AqFfaZYCfWLoors7EBeQrsuGyojqxCbtRUd
l2NQrkPrGVw9cp4qsK54H8ntVadNsYi7KAfDW8bHQNf3hzfcpe8ycXcdVPZram6W
pM9P7oS36jV2DLU59A/OGkcO5AkC0v5ESqzab3qaV3ZvELP6qSt5K4MaJmm8lZT2
6deHU7Nw3kR8fv41qAFe/B0NV7IT+hN+cn6uJBxG5IdAimr4Kl+vTW9tb+/Hh+f+
pQ8EzzyWyEELRp2C72MsmONarnomei0W7dVYbsgxUNFXLZiXBdtNjPCMv1u6Znhm
QMIu9Fhjtz18LwIDAQABo3kwdzAJBgNVHRMEAjAAMB0GA1UdDgQWBBTiN03yqw/B
WK/jgf6FNCZ8D+SgwDAfBgNVHSMEGDAWgBSB7qznOajKywOtZypVvV7ECAsgZjAL
BgNVHQ8EBAMCBaAwHQYDVR0lBBYwFAYIKwYBBQUHAwEGCCsGAQUFBwMCMA0GCSqG
SIb3DQEBDQUAA4IBAQABdgPkGvuSBCwWVGO/uzFEIyRius/BF/EOZ7hMuZluaF05
/FT5PYPWg+UFPORUevB6EHyfezv+XLLpcHkj37sxhXdDKB4rrQPNDY8wzS9DAqF4
WQtGMdY8W9z0gDzajrXRbXkYLDEXnouUWA8+AblROl1Jr2GlUsVujI6NE6Yz5JcJ
zDLVYx7pNZkhYcmEnKZ30+ICq6+0GNKMW+irogm1WkyFp4NHiMCQ6D2UMAIMfeI4
xsamcaGquzVMxmb+Py8gmgtjbpnO8ZAHV6x3BG04zcaHRDOqyA4g+Xhhbxp291c8
B31ZKg0R+JaGyy6ZpE5UPLVyUtLlN93V2V8n66kR
-----END CERTIFICATE-----"#;

// LAP client private key
pub(crate) const LAP_KEY_PEM: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEAyAOELqTwWNkF8ofSYJ9QkOHAYMmkVSRjVvZU2AqFfaZYCfWL
oors7EBeQrsuGyojqxCbtRUdl2NQrkPrGVw9cp4qsK54H8ntVadNsYi7KAfDW8bH
QNf3hzfcpe8ycXcdVPZram6WpM9P7oS36jV2DLU59A/OGkcO5AkC0v5ESqzab3qa
V3ZvELP6qSt5K4MaJmm8lZT26deHU7Nw3kR8fv41qAFe/B0NV7IT+hN+cn6uJBxG
5IdAimr4Kl+vTW9tb+/Hh+f+pQ8EzzyWyEELRp2C72MsmONarnomei0W7dVYbsgx
UNFXLZiXBdtNjPCMv1u6ZnhmQMIu9Fhjtz18LwIDAQABAoIBAQCXDtDNyZQcBgwP
17RzdN8MDPOWJbQO+aRtES2S3J9k/jSPkPscj3/QDe0iyOtRaMn3cFuor4HhzAgr
FPCB/sAJyJrFRX9DwuWUQv7SjkmLOhG5Rq9FsdYoMXBbggO+3g8xE8qcX1k2r7vW
kDW2lRnLDzPtt+IYxoHgh02yvIYnPn1VLuryM0+7eUrTVmdHQ1IGS5RRAGvtoFjf
4QhkkwLzZzCBly/iUDtNiincwRx7wUG60c4ZYu/uBbdJKT+8NcDLnh6lZyJIpGns
jjZvvYA9kgCB2QgQ0sdvm0rA31cbc72Y2lNdtE30DJHCQz/K3X7T0PlfR191NMiX
E7h2I/oBAoGBAPor1TqsQK0tT5CftdN6j49gtHcPXVoJQNhPyQldKXADIy8PVGnn
upG3y6wrKEb0w8BwaZgLAtqOO/TGPuLLFQ7Ln00nEVsCfWYs13IzXjCCR0daOvcF
3FCb0IT/HHym3ebtk9gvFY8Y9AcV/GMH5WkAufWxAbB7J82M//afSghPAoGBAMys
g9D0FYO/BDimcBbUBpGh7ec+XLPaB2cPM6PtXzMDmkqy858sTNBLLEDLl+B9yINi
FYcxpR7viNDAWtilVGKwkU3hM514k+xrEr7jJraLzd0j5mjp55dnmH0MH0APjEV0
qum+mIJmWXlkfKKIiIDgr6+FwIiF5ttSbX1NwnYhAoGAMRvjqrXfqF8prEk9xzra
7ZldM7YHbEI+wXfADh+En+FtybInrvZ3UF2VFMIQEQXBW4h1ogwfTkn3iRBVje2x
v4rHRbzykjwF48XPsTJWPg2E8oPK6Wz0F7rOjx0JOYsEKm3exORRRhru5Gkzdzk4
lok29/z8SOmUIayZHo+cV88CgYEAgPsmhoOLG19A9cJNWNV83kHBfryaBu0bRSMb
U+6+05MtpG1pgaGVNp5o4NxsdZhOyB0DnBL5D6m7+nF9zpFBwH+s0ftdX5sg/Rfs
1Eapmtg3f2ikRvFAdPVf7024U9J4fzyqiGsICQUe1ZUxxetsumrdzCrpzh80AHrN
bO2X4oECgYEAxoVXNMdFH5vaTo3X/mOaCi0/j7tOgThvGh0bWcRVIm/6ho1HXk+o
+kY8ld0vCa7VvqT+iwPt+7x96qesVPyWQN3+uLz9oL3hMOaXCpo+5w8U2Qxjinod
uHnNjMTXCVxNy4tkARwLRwI+1aV5PMzFSi+HyuWmBaWOe19uz3SFbYs=
-----END RSA PRIVATE KEY-----"#;

// Lutron Root CA (for RA3 processors that don't use the Caseta LAP CA)
pub(crate) const LUTRON_ROOT_CA_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIH5DCCBMygAwIBAgIJAKk++JqaJetSMA0GCSqGSIb3DQEBCwUAMH8xCzAJBgNV
BAYTAlVTMQswCQYDVQQIDAJQQTELMAkGA1UEBwwCQ0IxHzAdBgNVBAoMFkx1dHJv
biBFbGVjdHJvbmljcyBJbmMxHzAdBgNVBAsMFkx1dHJvbiBFbGVjdHJvbmljcyBJ
bmMxFDASBgNVBAMMC2x1dHJvbi1yb290MB4XDTE2MDkyODE5NTk0MVoXDTM2MDky
MzE5NTk0MVowfzELMAkGA1UEBhMCVVMxCzAJBgNVBAgMAlBBMQswCQYDVQQHDAJD
QjEfMB0GA1UECgwWTHV0cm9uIEVsZWN0cm9uaWNzIEluYzEfMB0GA1UECwwWTHV0
cm9uIEVsZWN0cm9uaWNzIEluYzEUMBIGA1UEAwwLbHV0cm9uLXJvb3QwggMiMA0G
CSqGSIb3DQEBAQUAA4IDDwAwggMKAoIDAQDBZbMODMzm+qpsOF5hhQ272GUlOaKz
n5b5YxokSAoxY4TqQApb9/uRHIBuuGLntq0QhR0Y3b0lXBeJWzWC6zscZJrheUKW
+2aHVvU4ugPAAXK/WVI68adBSY1UP0BcO1paYrXONcuXQgdy2/GV1mo1b+bmjNFT
zeDopkUoBxivBDZZ7B5vFfbJSgSF47Xsz8cspCEUIaV1rZbaDYBzsimdvrKusJfZ
Pci+Cx71sZuKunGTCgwHduYFsBfYRgTG1ihNEASi2++Er67AcabUGaqVQr/kIrUD
sS9jB6uaqPgMajjwXiZPDm82tTHobbKSav7aq+kSBNIFyvhK5y+vAWoGeZr5WK7n
9EekO3x7LXc6XSCASuhzK6zquAGUBSQNEO3c7sZ1rIdNs1lBSkCSxs+Bl8eEHO8k
O20TqKzKF9bQtccNkFWtRKIhVLFxQt234P+XJtWvWKVOlkLCAo0QgDivFJQVnNKM
Hr2/CIsOLC+ZSWAYl0lZEJaszt7wjR9cc7DRizq9aoKcGlPRvxzobFoQ4H0Z8vIR
DQRUQWFaTTOGiEk7JKxqjXX8xuGZpoXWw8VX0gz3Y0Bz8sU58ZZbugmVjvnKKYzd
ueZ/9+FsaYX6CKdJDANEJf+fqfkGXwQGt8Ns7SeG0JyCdJ4K2ECoOURYS4P1vSY1
40L9OldDjsW2qhpSBPHppfJ4rPRUu5J9Ux3AX4Mz+ibl6MS3wRpP1Rg+9TLITK5o
3AYrJO6oMsYrQkQvc+k20ocD7Iq0522iyw62/DpKMsPZXHNTT/rqzIihkaZaR8aa
ZOgAKi5o398mcfsuv42f8DriYc0Gr+3btiDU7rINqM935YNIABBDtVT9Ybc5uPHa
wXLmAIx2yLjqYaRDhr01Sql6WGy8Y0HcI5lM1pw4Vpx+VKWG/QdORGtZgKySGZ0+
9bY9cRN9IBFz4J60xoqx0MsM5o6FqVDypDCB32KaobVZSAnHifwEGtJJimNIzHpY
jGCTzBHSpuZcvV2dVAuPTHzck37ifpNTUFUCAwEAAaNjMGEwHQYDVR0OBBYEFErb
2SmGkh+4kYe2twSie5+xaqRsMB8GA1UdIwQYMBaAFErb2SmGkh+4kYe2twSie5+x
aqRsMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgGGMA0GCSqGSIb3DQEB
CwUAA4IDAQAP9t0STrn0xENQEDNwRMQUBYTA/r7BmzXX6b0Ip6HW7QNmTkFc5pUb
KT86v6g8cJ7ls96JwdTu1kRQt4Qpbyvzp2p5YlRFnm3NTVVdcffcZNo95x9Z/1Cv
5xZgw0OKODwPBJLCyq5ET4W6WrIZucRVBs035YXIN+z3EzCxBj6O1lcjOTTHSFFR
jI3t3AGdkFo7tCBu5TFlNEFfmaqjse140vfGWJpRKyOT4ahzXLcVxzfg/SKRID3Q
2Rop4KqLNCddzz+UM+IiwyFkOfjrXWStW46cLUzM1k5GRrl0aBg5oqMCBY4/Eeh3
W0ZATsxxfg2Ly4FIO5d7/xiZqARFuurYe/2PSzVSPIKQrPVjDEekD+qQ0bRRQGvL
KBiMhYZHwnz/OEQl21PNp5rksuFjKG4/PimQ8V96jpbzzsOZuic3aScszgNUPbdI
0LYjCQ8xCOFPpFC4x1+rGubRjKEuGCvvYErVkX9rQlFRGPOp+k8bYTlIUKZeNsuL
KiZ4VH5+ZUAIf94DHayoo/SvBsQ5Qizb17KVRKil+vidUkMtndrNtjr3GWmH+nkn
PhRXBlekUy3dgRvTE8RFDOG9TYAN1Bs/uMgNc8Sg5Yz0SG96SLXVer2zsjmQ7tf9
6s+UVvrr+wlL7jSJCfJo6gaUQh1sD3umPXDS+Fq+J7tiRwOvP3cejo8dLyhesDun
FGIHlKmUCIwS/3Kzvd9OtAJMsmV9Q2B1dXudJloj6ADaAmVvhI/eMUncL9sXMJZH
3CCorh2OSZt0vtdA59osgDSMrsQZSMrtovrKgeFmP1Z0ENvo90Zenm7Bjn6Hw3Y/
GebIgSgoc149ElxjN4nagIqSJJHRrYq85sjTUSESvQUL1oi4R/VU+qMIRSHju/ZM
bkqONDohUc7/pg5rnLTZnnaQ09KvdF0yySx3hYph7L7MZWV/tF7O7yj1egRKh7lT
rgZOI7EiN4DPfTTpXoWVmIpiB/ouKp6uZ/Zrq00WthT8lUaBsFYaC3FDkkcxwdkk
lJ+cvdbUdsU=
-----END CERTIFICATE-----"#;

/// Run the one-time pairing flow with a Lutron RA3 processor.
pub async fn pair(host: &str, certs_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(certs_dir)?;

    // Generate RSA-2048 key pair
    info!("Generating RSA-2048 key pair...");
    let rsa_key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)
        .context("Failed to generate RSA key")?;
    let key_der = rsa_key
        .to_pkcs8_der()
        .context("Failed to encode key to PKCS8 DER")?;
    let key_pair = KeyPair::try_from(key_der.as_bytes())
        .context("Failed to create rcgen KeyPair from RSA key")?;

    // Generate CSR
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "ra-bridge");
    params.distinguished_name = dn;

    let csr = params
        .serialize_request(&key_pair)
        .context("Failed to generate CSR")?;
    let csr_pem = csr.pem().context("Failed to encode CSR as PEM")?;

    // Save the private key now
    let key_path = certs_dir.join("ra-bridge.key");
    std::fs::write(&key_path, key_pair.serialize_pem())?;
    info!("Private key saved to {}", key_path.display());

    // Phase 1: Connect to pairing port with LAP credentials
    info!(
        "Connecting to {}:{} for pairing...",
        host, PAIRING_PORT
    );
    info!("Trying RA3 (Lutron Root CA) first...");

    let tls_connector = build_pairing_tls_connector()?;
    let tcp = TcpStream::connect((host, PAIRING_PORT)).await?;
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| rustls::pki_types::ServerName::IpAddress(
            host.parse::<std::net::IpAddr>()
                .expect("Invalid host address")
                .into(),
        ));
    let tls = tls_connector.connect(server_name, tcp).await
        .context("TLS connection to pairing port failed")?;

    info!("Connected! Press the pairing button on the processor within {} seconds...", BUTTON_TIMEOUT_SECS);

    // Read lines until we get PhysicalAccess permission
    let timeout = tokio::time::Duration::from_secs(BUTTON_TIMEOUT_SECS);
    let (reader, mut writer) = tokio::io::split(tls);
    let mut reader = tokio::io::BufReader::new(reader);

    let got_access = tokio::time::timeout(timeout, async {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                bail!("Connection closed before receiving PhysicalAccess");
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(perms) = v.pointer("/Body/Status/Permissions") {
                    if let Some(arr) = perms.as_array() {
                        for p in arr {
                            if p.as_str() == Some("PhysicalAccess") {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    })
    .await;

    match got_access {
        Ok(Ok(())) => info!("Physical access granted!"),
        Ok(Err(e)) => bail!("Error waiting for physical access: {}", e),
        Err(_) => bail!("Timeout waiting for button press ({} seconds)", BUTTON_TIMEOUT_SECS),
    }

    // Send CSR
    let pair_request = serde_json::json!({
        "Header": {
            "RequestType": "Execute",
            "Url": "/pair",
            "ClientTag": "get-cert"
        },
        "Body": {
            "CommandType": "CSR",
            "Parameters": {
                "CSR": csr_pem,
                "DisplayName": "ra-bridge",
                "DeviceUID": "000000000000",
                "Role": "Admin"
            }
        }
    });
    let mut msg = serde_json::to_string(&pair_request)?;
    msg.push_str("\r\n");
    writer.write_all(msg.as_bytes()).await?;
    info!("CSR submitted, waiting for signed certificate...");

    // Read response with signed cert
    let mut line = String::new();
    let cert_response = tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        async {
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    bail!("Connection closed before receiving certificate");
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let v: serde_json::Value = serde_json::from_str(trimmed)?;
                if v.pointer("/Body/SigningResult").is_some() {
                    return Ok(v);
                }
            }
        },
    )
    .await
    .context("Timeout waiting for certificate response")?
    .context("Error reading certificate response")?;

    let signed_cert = cert_response
        .pointer("/Body/SigningResult/Certificate")
        .and_then(|v| v.as_str())
        .context("Missing Certificate in response")?;
    let root_cert = cert_response
        .pointer("/Body/SigningResult/RootCertificate")
        .and_then(|v| v.as_str());

    // Save signed certificate
    let cert_path = certs_dir.join("ra-bridge.crt");
    std::fs::write(&cert_path, signed_cert)?;
    info!("Signed certificate saved to {}", cert_path.display());

    // For RA3, use the Lutron Root CA; for Caseta, use the returned root cert
    let ca_path = certs_dir.join("ca.crt");
    let ca_pem = if let Some(rc) = root_cert {
        // Check if it's the Caseta CA or Lutron root; for RA3 we prefer the Lutron root
        rc.to_string()
    } else {
        LUTRON_ROOT_CA_PEM.to_string()
    };
    std::fs::write(&ca_path, &ca_pem)?;
    info!("CA certificate saved to {}", ca_path.display());

    // Phase 2: Verify by connecting to LEAP port
    info!("Verifying pairing by connecting to port {}...", LEAP_PORT);
    if let Err(e) = verify_pairing(host, certs_dir).await {
        warn!("Verification failed: {}. You may need to re-pair.", e);
    } else {
        info!("Pairing verified successfully!");
    }

    Ok(())
}

fn build_pairing_tls_connector() -> Result<TlsConnector> {
    let mut root_store = rustls::RootCertStore::empty();

    // Add both CAs — the LAP CA (Caseta) and the Lutron Root CA (RA3)
    for ca_pem in [LAP_CA_PEM, LUTRON_ROOT_CA_PEM] {
        let mut reader = BufReader::new(ca_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
        for cert in certs {
            root_store.add(cert)?;
        }
    }

    // Load LAP client cert + key
    let mut cert_reader = BufReader::new(LAP_CERT_PEM.as_bytes());
    let client_certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    let mut key_reader = BufReader::new(LAP_KEY_PEM.as_bytes());
    let client_key = rustls_pemfile::rsa_private_keys(&mut key_reader)
        .next()
        .ok_or_else(|| anyhow::anyhow!("No RSA key found in LAP key PEM"))?
        .context("Failed to parse LAP RSA key")?;

    let verifier = Arc::new(
        crate::leap_client::NoHostnameVerification::new(Arc::new(root_store))
            .context("Failed to build pairing cert verifier")?,
    );

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(
            client_certs,
            rustls::pki_types::PrivateKeyDer::Pkcs1(client_key),
        )
        .context("Failed to build TLS client config")?;

    Ok(TlsConnector::from(Arc::new(config)))
}

async fn verify_pairing(host: &str, certs_dir: &Path) -> Result<()> {
    let connector = crate::leap_client::build_leap_tls_connector(certs_dir)?;
    let tcp = TcpStream::connect((host, LEAP_PORT)).await?;
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| rustls::pki_types::ServerName::IpAddress(
            host.parse::<std::net::IpAddr>()
                .expect("Invalid host address")
                .into(),
        ));
    let mut tls = connector.connect(server_name, tcp).await?;

    let ping = serde_json::json!({
        "CommuniqueType": "ReadRequest",
        "Header": {"Url": "/server/1/status/ping"}
    });
    let mut msg = serde_json::to_string(&ping)?;
    msg.push_str("\r\n");

    use tokio::io::AsyncWriteExt;
    tls.write_all(msg.as_bytes()).await?;

    let mut reader = tokio::io::BufReader::new(tls);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let v: serde_json::Value = serde_json::from_str(line.trim())?;
    if v.pointer("/Body/PingResponse").is_some() {
        info!("Ping response received: {}", line.trim());
        Ok(())
    } else {
        bail!("Unexpected response: {}", line.trim())
    }
}

