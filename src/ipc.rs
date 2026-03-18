use crate::tui::app::{AppEvent, DeviceCommand};
use log::{error, info};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, broadcast, mpsc};

pub fn socket_path() -> PathBuf {
    crate::utils::runtime_dir().join("airpods-tui.sock")
}

async fn write_msg(stream: &mut (impl AsyncWriteExt + Unpin), data: &[u8]) -> std::io::Result<()> {
    let len = (data.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(data).await?;
    stream.flush().await
}

async fn read_msg(stream: &mut (impl AsyncReadExt + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// State snapshot maintained by the daemon for replaying to new clients.
pub type StateSnapshot = Arc<RwLock<Vec<AppEvent>>>;

/// Build a fresh snapshot from a stream of AppEvents.
/// Keeps the latest DeviceConnected + all AACPEvents per device.
pub fn update_snapshot(snapshot: &mut Vec<AppEvent>, event: &AppEvent) {
    match event {
        AppEvent::DeviceConnected { mac, .. } => {
            // Remove old events for this device and re-add
            snapshot.retain(|e| match e {
                AppEvent::DeviceConnected { mac: m, .. } => m != mac,
                AppEvent::AACPEvent(m, _) => m != mac,
                _ => true,
            });
            snapshot.push(event.clone());
        }
        AppEvent::DeviceDisconnected(mac) => {
            snapshot.retain(|e| match e {
                AppEvent::DeviceConnected { mac: m, .. } => m != mac,
                AppEvent::AACPEvent(m, _) => m != mac,
                AppEvent::DeviceDisconnected(m) => m != mac,
                _ => true,
            });
        }
        AppEvent::AACPEvent(mac, aacp_event) => {
            // For control commands / battery, replace previous of same variant per device
            use crate::bluetooth::aacp::AACPEvent as AE;
            match &**aacp_event {
                AE::BatteryInfo(_) => {
                    snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::BatteryInfo(_))));
                }
                AE::ControlCommand(cmd) => {
                    let id = cmd.identifier;
                    snapshot.retain(|e| {
                        !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(&**ae, AE::ControlCommand(c) if c.identifier == id))
                    });
                }
                AE::DeviceInfo(_) => {
                    snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::DeviceInfo(_))));
                }
                AE::EarDetection { .. } => {
                    snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::EarDetection { .. })));
                }
                AE::ConnectedDevices(_, _) => {
                    snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::ConnectedDevices(_, _))));
                }
                AE::EqData(_) => {
                    snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::EqData(_))));
                }
                _ => {}
            }
            snapshot.push(event.clone());
        }
        AppEvent::AudioUnavailable => {
            if !snapshot.iter().any(|e| matches!(e, AppEvent::AudioUnavailable)) {
                snapshot.push(event.clone());
            }
        }
    }
}

pub struct IpcServer {
    snapshot: StateSnapshot,
    broadcast_tx: broadcast::Sender<AppEvent>,
    cmd_tx: mpsc::UnboundedSender<(String, DeviceCommand)>,
}

