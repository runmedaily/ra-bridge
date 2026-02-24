use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

use crate::ra2_protocol::{self, Ra2Command, Ra2Event};

/// Start the telnet server. Incoming commands are sent on `cmd_tx`.
/// Events from LEAP are received on `event_rx` and forwarded to all clients.
pub async fn run(
    port: u16,
    cmd_tx: mpsc::Sender<Ra2Command>,
    event_tx: broadcast::Sender<Ra2Event>,
) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("RA2 telnet server listening on port {}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("Telnet client connected: {}", addr);

        let cmd_tx = cmd_tx.clone();
        let event_rx = event_tx.subscribe();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, cmd_tx, event_rx).await {
                warn!("Client {} disconnected: {}", addr, e);
            }
        });
    }
}

async fn handle_client(
    stream: TcpStream,
    cmd_tx: mpsc::Sender<Ra2Command>,
    mut event_rx: broadcast::Receiver<Ra2Event>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Login sequence
    if !login_flow(&mut reader, &mut writer).await? {
        return Ok(());
    }

    writer.write_all(b"GNET> ").await?;

    // Spawn event writer task
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let write_handle = {
        let mut writer = writer;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = event_rx.recv() => {
                        match event {
                            Ok(ev) => {
                                let line = format!("{}\r\n", ra2_protocol::format_event(&ev));
                                if writer.write_all(line.as_bytes()).await.is_err() {
                                    break;
                                }
                                if writer.write_all(b"GNET> ").await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Client lagged, dropped {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = shutdown_rx.recv() => break,
                }
            }
            writer
        })
    };

    // Read commands from client
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // Client disconnected
        }

        if let Some(cmd) = ra2_protocol::parse_command(&line) {
            cmd_tx.send(cmd).await?;
        }
    }

    drop(shutdown_tx);
    let _ = write_handle.await;
    Ok(())
}

async fn login_flow(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<bool> {
    // Send login prompt
    writer.write_all(b"login: ").await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let username = line.trim().to_string();
    line.clear();

    writer.write_all(b"password: ").await?;
    reader.read_line(&mut line).await?;
    let password = line.trim().to_string();

    if username == "lutron" && password == "integration" {
        info!("Client authenticated successfully");
        Ok(true)
    } else {
        warn!(
            "Authentication failed: user={:?} pass={:?}",
            username, password
        );
        writer.write_all(b"login incorrect\r\n").await?;
        Ok(false)
    }
}
