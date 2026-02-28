use crate::bluetooth::aacp::ControlCommandIdentifiers;
use crate::bluetooth::aacp::{AACPEvent, AACPManager, AirPodsLEKeys, ProximityKeyType, StemPressType};
use crate::config::Config;
use crate::media_controller::MediaController;
use crate::tui::app::AppEvent;
use bluer::Address;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Duration;

pub struct AirPodsDevice {
    pub aacp_manager: AACPManager,
}

impl AirPodsDevice {
    pub async fn new(
        mac_address: Address,
        app_tx: UnboundedSender<AppEvent>,
        product_id: u16,
        config: Config,
    ) -> Result<Self, bluer::Error> {
        info!("Creating new AirPodsDevice for {}", mac_address);
        let mut aacp_manager = AACPManager::new();
        aacp_manager.connect(mac_address).await;

        // ── Set up event channel and ALL subscriptions BEFORE sending any packets ──
        // Otherwise the AirPods respond to handshake/notifications before we're listening,
        // and battery info, device info, and control command states are silently dropped.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<(ControlCommandIdentifiers, Vec<u8>)>();

        aacp_manager.set_event_channel(tx).await;

        // Control command subscriptions — all forwarded to TUI via AppEvent
        for cmd_id in [
            ControlCommandIdentifiers::ListeningMode,
            ControlCommandIdentifiers::AllowOffOption,
            ControlCommandIdentifiers::ConversationDetectConfig,
            ControlCommandIdentifiers::OneBudAncMode,
            ControlCommandIdentifiers::VolumeSwipeMode,
            ControlCommandIdentifiers::AdaptiveVolumeConfig,
            ControlCommandIdentifiers::AllowAutoConnect,
            ControlCommandIdentifiers::DoubleClickInterval,
            ControlCommandIdentifiers::ClickHoldInterval,
            ControlCommandIdentifiers::ChimeVolume,
            ControlCommandIdentifiers::VolumeSwipeInterval,
            ControlCommandIdentifiers::AutoAncStrength,
        ] {
            let (tx_sub, mut rx_sub) = tokio::sync::mpsc::unbounded_channel();
            aacp_manager
                .subscribe_to_control_command(cmd_id, tx_sub)
                .await;
            let app_tx_sub = app_tx.clone();
            let mac_str = mac_address.to_string();
            tokio::spawn(async move {
                while let Some(value) = rx_sub.recv().await {
                    let _ = app_tx_sub.send(AppEvent::AACPEvent(
                        mac_str.clone(),
                        AACPEvent::ControlCommand(crate::bluetooth::aacp::ControlCommandStatus {
                            identifier: cmd_id,
                            value,
                        }),
                    ));
                }
            });
        }

        // OwnsConnection — handle audio ownership loss
        let (owns_connection_tx, mut owns_connection_rx) = tokio::sync::mpsc::unbounded_channel();
        aacp_manager
            .subscribe_to_control_command(
                ControlCommandIdentifiers::OwnsConnection,
                owns_connection_tx,
            )
            .await;

        // Command dispatcher
        let aacp_manager_clone = aacp_manager.clone();
        tokio::spawn(async move {
            while let Some((id, value)) = command_rx.recv().await {
                if let Err(e) = aacp_manager_clone.send_control_command(id, &value).await {
                    log::error!("Failed to send control command: {}", e);
                }
            }
        });

        // ── Now send protocol packets (responses will be caught by channels above) ──
        // Instead of fixed sleeps between packets, we wait for the device to respond
        // (or time out after 500ms). AACP has no formal ACK, but the device typically
        // sends a response packet after processing each command.
        let notify = aacp_manager.state.lock().await.packet_received.clone();

        info!("Sending handshake");
        if let Err(e) = aacp_manager.send_handshake().await {
            error!("Failed to send handshake to AirPods device: {}", e);
        }

        let _ = tokio::time::timeout(Duration::from_millis(500), notify.notified()).await;

        info!("Setting feature flags");
        if let Err(e) = aacp_manager.send_set_feature_flags_packet().await {
            error!("Failed to set feature flags: {}", e);
        }

        let _ = tokio::time::timeout(Duration::from_millis(500), notify.notified()).await;

        info!("Requesting notifications");
        if let Err(e) = aacp_manager.send_notification_request().await {
            error!("Failed to request notifications: {}", e);
        }

        info!("sending some packet");
        if let Err(e) = aacp_manager.send_some_packet().await {
            error!("Failed to send some packet: {}", e);
        }

        if crate::devices::apple_models::needs_init_ext(product_id) {
            info!("Sending AapInitExt for model 0x{:04x} (unlocks Adaptive ANC)", product_id);
            let _ = tokio::time::timeout(Duration::from_millis(500), notify.notified()).await;
            if let Err(e) = aacp_manager.send_init_ext().await {
                error!("Failed to send AapInitExt: {}", e);
            }
        }

        info!("Requesting Proximity Keys: IRK and ENC_KEY");
        if let Err(e) = aacp_manager
            .send_proximity_keys_request(vec![ProximityKeyType::Irk, ProximityKeyType::EncKey])
            .await
        {
            error!("Failed to request proximity keys: {}", e);
        }

        // ── Media controller setup ──
        let session = bluer::Session::new().await?;
        let adapter = session.default_adapter().await?;
        let local_mac = adapter.address().await?.to_string();

        let media_controller = Arc::new(Mutex::new(MediaController::new(
            mac_address.to_string(),
            local_mac.clone(),
            config,
            Some(app_tx.clone()),
        )));
        let mc_clone = media_controller.clone();

        let mc_listener = media_controller.lock().await;
        let aacp_manager_clone_listener = aacp_manager.clone();
        mc_listener
            .start_playback_listener(aacp_manager_clone_listener, command_tx.clone())
            .await;
        drop(mc_listener);

        // OwnsConnection handler
        let mc_clone_owns = media_controller.clone();
        tokio::spawn(async move {
            while let Some(value) = owns_connection_rx.recv().await {
                let owns = value.first().copied().unwrap_or(0) != 0;
                if !owns {
                    info!("Lost ownership, pausing media and disconnecting audio");
                    let controller = mc_clone_owns.lock().await;
                    controller.pause_all_media().await;
                    controller.deactivate_a2dp_profile().await;
                }
            }
        });

        // Main AACP event loop
        let aacp_manager_clone_events = aacp_manager.clone();
        let local_mac_events = local_mac.clone();
        let app_tx_events = app_tx.clone();
        let command_tx_clone = command_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let event_clone = event.clone();
                match event {
                    AACPEvent::EarDetection(old_status, new_status) => {
                        debug!(
                            "Received EarDetection event: old_status={:?}, new_status={:?}",
                            old_status, new_status
                        );
                        let controller = mc_clone.lock().await;
                        controller
                            .handle_ear_detection(old_status, new_status)
                            .await;
                    }
                    AACPEvent::ConversationalAwareness(status) => {
                        debug!("Received ConversationalAwareness event: {}", status);
                        let controller = mc_clone.lock().await;
                        controller.handle_conversational_awareness(status).await;
                    }
                    AACPEvent::ConnectedDevices(old_devices, new_devices) => {
                        let local_mac = local_mac_events.clone();
                        let new_devices_filtered = new_devices.iter().filter(|new_device| {
                            let not_in_old = old_devices
                                .iter()
                                .all(|old_device| old_device.mac != new_device.mac);
                            let not_local = new_device.mac != local_mac;
                            not_in_old && not_local
                        });

                        for device in new_devices_filtered {
                            info!(
                                "New connected device: {}, info1: {}, info2: {}",
                                device.mac, device.info1, device.info2
                            );
                            let aacp_manager_clone = aacp_manager_clone_events.clone();
                            let local_mac_clone = local_mac.clone();
                            let device_mac_clone = device.mac.clone();
                            tokio::spawn(async move {
                                if let Err(e) = aacp_manager_clone
                                    .send_media_information_new_device(
                                        &local_mac_clone,
                                        &device_mac_clone,
                                    )
                                    .await
                                {
                                    error!("Failed to send media info new device: {}", e);
                                }
                                if let Err(e) = aacp_manager_clone
                                    .send_add_tipi_device(&local_mac_clone, &device_mac_clone)
                                    .await
                                {
                                    error!("Failed to send add tipi device: {}", e);
                                }
                            });
                        }
                    }
                    AACPEvent::OwnershipToFalseRequest => {
                        info!(
                            "Received ownership to false request. Setting ownership to false and pausing media."
                        );
                        let _ = command_tx_clone
                            .send((ControlCommandIdentifiers::OwnsConnection, vec![0x00]));
                        let controller = mc_clone.lock().await;
                        controller.pause_all_media().await;
                        controller.deactivate_a2dp_profile().await;
                    }
                    AACPEvent::StemPress(press_type, _bud) => {
                        let controller = mc_clone.lock().await;
                        match press_type {
                            StemPressType::SinglePress => {
                                info!("Stem single press — toggling play/pause");
                                controller.toggle_play_pause().await;
                            }
                            StemPressType::DoublePress => {
                                info!("Stem double press — next track");
                                controller.next_track().await;
                            }
                            StemPressType::TriplePress => {
                                info!("Stem triple press — previous track");
                                controller.previous_track().await;
                            }
                            StemPressType::LongPress => {
                                debug!("Stem long press — ignored");
                            }
                        }
                    }
                    _ => {
                        debug!("Forwarding AACP event to TUI: {:?}", event_clone);
                        let _ = app_tx_events.send(AppEvent::AACPEvent(
                            mac_address.to_string(),
                            event_clone,
                        ));
                    }
                }
            }
        });

        // media_controller and mac_address are used by spawned tasks above
        // but not needed in the struct after initialization
        let _ = media_controller;
        Ok(AirPodsDevice {
            aacp_manager,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirPodsInformation {
    pub name: String,
    pub model_number: String,
    pub manufacturer: String,
    pub serial_number: String,
    pub version1: String,
    pub version2: String,
    pub hardware_revision: String,
    pub updater_identifier: String,
    pub left_serial_number: String,
    pub right_serial_number: String,
    pub version3: String,
    pub le_keys: AirPodsLEKeys,
}