impl IpcServer {
    pub fn new(
        snapshot: StateSnapshot,
        cmd_tx: mpsc::UnboundedSender<(String, DeviceCommand)>,
    ) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            snapshot,
            broadcast_tx,
            cmd_tx,
        }
    }

    /// Broadcast an event to all connected clients.
    pub fn broadcast(&self, event: &AppEvent) {
        let _ = self.broadcast_tx.send(event.clone());
    }

    /// Run the IPC server, accepting connections on the Unix socket.
    pub async fn run(&self) -> std::io::Result<()> {
        let path = socket_path();
        // Remove stale socket — ignore NotFound, log other errors
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("Failed to remove stale socket {}: {}", path.display(), e);
        }

        let listener = UnixListener::bind(&path)?;

        // Restrict socket to owner-only access
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            {
                log::warn!("Failed to set socket permissions: {}", e);
            }
        }

        info!("IPC server listening on {}", path.display());

        loop {
            let (stream, _) = listener.accept().await?;
            info!("IPC client connected");
            let snapshot = self.snapshot.clone();
            let mut event_rx = self.broadcast_tx.subscribe();
            let cmd_tx = self.cmd_tx.clone();

            tokio::spawn(async move {
                let (reader, writer) = stream.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                let mut writer = tokio::io::BufWriter::new(writer);

                // Replay snapshot
                {
                    let snap = snapshot.read().await;
                    for event in snap.iter() {
                        let json = match serde_json::to_vec(event) {
                            Ok(j) => j,
                            Err(e) => {
                                error!("Failed to serialize snapshot event: {}", e);
                                continue;
                            }
                        };
                        if write_msg(&mut writer, &json).await.is_err() {
                            return;
                        }
                    }
                }

                // Spawn writer task: forward broadcast events to client
                let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                let writer_handle = tokio::spawn(async move {
                    while let Some(data) = write_rx.recv().await {
                        if write_msg(&mut writer, &data).await.is_err() {
                            break;
                        }
                    }
                });

                // Forward broadcast events
                let write_tx_clone = write_tx.clone();
                let event_forward = tokio::spawn(async move {
                    loop {
                        match event_rx.recv().await {
                            Ok(event) => {
                                if let Ok(json) = serde_json::to_vec(&event)
                                    && write_tx_clone.send(json).is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                info!("IPC client lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });

                // Read commands from client
                while let Ok(data) = read_msg(&mut reader).await {
                    match serde_json::from_slice::<(String, DeviceCommand)>(&data) {
                        Ok(cmd) => { let _ = cmd_tx.send(cmd); }
                        Err(e) => { error!("Invalid IPC command: {}", e); }
                    }
                }

                info!("IPC client disconnected");
                event_forward.abort();
                writer_handle.abort();
            });
        }
    }
}

/// Connect to a running daemon via Unix socket.
/// Returns (cmd_tx, event_rx) that the TUI can use identically to in-process channels.
pub async fn ipc_connect() -> std::io::Result<(
    mpsc::UnboundedSender<(String, DeviceCommand)>,
    mpsc::UnboundedReceiver<AppEvent>,
)> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).await?;
    info!("Connected to IPC daemon at {}", path.display());

    let (reader, writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = tokio::io::BufWriter::new(writer);

    let (event_tx, event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<(String, DeviceCommand)>();

    // Read events from daemon → event_tx
    tokio::spawn(async move {
        loop {
            match read_msg(&mut reader).await {
                Ok(data) => match serde_json::from_slice::<AppEvent>(&data) {
                    Ok(event) => {
                        if event_tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Invalid IPC event: {}", e);
                    }
                },
                Err(_) => {
                    info!("IPC connection closed");
                    break;
                }
            }
        }
    });

    // Write commands from cmd_tx → daemon
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            if let Ok(json) = serde_json::to_vec(&cmd)
                && write_msg(&mut writer, &json).await.is_err() {
                break;
            }
        }
    });

    Ok((cmd_tx, event_rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_replaces_device_on_reconnect() {
        let mut snap = Vec::new();
        let e1 = AppEvent::DeviceConnected {
            mac: "AA:BB:CC:DD:EE:FF".into(),
            name: "Test".into(),
            product_id: 0x2014,
        };
        update_snapshot(&mut snap, &e1);
        assert_eq!(snap.len(), 1);

        let e2 = AppEvent::DeviceConnected {
            mac: "AA:BB:CC:DD:EE:FF".into(),
            name: "Test Renamed".into(),
            product_id: 0x2014,
        };
        update_snapshot(&mut snap, &e2);
        assert_eq!(snap.len(), 1); // replaced, not duplicated
    }

    #[test]
    fn snapshot_disconnect_removes_device() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &AppEvent::DeviceConnected {
                mac: "AA:BB:CC:DD:EE:FF".into(),
                name: "T".into(),
                product_id: 0,
            },
        );
        assert_eq!(snap.len(), 1);
        update_snapshot(
            &mut snap,
            &AppEvent::DeviceDisconnected("AA:BB:CC:DD:EE:FF".into()),
        );
        assert!(snap.is_empty());
    }
}
