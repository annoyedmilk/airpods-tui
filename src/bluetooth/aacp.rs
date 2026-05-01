use crate::devices::airpods::AirPodsInformation;
use crate::devices::enums::{DeviceData, DeviceInformation, DeviceType};
use crate::utils::get_devices_path;
use bluer::{
    Address, AddressType, Error, Result,
    l2cap::{Security, SecurityLevel, SeqPacket, Socket, SocketAddr},
};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep};

const PSM: u16 = 0x1001;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const HEADER_BYTES: [u8; 4] = [0x04, 0x00, 0x04, 0x00];

pub mod opcodes {
    pub const SET_FEATURE_FLAGS: u8 = 0x4D;
    pub const REQUEST_NOTIFICATIONS: u8 = 0x0F;
    pub const BATTERY_INFO: u8 = 0x04;
    pub const CONTROL_COMMAND: u8 = 0x09;
    pub const EAR_DETECTION: u8 = 0x06;
    pub const CONVERSATION_AWARENESS: u8 = 0x4B;
    pub const INFORMATION: u8 = 0x1D;
    pub const RENAME: u8 = 0x1A;
    pub const PROXIMITY_KEYS_REQ: u8 = 0x30;
    pub const PROXIMITY_KEYS_RSP: u8 = 0x31;
    pub const STEM_PRESS: u8 = 0x19;
    pub const EQ_DATA: u8 = 0x53;
    pub const CONNECTED_DEVICES: u8 = 0x2E;
    pub const AUDIO_SOURCE: u8 = 0x0E;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ControlCommandStatus {
    pub identifier: ControlCommandIdentifiers,
    pub value: Vec<u8>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize_repr, Deserialize_repr)]
pub enum ControlCommandIdentifiers {
    MicMode = 0x01,
    ButtonSendMode = 0x05,
    VoiceTrigger = 0x12,
    SingleClickMode = 0x14,
    DoubleClickMode = 0x15,
    ClickHoldMode = 0x16,
    DoubleClickInterval = 0x17,
    ClickHoldInterval = 0x18,
    ListeningModeConfigs = 0x1A,
    OneBudAncMode = 0x1B,
    CrownRotationDirection = 0x1C,
    ListeningMode = 0x0D,
    AutoAnswerMode = 0x1E,
    ChimeVolume = 0x1F,
    VolumeSwipeInterval = 0x23,
    CallManagementConfig = 0x24,
    VolumeSwipeMode = 0x25,
    AdaptiveVolumeConfig = 0x26,
    SoftwareMuteConfig = 0x27,
    ConversationDetectConfig = 0x28,
    Ssl = 0x29,
    HearingAid = 0x2C,
    AutoAncStrength = 0x2E,
    HpsGainSwipe = 0x2F,
    HrmState = 0x30,
    InCaseToneConfig = 0x31,
    SiriMultitoneConfig = 0x32,
    HearingAssistConfig = 0x33,
    AllowOffOption = 0x34,
    StemConfig = 0x39,
    SleepDetectionConfig = 0x35,
    AllowAutoConnect = 0x36,
    EarDetectionConfig = 0x0A,
    AutomaticConnectionConfig = 0x20,
    OwnsConnection = 0x06,
}

