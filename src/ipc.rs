use crate::tui::app::{AppEvent, DeviceCommand};
use log::{error, info};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, broadcast, mpsc};

pub fn socket_path() -> std::io::Result<PathBuf> {
    Ok(crate::utils::runtime_dir()?.join("airpods-tui.sock"))
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
                AE::BatteryInfo(new_infos) => {
                    // Preserve last known "good" case battery when the new event
                    // reports Case as Disconnected (case was closed).  Without this,
                    // new IPC clients that replay the snapshot would lose the case
                    // level because handle_aacp_event skips Disconnected case entries.
                    use crate::bluetooth::aacp::{BatteryComponent, BatteryStatus};
                    let new_has_case = new_infos.iter().any(|b| {
                        b.component == BatteryComponent::Case
                            && b.status != BatteryStatus::Disconnected
                    });
                    if !new_has_case {
                        // Find the previous case battery entry from the old snapshot
                        let prev_case = snapshot.iter().find_map(|e| {
                            if let AppEvent::AACPEvent(m, ae) = e
                                && m == mac
                                && let AE::BatteryInfo(prev) = &**ae
                            {
                                prev.iter()
                                    .find(|b| {
                                        b.component == BatteryComponent::Case
                                            && b.status != BatteryStatus::Disconnected
                                    })
                                    .cloned()
                            } else {
                                None
                            }
                        });
                        if let Some(case_info) = prev_case {
                            // Rebuild the event with the preserved case entry
                            let mut merged = new_infos.clone();
                            merged.retain(|b| b.component != BatteryComponent::Case);
                            merged.push(case_info);
                            let merged_event =
                                AppEvent::AACPEvent(mac.clone(), Box::new(AE::BatteryInfo(merged)));
                            snapshot.retain(|e| !matches!(e, AppEvent::AACPEvent(m, ae) if m == mac && matches!(**ae, AE::BatteryInfo(_))));
                            snapshot.push(merged_event);
                            return;
                        }
                    }
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
                // Transient events (StemPress, AudioSource, etc.) are not
                // meaningful to replay — skip storing them in the snapshot.
                _ => return,
            }
            snapshot.push(event.clone());
        }
        AppEvent::AudioUnavailable => {
            if !snapshot
                .iter()
                .any(|e| matches!(e, AppEvent::AudioUnavailable))
            {
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
        let path = socket_path()?;
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
                                    && write_tx_clone.send(json).is_err()
                                {
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
                        Ok(cmd) => {
                            let _ = cmd_tx.send(cmd);
                        }
                        Err(e) => {
                            error!("Invalid IPC command: {}", e);
                        }
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
    let path = socket_path()?;
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
                && write_msg(&mut writer, &json).await.is_err()
            {
                break;
            }
        }
    });

    Ok((cmd_tx, event_rx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::aacp::{
        AACPEvent as AE, AudioSource, AudioSourceType, BatteryComponent, BatteryInfo,
        BatteryStatus, ConnectedDevice, ControlCommandIdentifiers, ControlCommandStatus,
        EarDetectionStatus, StemPressBudType, StemPressType,
    };

    const MAC_A: &str = "AA:BB:CC:DD:EE:FF";
    const MAC_B: &str = "11:22:33:44:55:66";

    fn battery_event(mac: &str, infos: Vec<BatteryInfo>) -> AppEvent {
        AppEvent::AACPEvent(mac.into(), Box::new(AE::BatteryInfo(infos)))
    }

    fn control_event(mac: &str, id: ControlCommandIdentifiers, value: Vec<u8>) -> AppEvent {
        AppEvent::AACPEvent(
            mac.into(),
            Box::new(AE::ControlCommand(ControlCommandStatus {
                identifier: id,
                value,
            })),
        )
    }

    fn count_aacp(snap: &[AppEvent], mac: &str) -> usize {
        snap.iter()
            .filter(|e| matches!(e, AppEvent::AACPEvent(m, _) if m == mac))
            .count()
    }

    #[test]
    fn snapshot_replaces_device_on_reconnect() {
        let mut snap = Vec::new();
        let e1 = AppEvent::DeviceConnected {
            mac: MAC_A.into(),
            name: "Test".into(),
            product_id: 0x2014,
        };
        update_snapshot(&mut snap, &e1);
        assert_eq!(snap.len(), 1);

        let e2 = AppEvent::DeviceConnected {
            mac: MAC_A.into(),
            name: "Test Renamed".into(),
            product_id: 0x2014,
        };
        update_snapshot(&mut snap, &e2);
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            AppEvent::DeviceConnected { name, .. } => assert_eq!(name, "Test Renamed"),
            _ => panic!("expected DeviceConnected"),
        }
    }

    #[test]
    fn snapshot_disconnect_removes_device() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &AppEvent::DeviceConnected {
                mac: MAC_A.into(),
                name: "T".into(),
                product_id: 0,
            },
        );
        assert_eq!(snap.len(), 1);
        update_snapshot(&mut snap, &AppEvent::DeviceDisconnected(MAC_A.into()));
        assert!(snap.is_empty());
    }

    #[test]
    fn snapshot_disconnect_drops_only_target_device() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &AppEvent::DeviceConnected {
                mac: MAC_A.into(),
                name: "A".into(),
                product_id: 0,
            },
        );
        update_snapshot(
            &mut snap,
            &AppEvent::DeviceConnected {
                mac: MAC_B.into(),
                name: "B".into(),
                product_id: 0,
            },
        );
        update_snapshot(&mut snap, &battery_event(MAC_B, vec![]));
        update_snapshot(&mut snap, &AppEvent::DeviceDisconnected(MAC_A.into()));
        // B's DeviceConnected + BatteryInfo survive
        assert_eq!(snap.len(), 2);
        assert!(matches!(&snap[0], AppEvent::DeviceConnected { mac, .. } if mac == MAC_B));
    }

    #[test]
    fn snapshot_battery_replaces_previous_battery() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![BatteryInfo {
                    component: BatteryComponent::Left,
                    level: 50,
                    status: BatteryStatus::NotCharging,
                }],
            ),
        );
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![BatteryInfo {
                    component: BatteryComponent::Left,
                    level: 60,
                    status: BatteryStatus::NotCharging,
                }],
            ),
        );
        assert_eq!(count_aacp(&snap, MAC_A), 1);
        match &snap[0] {
            AppEvent::AACPEvent(_, ae) => match &**ae {
                AE::BatteryInfo(b) => assert_eq!(b[0].level, 60),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn snapshot_preserves_case_when_new_event_has_disconnected_case() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![
                    BatteryInfo {
                        component: BatteryComponent::Left,
                        level: 80,
                        status: BatteryStatus::NotCharging,
                    },
                    BatteryInfo {
                        component: BatteryComponent::Case,
                        level: 40,
                        status: BatteryStatus::NotCharging,
                    },
                ],
            ),
        );
        // case lid closed → reports Disconnected; should preserve previous case level
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![
                    BatteryInfo {
                        component: BatteryComponent::Left,
                        level: 81,
                        status: BatteryStatus::NotCharging,
                    },
                    BatteryInfo {
                        component: BatteryComponent::Case,
                        level: 0,
                        status: BatteryStatus::Disconnected,
                    },
                ],
            ),
        );
        let merged = match &snap[0] {
            AppEvent::AACPEvent(_, ae) => match &**ae {
                AE::BatteryInfo(b) => b.clone(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        let case = merged
            .iter()
            .find(|b| b.component == BatteryComponent::Case)
            .expect("case retained");
        assert_eq!(case.level, 40);
        assert_eq!(case.status, BatteryStatus::NotCharging);
    }

    #[test]
    fn snapshot_case_passthrough_when_present() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![BatteryInfo {
                    component: BatteryComponent::Case,
                    level: 30,
                    status: BatteryStatus::NotCharging,
                }],
            ),
        );
        update_snapshot(
            &mut snap,
            &battery_event(
                MAC_A,
                vec![BatteryInfo {
                    component: BatteryComponent::Case,
                    level: 90,
                    status: BatteryStatus::Charging,
                }],
            ),
        );
        match &snap[0] {
            AppEvent::AACPEvent(_, ae) => match &**ae {
                AE::BatteryInfo(b) => {
                    let case = b
                        .iter()
                        .find(|x| x.component == BatteryComponent::Case)
                        .unwrap();
                    assert_eq!(case.level, 90);
                    assert_eq!(case.status, BatteryStatus::Charging);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn snapshot_control_command_replaces_per_identifier() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &control_event(MAC_A, ControlCommandIdentifiers::ListeningMode, vec![0x02]),
        );
        update_snapshot(
            &mut snap,
            &control_event(MAC_A, ControlCommandIdentifiers::ListeningMode, vec![0x03]),
        );
        // different identifier should accumulate
        update_snapshot(
            &mut snap,
            &control_event(MAC_A, ControlCommandIdentifiers::OneBudAncMode, vec![0x01]),
        );
        assert_eq!(count_aacp(&snap, MAC_A), 2);
        let listening = snap.iter().find_map(|e| match e {
            AppEvent::AACPEvent(_, ae) => match &**ae {
                AE::ControlCommand(c)
                    if c.identifier == ControlCommandIdentifiers::ListeningMode =>
                {
                    Some(c.value.clone())
                }
                _ => None,
            },
            _ => None,
        });
        assert_eq!(listening, Some(vec![0x03]));
    }

    #[test]
    fn snapshot_keeps_devices_independent() {
        let mut snap = Vec::new();
        update_snapshot(
            &mut snap,
            &control_event(MAC_A, ControlCommandIdentifiers::ListeningMode, vec![0x02]),
        );
        update_snapshot(
            &mut snap,
            &control_event(MAC_B, ControlCommandIdentifiers::ListeningMode, vec![0x03]),
        );
        // Replacing A's listening mode must not touch B's
        update_snapshot(
            &mut snap,
            &control_event(MAC_A, ControlCommandIdentifiers::ListeningMode, vec![0x04]),
        );
        assert_eq!(count_aacp(&snap, MAC_A), 1);
        assert_eq!(count_aacp(&snap, MAC_B), 1);
    }

    #[test]
    fn snapshot_skips_transient_events() {
        let mut snap = Vec::new();
        // StemPress, AudioSource, ConversationalAwareness, OwnershipToFalse, ConnectionLost
        // are transient and should not appear in the replay snapshot.
        let stem = AppEvent::AACPEvent(
            MAC_A.into(),
            Box::new(AE::StemPress(
                StemPressType::Single,
                Some(StemPressBudType::Left),
            )),
        );
        let audio = AppEvent::AACPEvent(
            MAC_A.into(),
            Box::new(AE::AudioSource(AudioSource {
                mac: MAC_A.into(),
                r#type: AudioSourceType::Media,
            })),
        );
        let ca = AppEvent::AACPEvent(MAC_A.into(), Box::new(AE::ConversationalAwareness(1)));
        let lost = AppEvent::AACPEvent(MAC_A.into(), Box::new(AE::ConnectionLost));

        for e in [&stem, &audio, &ca, &lost] {
            update_snapshot(&mut snap, e);
        }
        assert!(
            snap.is_empty(),
            "transient events leaked into snapshot: {:?}",
            snap
        );
    }

    #[test]
    fn snapshot_replaces_ear_detection_per_device() {
        let mut snap = Vec::new();
        let mk = |l: EarDetectionStatus, r: EarDetectionStatus| {
            AppEvent::AACPEvent(
                MAC_A.into(),
                Box::new(AE::EarDetection {
                    old_left: None,
                    old_right: None,
                    new_left: Some(l),
                    new_right: Some(r),
                }),
            )
        };
        update_snapshot(
            &mut snap,
            &mk(EarDetectionStatus::OutOfEar, EarDetectionStatus::OutOfEar),
        );
        update_snapshot(
            &mut snap,
            &mk(EarDetectionStatus::InEar, EarDetectionStatus::InEar),
        );
        assert_eq!(count_aacp(&snap, MAC_A), 1);
        match &snap[0] {
            AppEvent::AACPEvent(_, ae) => match &**ae {
                AE::EarDetection {
                    new_left,
                    new_right,
                    ..
                } => {
                    assert_eq!(*new_left, Some(EarDetectionStatus::InEar));
                    assert_eq!(*new_right, Some(EarDetectionStatus::InEar));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn snapshot_replaces_connected_devices_per_device() {
        let mut snap = Vec::new();
        let mk = |peers: Vec<ConnectedDevice>| {
            AppEvent::AACPEvent(MAC_A.into(), Box::new(AE::ConnectedDevices(vec![], peers)))
        };
        update_snapshot(
            &mut snap,
            &mk(vec![ConnectedDevice {
                mac: "11:22:33:44:55:66".into(),
                info1: 0,
                info2: 0,
            }]),
        );
        update_snapshot(&mut snap, &mk(vec![]));
        assert_eq!(count_aacp(&snap, MAC_A), 1);
    }

    #[test]
    fn snapshot_audio_unavailable_dedupes() {
        let mut snap = Vec::new();
        update_snapshot(&mut snap, &AppEvent::AudioUnavailable);
        update_snapshot(&mut snap, &AppEvent::AudioUnavailable);
        update_snapshot(&mut snap, &AppEvent::AudioUnavailable);
        assert_eq!(
            snap.iter()
                .filter(|e| matches!(e, AppEvent::AudioUnavailable))
                .count(),
            1
        );
    }
}