impl TryFrom<u8> for ControlCommandIdentifiers {
    type Error = ();
    fn try_from(value: u8) -> std::result::Result<Self, ()> {
        match value {
            0x01 => Ok(Self::MicMode),
            0x05 => Ok(Self::ButtonSendMode),
            0x12 => Ok(Self::VoiceTrigger),
            0x14 => Ok(Self::SingleClickMode),
            0x15 => Ok(Self::DoubleClickMode),
            0x16 => Ok(Self::ClickHoldMode),
            0x17 => Ok(Self::DoubleClickInterval),
            0x18 => Ok(Self::ClickHoldInterval),
            0x1A => Ok(Self::ListeningModeConfigs),
            0x1B => Ok(Self::OneBudAncMode),
            0x1C => Ok(Self::CrownRotationDirection),
            0x0D => Ok(Self::ListeningMode),
            0x1E => Ok(Self::AutoAnswerMode),
            0x1F => Ok(Self::ChimeVolume),
            0x23 => Ok(Self::VolumeSwipeInterval),
            0x24 => Ok(Self::CallManagementConfig),
            0x25 => Ok(Self::VolumeSwipeMode),
            0x26 => Ok(Self::AdaptiveVolumeConfig),
            0x27 => Ok(Self::SoftwareMuteConfig),
            0x28 => Ok(Self::ConversationDetectConfig),
            0x29 => Ok(Self::Ssl),
            0x2C => Ok(Self::HearingAid),
            0x2E => Ok(Self::AutoAncStrength),
            0x2F => Ok(Self::HpsGainSwipe),
            0x30 => Ok(Self::HrmState),
            0x31 => Ok(Self::InCaseToneConfig),
            0x32 => Ok(Self::SiriMultitoneConfig),
            0x33 => Ok(Self::HearingAssistConfig),
            0x34 => Ok(Self::AllowOffOption),
            0x39 => Ok(Self::StemConfig),
            0x35 => Ok(Self::SleepDetectionConfig),
            0x36 => Ok(Self::AllowAutoConnect),
            0x0A => Ok(Self::EarDetectionConfig),
            0x20 => Ok(Self::AutomaticConnectionConfig),
            0x06 => Ok(Self::OwnsConnection),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for ControlCommandIdentifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ControlCommandIdentifiers::MicMode => "Mic Mode",
            ControlCommandIdentifiers::ButtonSendMode => "Button Send Mode",
            ControlCommandIdentifiers::VoiceTrigger => "Voice Trigger",
            ControlCommandIdentifiers::SingleClickMode => "Single Click Mode",
            ControlCommandIdentifiers::DoubleClickMode => "Double Click Mode",
            ControlCommandIdentifiers::ClickHoldMode => "Click Hold Mode",
            ControlCommandIdentifiers::DoubleClickInterval => "Double Click Interval",
            ControlCommandIdentifiers::ClickHoldInterval => "Click Hold Interval",
            ControlCommandIdentifiers::ListeningModeConfigs => "Listening Mode Configs",
            ControlCommandIdentifiers::OneBudAncMode => "One Bud ANC Mode",
            ControlCommandIdentifiers::CrownRotationDirection => "Crown Rotation Direction",
            ControlCommandIdentifiers::ListeningMode => "Listening Mode",
            ControlCommandIdentifiers::AutoAnswerMode => "Auto Answer Mode",
            ControlCommandIdentifiers::ChimeVolume => "Chime Volume",
            ControlCommandIdentifiers::VolumeSwipeInterval => "Volume Swipe Interval",
            ControlCommandIdentifiers::CallManagementConfig => "Call Management Config",
            ControlCommandIdentifiers::VolumeSwipeMode => "Volume Swipe Mode",
            ControlCommandIdentifiers::AdaptiveVolumeConfig => "Adaptive Volume Config",
            ControlCommandIdentifiers::SoftwareMuteConfig => "Software Mute Config",
            ControlCommandIdentifiers::ConversationDetectConfig => "Conversation Detect Config",
            ControlCommandIdentifiers::Ssl => "SSL",
            ControlCommandIdentifiers::HearingAid => "Hearing Aid",
            ControlCommandIdentifiers::AutoAncStrength => "Auto ANC Strength",
            ControlCommandIdentifiers::HpsGainSwipe => "HPS Gain Swipe",
            ControlCommandIdentifiers::HrmState => "HRM State",
            ControlCommandIdentifiers::InCaseToneConfig => "In Case Tone Config",
            ControlCommandIdentifiers::SiriMultitoneConfig => "Siri Multitone Config",
            ControlCommandIdentifiers::HearingAssistConfig => "Hearing Assist Config",
            ControlCommandIdentifiers::AllowOffOption => "Allow Off Option",
            ControlCommandIdentifiers::StemConfig => "Stem Config",
            ControlCommandIdentifiers::SleepDetectionConfig => "Sleep Detection Config",
            ControlCommandIdentifiers::AllowAutoConnect => "Allow Auto Connect",
            ControlCommandIdentifiers::EarDetectionConfig => "Ear Detection Config",
            ControlCommandIdentifiers::AutomaticConnectionConfig => "Automatic Connection Config",
            ControlCommandIdentifiers::OwnsConnection => "Owns Connection",
        };
        write!(f, "{}", name)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ProximityKeyType {
    Irk = 0x01,
    EncKey = 0x04,
}

impl TryFrom<u8> for ProximityKeyType {
    type Error = ();
    fn try_from(value: u8) -> std::result::Result<Self, ()> {
        match value {
            0x01 => Ok(Self::Irk),
            0x04 => Ok(Self::EncKey),
            _ => Err(()),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum StemPressType {
    Single = 0x05,
    Double = 0x06,
    Triple = 0x07,
    Long = 0x08,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum StemPressBudType {
    Left = 0x01,
    Right = 0x02,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum AudioSourceType {
    None = 0x00,
    Call = 0x01,
    Media = 0x02,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum BatteryComponent {
    Headphone = 1,
    Left = 4,
    Right = 2,
    Case = 8,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum BatteryStatus {
    Charging = 1,
    NotCharging = 2,
    Disconnected = 4,
    InUse = 5, // 0x05 — active/playing state on AirPods Pro 3rd gen
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
pub enum EarDetectionStatus {
    InEar = 0x00,
    OutOfEar = 0x01,
    InCase = 0x02,
    Disconnected = 0x03,
}

impl TryFrom<u8> for AudioSourceType {
    type Error = ();
    fn try_from(value: u8) -> std::result::Result<Self, ()> {
        match value {
            0x00 => Ok(Self::None),
            0x01 => Ok(Self::Call),
            0x02 => Ok(Self::Media),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSource {
    pub mac: String,
    pub r#type: AudioSourceType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryInfo {
    pub component: BatteryComponent,
    pub level: u8,
    pub status: BatteryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedDevice {
    pub mac: String,
    pub info1: u8,
    pub info2: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AACPEvent {
    BatteryInfo(Vec<BatteryInfo>),
    ControlCommand(ControlCommandStatus),
    EarDetection {
        old_left: Option<EarDetectionStatus>,
        old_right: Option<EarDetectionStatus>,
        new_left: Option<EarDetectionStatus>,
        new_right: Option<EarDetectionStatus>,
    },
    ConversationalAwareness(u8),
    AudioSource(AudioSource),
    ConnectedDevices(Vec<ConnectedDevice>, Vec<ConnectedDevice>),
    OwnershipToFalseRequest,
    DeviceInfo(Box<crate::devices::airpods::AirPodsInformation>),
    StemPress(StemPressType, Option<StemPressBudType>),
    EqData([u8; 8]),
    /// L2CAP connection dropped (read error or remote close).
    ConnectionLost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirPodsLEKeys {
    pub irk: String,
    pub enc_key: String,
}

pub struct AACPManagerState {
    pub sender: Option<mpsc::Sender<Vec<u8>>>,
    pub control_command_status_list: Vec<ControlCommandStatus>,
    pub control_command_subscribers:
        HashMap<ControlCommandIdentifiers, Vec<mpsc::UnboundedSender<Vec<u8>>>>,
    pub owns: bool,
    pub old_connected_devices: Vec<ConnectedDevice>,
    pub connected_devices: Vec<ConnectedDevice>,
    pub audio_source: Option<AudioSource>,
    pub battery_info: Vec<BatteryInfo>,
    pub conversational_awareness_status: u8,
    pub ear_detection_left: Option<EarDetectionStatus>,
    pub ear_detection_right: Option<EarDetectionStatus>,
    pub primary_pod: Option<BatteryComponent>,
    event_tx: Option<mpsc::UnboundedSender<AACPEvent>>,
    pub devices: HashMap<String, DeviceData>,
    pub airpods_mac: Option<Address>,
    /// Notified after every successfully parsed incoming packet, allowing callers
    /// to wait for a device response instead of using fixed sleeps.
    pub packet_received: Arc<tokio::sync::Notify>,
    /// Broadcasts the opcode of every incoming packet for strict init sequencing.
    pub opcode_tx: tokio::sync::broadcast::Sender<u8>,
}

impl AACPManagerState {
    fn new() -> Self {
        let devices: HashMap<String, DeviceData> = std::fs::read_to_string(get_devices_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        AACPManagerState {
            sender: None,
            control_command_status_list: Vec::new(),
            control_command_subscribers: HashMap::new(),
            owns: false,
            old_connected_devices: Vec::new(),
            connected_devices: Vec::new(),
            audio_source: None,
            battery_info: Vec::new(),
            conversational_awareness_status: 0,
            ear_detection_left: None,
            ear_detection_right: None,
            primary_pod: None,
            event_tx: None,
            devices,
            airpods_mac: None,
            packet_received: Arc::new(tokio::sync::Notify::new()),
            opcode_tx: tokio::sync::broadcast::channel(16).0,
        }
    }
}

#[derive(Clone)]
pub struct AACPManager {
    pub state: Arc<Mutex<AACPManagerState>>,
    tasks: Arc<Mutex<JoinSet<()>>>,
}

impl AACPManager {
    pub fn new() -> Self {
        AACPManager {
            state: Arc::new(Mutex::new(AACPManagerState::new())),
            tasks: Arc::new(Mutex::new(JoinSet::new())),
        }
    }

    pub async fn connect(&mut self, addr: Address) {
        info!("AACPManager connecting to {} on PSM {:#06X}...", addr, PSM);
        let target_sa = SocketAddr::new(addr, AddressType::BrEdr, PSM);

        {
            let mut state = self.state.lock().await;
            state.airpods_mac = Some(addr);
        }

        let socket = match Socket::new_seq_packet() {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to create L2CAP socket: {}", e);
                return;
            }
        };

        // BlueZ 5.86+ requires an explicit security level on BR/EDR L2CAP sockets.
        // Without it the kernel accepts connect() but drops the channel before the
        // first send, returning ENOTCONN (os error 107).
        if let Err(e) = socket.set_security(Security {
            level: SecurityLevel::Medium,
            key_size: 0,
        }) {
            error!("Failed to set L2CAP security level: {}", e);
            return;
        }

        let seq_packet =
            match tokio::time::timeout(CONNECT_TIMEOUT, socket.connect(target_sa)).await {
                Ok(Ok(s)) => Arc::new(s),
                Ok(Err(e)) => {
                    error!("L2CAP connect failed: {}", e);
                    return;
                }
                Err(_) => {
                    error!("L2CAP connect timed out");
                    return;
                }
            };

        // Wait for connection to be fully established
        let start = Instant::now();
        loop {
            match seq_packet.peer_addr() {
                Ok(peer) if peer.cid != 0 => break,
                Ok(_) => { /* still waiting */ }
                Err(e) => {
                    if e.raw_os_error() == Some(107) {
                        // ENOTCONN
                        error!("Peer has disconnected during connection setup.");
                        return;
                    }
                    error!("Error getting peer address: {}", e);
                }
            }
            if start.elapsed() >= CONNECT_TIMEOUT {
                error!("Timed out waiting for L2CAP connection to be fully established.");
                return;
            }
            sleep(POLL_INTERVAL).await;
        }

        info!("L2CAP connection established with {}", addr);

        let (tx, rx) = mpsc::channel(128);

        let manager_clone = self.clone();
        {
            let mut state = self.state.lock().await;
            state.sender = Some(tx);
        }

        let mut tasks = self.tasks.lock().await;
        tasks.spawn(recv_thread(manager_clone, seq_packet.clone()));
        tasks.spawn(send_thread(rx, seq_packet));
    }

    async fn send_packet(&self, data: &[u8]) -> Result<()> {
        let state = self.state.lock().await;
        if let Some(sender) = &state.sender {
            sender.send(data.to_vec()).await.map_err(|e| {
                error!("Failed to send packet to channel: {}", e);
                Error::from(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "L2CAP send channel closed",
                ))
            })
        } else {
            error!("Cannot send packet, sender is not available.");
            Err(Error::from(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "L2CAP stream not connected",
            )))
        }
    }

    async fn send_data_packet(&self, data: &[u8]) -> Result<()> {
        let packet = [HEADER_BYTES.as_slice(), data].concat();
        self.send_packet(&packet).await
    }

    pub async fn set_event_channel(&self, tx: mpsc::UnboundedSender<AACPEvent>) {
        let mut state = self.state.lock().await;
        state.event_tx = Some(tx);
    }

    pub async fn subscribe_to_control_command(
        &self,
        identifier: ControlCommandIdentifiers,
        tx: mpsc::UnboundedSender<Vec<u8>>,
    ) {
        let mut state = self.state.lock().await;
        state
            .control_command_subscribers
            .entry(identifier)
            .or_default()
            .push(tx);
        // send initial value if available
        if let Some(status) = state
            .control_command_status_list
            .iter()
            .find(|s| s.identifier == identifier)
        {
            let _ = state
                .control_command_subscribers
                .get(&identifier)
                .expect("subscriber list just inserted")
                .last()
                .expect("tx just pushed")
                .send(status.value.clone());
        }
    }

    pub async fn receive_packet(&self, packet: &[u8]) {
        if !packet.starts_with(&HEADER_BYTES) {
            debug!(
                "Received packet does not start with expected header: {}",
                hex::encode(packet)
            );
            return;
        }
        if packet.len() < 5 {
            debug!("Received packet too short: {}", hex::encode(packet));
            return;
        }

        let opcode = packet[4];
        let payload = &packet[4..];

        // Broadcast opcode for strict init sequencing
        let _ = self.state.lock().await.opcode_tx.send(opcode);

        match opcode {
            opcodes::BATTERY_INFO => {
                if payload.len() < 3 {
                    error!("Battery Info packet too short: {}", hex::encode(payload));
                    return;
                }
                let count = payload[2] as usize;
                if payload.len() < 3 + count * 5 {
                    error!(
                        "Battery Info packet length mismatch: {}",
                        hex::encode(payload)
                    );
                    return;
                }
                let mut batteries = Vec::with_capacity(count);
                for i in 0..count {
                    let base_index = 3 + i * 5;
                    batteries.push(BatteryInfo {
                        component: match payload[base_index] {
                            0x01 => BatteryComponent::Headphone,
                            0x02 => BatteryComponent::Right,
                            0x04 => BatteryComponent::Left,
                            0x08 => BatteryComponent::Case,
                            _ => {
                                error!("Unknown battery component: {:#04x}", payload[base_index]);
                                continue;
                            }
                        },
                        level: payload[base_index + 2],
                        status: match payload[base_index + 3] {
                            0x01 => BatteryStatus::Charging,
                            0x02 => BatteryStatus::NotCharging,
                            0x04 => BatteryStatus::Disconnected,
                            0x05 => BatteryStatus::InUse,
                            _ => {
                                debug!("Unknown battery status: {:#04x}", payload[base_index + 3]);
                                continue;
                            }
                        },
                    });
                }
                let primary = batteries
                    .iter()
                    .find(|b| {
                        matches!(
                            b.component,
                            BatteryComponent::Left | BatteryComponent::Right
                        )
                    })
                    .map(|b| b.component);

                let mut state = self.state.lock().await;
                state.battery_info = batteries.clone();
                if let Some(p) = primary {
                    state.primary_pod = Some(p);
                }
                if let Some(ref tx) = state.event_tx {
                    let _ = tx.send(AACPEvent::BatteryInfo(batteries));
                }
                info!(
                    "Received Battery Info: {:?} (primary_pod={:?})",
                    state.battery_info, state.primary_pod
                );
            }
            opcodes::CONTROL_COMMAND => {
                if payload.len() < 7 {
                    error!("Control Command packet too short: {}", hex::encode(payload));
                    return;
                }
                let identifier_byte = payload[2];
                let value_bytes = &payload[3..7];

                let last_non_zero = value_bytes.iter().rposition(|&b| b != 0);
                let value = match last_non_zero {
                    Some(i) => value_bytes[..=i].to_vec(),
                    None => vec![0],
                };

                if let Ok(identifier) = ControlCommandIdentifiers::try_from(identifier_byte) {
                    let status = ControlCommandStatus {
                        identifier,
                        value: value.clone(),
                    };
                    let mut state = self.state.lock().await;
                    if let Some(existing) = state
                        .control_command_status_list
                        .iter_mut()
                        .find(|s| s.identifier == identifier)
                    {
                        existing.value = value.clone();
                    } else {
                        state.control_command_status_list.push(status.clone());
                    }
                    if identifier == ControlCommandIdentifiers::OwnsConnection {
                        state.owns = value_bytes[0] != 0;
                    }
                    if let Some(subscribers) = state.control_command_subscribers.get(&identifier) {
                        for sub in subscribers {
                            let _ = sub.send(value.clone());
                        }
                    }
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.send(AACPEvent::ControlCommand(status));
                    }
                    info!(
                        "Received Control Command: {:?}, value: {}",
                        identifier,
                        hex::encode(&value)
                    );
                } else {
                    debug!(
                        "Unknown Control Command identifier: {:#04x}",
                        identifier_byte
                    );
                }
            }
            opcodes::EAR_DETECTION => {
                let primary_status = packet[6];
                let secondary_status = packet[7];

                let parse_status = |b: u8| match b {
                    0x00 => EarDetectionStatus::InEar,
                    0x01 => EarDetectionStatus::OutOfEar,
                    0x02 => EarDetectionStatus::InCase,
                    0x03 => EarDetectionStatus::Disconnected,
                    _ => {
                        error!("Unknown ear detection status: {:#04x}", b);
                        EarDetectionStatus::OutOfEar
                    }
                };
                let ps = parse_status(primary_status);
                let ss = parse_status(secondary_status);

                let mut state = self.state.lock().await;
                let right_is_primary = state.primary_pod == Some(BatteryComponent::Right);
                let (left, right) = if right_is_primary {
                    (ss, ps) // index 0 = right, index 1 = left
                } else {
                    (ps, ss) // index 0 = left, index 1 = right
                };

                info!(
                    "Ear Detection: raw=[{:#04x},{:#04x}] right_is_primary={} → L={:?} R={:?}",
                    primary_status, secondary_status, right_is_primary, left, right
                );

                let old_left = state.ear_detection_left;
                let old_right = state.ear_detection_right;
                state.ear_detection_left = Some(left);
                state.ear_detection_right = Some(right);

                if let Some(ref tx) = state.event_tx {
                    let _ = tx.send(AACPEvent::EarDetection {
                        old_left,
                        old_right,
                        new_left: Some(left),
                        new_right: Some(right),
                    });
                }
            }
            opcodes::CONVERSATION_AWARENESS => {
                if packet.len() == 10 {
                    let status = packet[9];
                    let mut state = self.state.lock().await;
                    state.conversational_awareness_status = status;
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.send(AACPEvent::ConversationalAwareness(status));
                    }
                    info!("Received Conversation Awareness: {}", status);
                } else {
                    info!(
                        "Received Conversation Awareness packet with unexpected length: {}",
                        packet.len()
                    );
                }
            }
            opcodes::INFORMATION => {
                if payload.len() < 6 {
                    error!("Information packet too short: {}", hex::encode(payload));
                    return;
                }
                let data = &payload[4..];
                let mut index = 0;
                while index < data.len() && data[index] != 0x00 {
                    index += 1;
                }
                let mut strings = Vec::new();
                while index < data.len() {
                    while index < data.len() && data[index] == 0x00 {
                        index += 1;
                    }
                    if index >= data.len() {
                        break;
                    }
                    let start = index;
                    while index < data.len() && data[index] != 0x00 {
                        index += 1;
                    }
                    let str_bytes = &data[start..index];
                    if let Ok(s) = std::str::from_utf8(str_bytes) {
                        strings.push(s.to_string());
                    }
                }
                if !strings.is_empty() {
                    strings.remove(0);
                }
                let info = AirPodsInformation {
                    name: strings.first().cloned().unwrap_or_default(),
                    model_number: strings.get(1).cloned().unwrap_or_default(),
                    manufacturer: strings.get(2).cloned().unwrap_or_default(),
                    serial_number: strings.get(3).cloned().unwrap_or_default(),
                    version1: strings.get(4).cloned().unwrap_or_default(),
                    version2: strings.get(5).cloned().unwrap_or_default(),
                    hardware_revision: strings.get(6).cloned().unwrap_or_default(),
                    updater_identifier: strings.get(7).cloned().unwrap_or_default(),
                    left_serial_number: strings.get(8).cloned().unwrap_or_default(),
                    right_serial_number: strings.get(9).cloned().unwrap_or_default(),
                    version3: strings.get(10).cloned().unwrap_or_default(),
                    le_keys: AirPodsLEKeys {
                        irk: "".to_string(),
                        enc_key: "".to_string(),
                    },
                };
                let mut state = self.state.lock().await;
                if let Some(mac) = state.airpods_mac
                    && let Some(device_data) = state.devices.get_mut(&mac.to_string())
                {
                    device_data.name = info.name.clone();
                    device_data.information = Some(DeviceInformation::AirPods(info.clone()));
                }
                let Ok(json) = serde_json::to_string(&state.devices) else {
                    error!("Failed to serialize devices to JSON");
                    return;
                };
                if let Some(parent) = get_devices_path().parent()
                    && let Err(e) = tokio::fs::create_dir_all(&parent).await
                {
                    error!("Failed to create directory for devices: {}", e);
                    return;
                }
                if let Err(e) = tokio::fs::write(&get_devices_path(), json).await {
                    error!("Failed to save devices: {}", e);
                }
                info!("Received Information: {:?}", info);
                if let Some(tx) = &state.event_tx {
                    let _ = tx.send(AACPEvent::DeviceInfo(Box::new(info)));
                }
            }

            opcodes::PROXIMITY_KEYS_RSP => {
                if payload.len() < 4 {
                    error!(
                        "Proximity Keys Response packet too short: {}",
                        hex::encode(payload)
                    );
                    return;
                }
                let key_count = payload[2] as usize;
                debug!("Proximity Keys Response contains {} keys.", key_count);
                let mut offset = 3;
                let mut keys = Vec::new();
                for _ in 0..key_count {
                    if offset + 3 >= payload.len() {
                        error!(
                            "Proximity Keys Response packet too short while parsing keys: {}",
                            hex::encode(payload)
                        );
                        return;
                    }
                    let key_type = payload[offset];
                    let key_length = payload[offset + 2] as usize;
                    offset += 4;
                    if offset + key_length > payload.len() {
                        error!(
                            "Proximity Keys Response packet too short for key data: {}",
                            hex::encode(payload)
                        );
                        return;
                    }
                    let key_data = payload[offset..offset + key_length].to_vec();
                    keys.push((key_type, key_data));
                    offset += key_length;
                }
                info!(
                    "Received Proximity Keys Response: {:?}",
                    keys.iter()
                        .map(|(kt, kd)| (kt, hex::encode(kd)))
                        .collect::<Vec<_>>()
                );
                let mut state = self.state.lock().await;
                for (key_type, key_data) in &keys {
                    if let Ok(kt) = ProximityKeyType::try_from(*key_type)
                        && let Some(mac) = state.airpods_mac
                    {
                        let mac_str = mac.to_string();
                        let device_data =
                            state.devices.entry(mac_str.clone()).or_insert(DeviceData {
                                name: mac_str.clone(),
                                type_: DeviceType::AirPods,
                                information: None,
                            });
                        match kt {
                            ProximityKeyType::Irk => {
                                if let Some(DeviceInformation::AirPods(info)) =
                                    device_data.information.as_mut()
                                {
                                    info.le_keys.irk = hex::encode(key_data);
                                }
                            }
                            ProximityKeyType::EncKey => {
                                if let Some(DeviceInformation::AirPods(info)) =
                                    device_data.information.as_mut()
                                {
                                    info.le_keys.enc_key = hex::encode(key_data);
                                }
                            }
                        }
                    }
                }
                let Ok(json) = serde_json::to_string(&state.devices) else {
                    error!("Failed to serialize devices to JSON");
                    return;
                };
                if let Some(parent) = get_devices_path().parent()
                    && let Err(e) = tokio::fs::create_dir_all(&parent).await
                {
                    error!("Failed to create directory for devices: {}", e);
                    return;
                }
                if let Err(e) = tokio::fs::write(&get_devices_path(), json).await {
                    error!("Failed to save devices: {}", e);
                }
            }
            opcodes::STEM_PRESS => {
                let press_type = payload.get(2).and_then(|&b| match b {
                    0x05 => Some(StemPressType::Single),
                    0x06 => Some(StemPressType::Double),
                    0x07 => Some(StemPressType::Triple),
                    0x08 => Some(StemPressType::Long),
                    _ => None,
                });
                let bud = payload.get(3).and_then(|&b| match b {
                    0x01 => Some(StemPressBudType::Left),
                    0x02 => Some(StemPressBudType::Right),
                    _ => None,
                });
                info!(
                    "Received Stem Press packet: {:?} bud={:?} raw={}",
                    press_type,
                    bud,
                    hex::encode(payload)
                );
                if let Some(pt) = press_type
                    && let Some(ref tx) = self.state.lock().await.event_tx
                {
                    let _ = tx.send(AACPEvent::StemPress(pt, bud));
                }
            }
            opcodes::AUDIO_SOURCE => {
                if payload.len() < 9 {
                    error!("Audio Source packet too short: {}", hex::encode(payload));
                    return;
                }
                let mac = format!(
                    "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    payload[7], payload[6], payload[5], payload[4], payload[3], payload[2]
                );
                let typ = AudioSourceType::try_from(payload[8]).unwrap_or(AudioSourceType::None);
                let audio_source = AudioSource { mac, r#type: typ };
                let mut state = self.state.lock().await;
                state.audio_source = Some(audio_source.clone());
                if let Some(ref tx) = state.event_tx {
                    let _ = tx.send(AACPEvent::AudioSource(audio_source));
                }
                info!("Received Audio Source: {:?}", state.audio_source);
            }
            opcodes::CONNECTED_DEVICES => {
                if payload.len() < 3 {
                    error!(
                        "Connected Devices packet too short: {}",
                        hex::encode(payload)
                    );
                    return;
                }
                let count = payload[2] as usize;
                if payload.len() < 3 + count * 8 {
                    error!(
                        "Connected Devices packet length mismatch: {}",
                        hex::encode(payload)
                    );
                    return;
                }
                let mut devices = Vec::with_capacity(count);
                for i in 0..count {
                    let base = 5 + i * 8;
                    let mac = format!(
                        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        payload[base],
                        payload[base + 1],
                        payload[base + 2],
                        payload[base + 3],
                        payload[base + 4],
                        payload[base + 5]
                    );
                    let info1 = payload[base + 6];
                    let info2 = payload[base + 7];
                    devices.push(ConnectedDevice { mac, info1, info2 });
                }
                let mut state = self.state.lock().await;
                state.old_connected_devices = state.connected_devices.clone();
                state.connected_devices = devices.clone();
                if let Some(ref tx) = state.event_tx {
                    let _ = tx.send(AACPEvent::ConnectedDevices(
                        state.old_connected_devices.clone(),
                        devices,
                    ));
                }
                info!("Received Connected Devices: {:?}", state.connected_devices);
            }
            0x11 => {
                // Smart-Routing response — only the OwnershipToFalse notification matters.
                let packet_string = String::from_utf8_lossy(&payload[2..]);
                if packet_string.contains("SetOwnershipToFalse") {
                    info!("Received OwnershipToFalse request via smart-routing response");
                    if let Some(ref tx) = self.state.lock().await.event_tx {
                        let _ = tx.send(AACPEvent::OwnershipToFalseRequest);
                    }
                } else {
                    debug!("Smart-routing response (ignored): {}", packet_string);
                }
            }
            opcodes::EQ_DATA => {
                // Packet: opcode(1) pad(1) 0x84 0x00 0x02 0x02 Phone Media EQ[0..8]
                // payload[0] = opcode, so EQ bands start at payload[8]
                if payload.len() >= 16 {
                    let mut bands = [0u8; 8];
                    bands.copy_from_slice(&payload[8..16]);
                    let state = self.state.lock().await;
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.send(AACPEvent::EqData(bands));
                    }
                }
                debug!("Received EQ Data");
            }
            _ => debug!("Received unknown packet with opcode {:#04x}", opcode),
        }

        // Notify anyone waiting for a device response (replaces fixed sleep delays)
        self.state.lock().await.packet_received.notify_waiters();
    }

    pub async fn send_notification_request(&self) -> Result<()> {
        let opcode = [opcodes::REQUEST_NOTIFICATIONS, 0x00];
        let data = [0xFF, 0xFF, 0xFF, 0xFF];
        let packet = [opcode.as_slice(), data.as_slice()].concat();
        self.send_data_packet(&packet).await
    }

    pub async fn send_set_feature_flags_packet(&self) -> Result<()> {
        let opcode = [opcodes::SET_FEATURE_FLAGS, 0x00];
        let data = [0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let packet = [opcode.as_slice(), data.as_slice()].concat();
        self.send_data_packet(&packet).await
    }

    /// AapInitExt — sent to AirPods Pro 2/3/USB-C and AirPods 4 ANC to unlock Adaptive mode.
    /// Wire packet: 04 00 04 00 4d 00 0e 00 00 00 00 00 00 00
    pub async fn send_init_ext(&self) -> Result<()> {
        let data = [0x4d, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        self.send_data_packet(&data).await
    }

    pub async fn send_handshake(&self) -> Result<()> {
        let packet = [
            0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        self.send_packet(&packet).await
    }

    pub async fn send_proximity_keys_request(
        &self,
        key_types: Vec<ProximityKeyType>,
    ) -> Result<()> {
        let opcode = [opcodes::PROXIMITY_KEYS_REQ, 0x00];
        let data = vec![
            key_types.iter().fold(0u8, |acc, kt| acc | (*kt as u8)),
            0x00,
        ];
        let packet = [opcode.as_slice(), data.as_slice()].concat();
        self.send_data_packet(&packet).await
    }

    pub async fn send_rename_packet(&self, name: &str) -> Result<()> {
        let name_bytes = name.as_bytes();
        let size = name_bytes.len();
        let mut packet = Vec::with_capacity(6 + size);
        packet.push(opcodes::RENAME);
        packet.push(0x00);
        packet.push(0x01);
        packet.push(size as u8);
        packet.push(0x00);
        packet.extend_from_slice(name_bytes);
        self.send_data_packet(&packet).await
    }

    pub async fn send_control_command(
        &self,
        identifier: ControlCommandIdentifiers,
        value: &[u8],
    ) -> Result<()> {
        let opcode = [opcodes::CONTROL_COMMAND, 0x00];
        let mut data = vec![identifier as u8];
        for i in 0..4 {
            data.push(value.get(i).copied().unwrap_or(0));
        }
        let packet = [opcode.as_slice(), data.as_slice()].concat();
        self.send_data_packet(&packet).await
    }

    /// Request the current SSL (audio-routing) state from the device.
    pub async fn send_ssl_request(&self) -> Result<()> {
        self.send_data_packet(&[0x29, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
            .await
    }
}

async fn recv_thread(manager: AACPManager, sp: Arc<SeqPacket>) {
    let mut buf = vec![0u8; 1024];
    loop {
        match sp.recv(&mut buf).await {
            Ok(0) => {
                info!("Remote closed the connection.");
                break;
            }
            Ok(n) => {
                let data = &buf[..n];
                debug!("Received {} bytes: {}", n, hex::encode(data));
                manager.receive_packet(data).await;
            }
            Err(e) => {
                error!("Read error: {}", e);
                debug!(
                    "We have probably disconnected, clearing state variables (owns=false, connected_devices=empty, control_command_status_list=empty)."
                );
                let mut state = manager.state.lock().await;
                state.owns = false;
                state.connected_devices.clear();
                state.control_command_status_list.clear();
                break;
            }
        }
    }
    let mut state = manager.state.lock().await;
    state.sender = None;
    // Notify listeners that the L2CAP connection is gone so they can trigger reconnect
    if let Some(tx) = &state.event_tx {
        let _ = tx.send(AACPEvent::ConnectionLost);
    }
}

async fn send_thread(mut rx: mpsc::Receiver<Vec<u8>>, sp: Arc<SeqPacket>) {
    while let Some(data) = rx.recv().await {
        if let Err(e) = sp.send(&data).await {
            error!("Failed to send data: {}", e);
            break;
        }
        debug!("Sent {} bytes: {}", data.len(), hex::encode(&data));
    }
    info!("Send thread finished.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::time::timeout;

    /// Helper: build a manager wired to an event channel and return both.
    async fn manager_with_events() -> (AACPManager, UnboundedReceiver<AACPEvent>) {
        let m = AACPManager::new();
        let (tx, rx) = mpsc::unbounded_channel();
        m.set_event_channel(tx).await;
        (m, rx)
    }

    /// Helper: prepend the standard 4-byte AACP header to a payload.
    fn pkt(payload: &[u8]) -> Vec<u8> {
        let mut v = HEADER_BYTES.to_vec();
        v.extend_from_slice(payload);
        v
    }

    /// Drain an event from the channel within a short window.
    async fn next_event(rx: &mut UnboundedReceiver<AACPEvent>) -> Option<AACPEvent> {
        timeout(Duration::from_millis(100), rx.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn rejects_packet_without_header() {
        let (m, mut rx) = manager_with_events().await;
        m.receive_packet(&[0xFF, 0xFF, 0xFF]).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn rejects_packet_too_short_for_opcode() {
        let (m, mut rx) = manager_with_events().await;
        m.receive_packet(&HEADER_BYTES).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn battery_info_parses_all_components() {
        let (m, mut rx) = manager_with_events().await;
        // opcode(0x04) pad count=4 [comp, _, level, status, _]*4
        let payload = [
            opcodes::BATTERY_INFO,
            0x00,
            0x04,
            0x01,
            0x00,
            80,
            0x02,
            0x00, // headphone 80% NotCharging
            0x02,
            0x00,
            70,
            0x01,
            0x00, // right 70% Charging
            0x04,
            0x00,
            60,
            0x05,
            0x00, // left 60% InUse
            0x08,
            0x00,
            50,
            0x02,
            0x00, // case 50% NotCharging
        ];
        m.receive_packet(&pkt(&payload)).await;
        let ev = next_event(&mut rx).await.expect("BatteryInfo emitted");
        match ev {
            AACPEvent::BatteryInfo(b) => {
                assert_eq!(b.len(), 4);
                let comps: Vec<_> = b.iter().map(|x| x.component).collect();
                assert!(comps.contains(&BatteryComponent::Headphone));
                assert!(comps.contains(&BatteryComponent::Right));
                assert!(comps.contains(&BatteryComponent::Left));
                assert!(comps.contains(&BatteryComponent::Case));
                let left = b
                    .iter()
                    .find(|x| x.component == BatteryComponent::Left)
                    .unwrap();
                assert_eq!(left.level, 60);
                assert_eq!(left.status, BatteryStatus::InUse);
                let right = b
                    .iter()
                    .find(|x| x.component == BatteryComponent::Right)
                    .unwrap();
                assert_eq!(right.status, BatteryStatus::Charging);
            }
            other => panic!("expected BatteryInfo, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn battery_info_skips_unknown_status_byte() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [
            opcodes::BATTERY_INFO,
            0x00,
            0x02,
            0x04,
            0x00,
            75,
            0x02,
            0x00, // valid: left 75% NotCharging
            0x02,
            0x00,
            50,
            0xFE,
            0x00, // invalid status — should be skipped
        ];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::BatteryInfo(b) => {
                assert_eq!(b.len(), 1);
                assert_eq!(b[0].component, BatteryComponent::Left);
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn battery_info_truncated_packet_does_not_emit() {
        let (m, mut rx) = manager_with_events().await;
        // Says count=4 but only one entry's worth of bytes follows
        let payload = [
            opcodes::BATTERY_INFO,
            0x00,
            0x04,
            0x01,
            0x00,
            80,
            0x02,
            0x00,
        ];
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn battery_info_records_primary_pod() {
        let (m, _rx) = manager_with_events().await;
        let payload = [
            opcodes::BATTERY_INFO,
            0x00,
            0x01,
            0x02,
            0x00,
            70,
            0x02,
            0x00, // right
        ];
        m.receive_packet(&pkt(&payload)).await;
        let state = m.state.lock().await;
        assert_eq!(state.primary_pod, Some(BatteryComponent::Right));
    }

    #[tokio::test]
    async fn control_command_trims_trailing_zeros() {
        let (m, mut rx) = manager_with_events().await;
        // opcode pad identifier=ListeningMode(0x0D) value=[0x02, 0x00, 0x00, 0x00]
        let payload = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x02, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::ControlCommand(c) => {
                assert_eq!(c.identifier, ControlCommandIdentifiers::ListeningMode);
                assert_eq!(c.value, vec![0x02]);
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn control_command_all_zero_value_normalizes_to_single_zero() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x00, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::ControlCommand(c) => assert_eq!(c.value, vec![0x00]),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn control_command_unknown_identifier_emits_nothing() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [opcodes::CONTROL_COMMAND, 0x00, 0x7F, 0x01, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn control_command_owns_connection_updates_owns_flag() {
        let (m, _rx) = manager_with_events().await;
        // OwnsConnection (0x06) value = 1
        let payload = [opcodes::CONTROL_COMMAND, 0x00, 0x06, 0x01, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        assert!(m.state.lock().await.owns);

        let payload = [opcodes::CONTROL_COMMAND, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        assert!(!m.state.lock().await.owns);
    }

    #[tokio::test]
    async fn control_command_replaces_existing_status() {
        let (m, _rx) = manager_with_events().await;
        let p1 = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x02, 0x00, 0x00, 0x00];
        let p2 = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x03, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&p1)).await;
        m.receive_packet(&pkt(&p2)).await;
        let s = m.state.lock().await;
        let listening: Vec<_> = s
            .control_command_status_list
            .iter()
            .filter(|c| c.identifier == ControlCommandIdentifiers::ListeningMode)
            .collect();
        assert_eq!(listening.len(), 1);
        assert_eq!(listening[0].value, vec![0x03]);
    }

    #[tokio::test]
    async fn ear_detection_with_left_primary_passes_through() {
        let (m, mut rx) = manager_with_events().await;
        // Force primary pod to Left via a battery packet first
        let bat = [
            opcodes::BATTERY_INFO,
            0x00,
            0x01,
            0x04,
            0x00,
            50,
            0x02,
            0x00,
        ];
        m.receive_packet(&pkt(&bat)).await;
        let _ = next_event(&mut rx).await; // discard battery event

        // EarDetection: full packet form (header + opcode + filler + L + R)
        // receive_packet reads packet[6] as primary, packet[7] as secondary
        let p = [
            HEADER_BYTES[0],
            HEADER_BYTES[1],
            HEADER_BYTES[2],
            HEADER_BYTES[3],
            opcodes::EAR_DETECTION,
            0x00,
            0x00,
            0x01, // primary=InEar(0x00), secondary=OutOfEar(0x01)
        ];
        m.receive_packet(&p).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::EarDetection {
                new_left,
                new_right,
                ..
            } => {
                assert_eq!(new_left, Some(EarDetectionStatus::InEar));
                assert_eq!(new_right, Some(EarDetectionStatus::OutOfEar));
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn ear_detection_with_right_primary_swaps() {
        let (m, mut rx) = manager_with_events().await;
        // Right is primary → primary byte maps to right, secondary to left
        let bat = [
            opcodes::BATTERY_INFO,
            0x00,
            0x01,
            0x02,
            0x00,
            50,
            0x02,
            0x00,
        ];
        m.receive_packet(&pkt(&bat)).await;
        let _ = next_event(&mut rx).await;

        let p = [
            HEADER_BYTES[0],
            HEADER_BYTES[1],
            HEADER_BYTES[2],
            HEADER_BYTES[3],
            opcodes::EAR_DETECTION,
            0x00,
            0x00,
            0x01,
        ];
        m.receive_packet(&p).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::EarDetection {
                new_left,
                new_right,
                ..
            } => {
                assert_eq!(new_right, Some(EarDetectionStatus::InEar));
                assert_eq!(new_left, Some(EarDetectionStatus::OutOfEar));
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn conversation_awareness_parses() {
        let (m, mut rx) = manager_with_events().await;
        // Total packet length must equal 10 (header 4 + 6 payload)
        let p = [
            HEADER_BYTES[0],
            HEADER_BYTES[1],
            HEADER_BYTES[2],
            HEADER_BYTES[3],
            opcodes::CONVERSATION_AWARENESS,
            0,
            0,
            0,
            0,
            0x01,
        ];
        m.receive_packet(&p).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::ConversationalAwareness(s) => assert_eq!(s, 0x01),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn conversation_awareness_wrong_length_ignored() {
        let (m, mut rx) = manager_with_events().await;
        let p = [
            HEADER_BYTES[0],
            HEADER_BYTES[1],
            HEADER_BYTES[2],
            HEADER_BYTES[3],
            opcodes::CONVERSATION_AWARENESS,
            0,
            0,
        ];
        m.receive_packet(&p).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn audio_source_reverses_mac_byte_order() {
        let (m, mut rx) = manager_with_events().await;
        // Payload bytes [2..8] are the MAC in reverse order, byte 8 is the type.
        let payload = [
            opcodes::AUDIO_SOURCE,
            0x00,
            0x66,
            0x55,
            0x44,
            0x33,
            0x22,
            0x11, // reversed MAC
            AudioSourceType::Media as u8,
        ];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::AudioSource(src) => {
                assert_eq!(src.mac, "11:22:33:44:55:66");
                assert_eq!(src.r#type, AudioSourceType::Media);
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn audio_source_unknown_type_falls_back_to_none() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [
            opcodes::AUDIO_SOURCE,
            0x00,
            0x66,
            0x55,
            0x44,
            0x33,
            0x22,
            0x11,
            0xFE, // unknown type
        ];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::AudioSource(src) => assert_eq!(src.r#type, AudioSourceType::None),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn connected_devices_parses_count_and_macs() {
        let (m, mut rx) = manager_with_events().await;
        // opcode pad count [pad pad mac6 info1 info2]*count — base offset for first device is 5 (i=0 → base=5)
        let payload = [
            opcodes::CONNECTED_DEVICES,
            0x00,
            0x01,
            0x00,
            0x00, // padding so device entry starts at index 5
            0xAA,
            0xBB,
            0xCC,
            0xDD,
            0xEE,
            0xFF, // MAC
            0x42,
            0x43, // info1, info2
        ];
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::ConnectedDevices(_old, new) => {
                assert_eq!(new.len(), 1);
                assert_eq!(new[0].mac, "AA:BB:CC:DD:EE:FF");
                assert_eq!(new[0].info1, 0x42);
                assert_eq!(new[0].info2, 0x43);
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn connected_devices_truncated_packet_emits_nothing() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [opcodes::CONNECTED_DEVICES, 0x00, 0x02, 0x00, 0x00, 0xAA];
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn stem_press_parses_known_combos() {
        let cases = [
            (
                0x05,
                0x01,
                StemPressType::Single,
                Some(StemPressBudType::Left),
            ),
            (
                0x06,
                0x02,
                StemPressType::Double,
                Some(StemPressBudType::Right),
            ),
            (
                0x07,
                0x01,
                StemPressType::Triple,
                Some(StemPressBudType::Left),
            ),
            (
                0x08,
                0x02,
                StemPressType::Long,
                Some(StemPressBudType::Right),
            ),
        ];
        for (pt, bud, expected_pt, expected_bud) in cases {
            let (m, mut rx) = manager_with_events().await;
            let payload = [opcodes::STEM_PRESS, 0x00, pt, bud];
            m.receive_packet(&pkt(&payload)).await;
            match next_event(&mut rx).await.expect("event") {
                AACPEvent::StemPress(p, b) => {
                    assert_eq!(p, expected_pt);
                    assert_eq!(b, expected_bud);
                }
                _ => panic!(),
            }
        }
    }

    #[tokio::test]
    async fn stem_press_unknown_type_no_event() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [opcodes::STEM_PRESS, 0x00, 0xAB, 0x01];
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn eq_data_parses_8_bands() {
        let (m, mut rx) = manager_with_events().await;
        // payload[0]=opcode payload[1..8] padding payload[8..16] EQ bands
        let mut payload = vec![opcodes::EQ_DATA, 0, 0, 0, 0, 0, 0, 0];
        payload.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::EqData(b) => assert_eq!(b, [1, 2, 3, 4, 5, 6, 7, 8]),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn unknown_opcode_does_not_panic_or_emit() {
        let (m, mut rx) = manager_with_events().await;
        let payload = [0xAB, 0x00, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn smart_routing_ownership_to_false_emits() {
        let (m, mut rx) = manager_with_events().await;
        let mut payload = vec![0x11, 0x00];
        payload.extend_from_slice(b"SetOwnershipToFalse");
        m.receive_packet(&pkt(&payload)).await;
        match next_event(&mut rx).await.expect("event") {
            AACPEvent::OwnershipToFalseRequest => {}
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn smart_routing_other_message_silent() {
        let (m, mut rx) = manager_with_events().await;
        let mut payload = vec![0x11, 0x00];
        payload.extend_from_slice(b"SomeOtherMessage");
        m.receive_packet(&pkt(&payload)).await;
        assert!(next_event(&mut rx).await.is_none());
    }

    #[tokio::test]
    async fn subscriber_receives_initial_value() {
        let (m, _rx) = manager_with_events().await;
        // Push a control command so a value exists
        let p = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x02, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&p)).await;

        let (sub_tx, mut sub_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        m.subscribe_to_control_command(ControlCommandIdentifiers::ListeningMode, sub_tx)
            .await;
        let v = timeout(Duration::from_millis(100), sub_rx.recv())
            .await
            .unwrap();
        assert_eq!(v, Some(vec![0x02]));
    }

    #[tokio::test]
    async fn subscriber_gets_subsequent_updates() {
        let (m, _rx) = manager_with_events().await;
        let (sub_tx, mut sub_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        m.subscribe_to_control_command(ControlCommandIdentifiers::ListeningMode, sub_tx)
            .await;

        let p = [opcodes::CONTROL_COMMAND, 0x00, 0x0D, 0x03, 0x00, 0x00, 0x00];
        m.receive_packet(&pkt(&p)).await;
        let v = timeout(Duration::from_millis(100), sub_rx.recv())
            .await
            .unwrap();
        assert_eq!(v, Some(vec![0x03]));
    }

    #[test]
    fn control_command_identifier_roundtrip() {
        // Every variant we map in TryFrom should roundtrip.
        let cases = [
            (0x01u8, ControlCommandIdentifiers::MicMode),
            (0x05, ControlCommandIdentifiers::ButtonSendMode),
            (0x0D, ControlCommandIdentifiers::ListeningMode),
            (0x1A, ControlCommandIdentifiers::ListeningModeConfigs),
            (0x34, ControlCommandIdentifiers::AllowOffOption),
            (0x06, ControlCommandIdentifiers::OwnsConnection),
        ];
        for (byte, expected) in cases {
            assert_eq!(ControlCommandIdentifiers::try_from(byte).unwrap(), expected);
            assert_eq!(expected as u8, byte);
        }
        assert!(ControlCommandIdentifiers::try_from(0xFEu8).is_err());
    }
}
