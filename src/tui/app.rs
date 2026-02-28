use crate::bluetooth::aacp::{AACPEvent, BatteryComponent, BatteryStatus, ControlCommandIdentifiers};
use crate::devices::enums::{AirPodsNoiseControlMode, NothingAncMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceCommand {
    ControlCommand(ControlCommandIdentifiers, Vec<u8>),
    Rename(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppEvent {
    DeviceConnected { mac: String, name: String, is_nothing: bool, product_id: u16 },
    DeviceDisconnected(String),
    AACPEvent(String, AACPEvent),
    AudioUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedSection {
    NoiseControl,
    Settings,
}

impl FocusedSection {
    pub fn next(self) -> Self {
        match self {
            Self::NoiseControl => Self::Settings,
            Self::Settings => Self::NoiseControl,
        }
    }

    pub fn prev(self) -> Self {
        self.next() // only 2 variants, prev == next
    }
}

#[derive(Debug, Clone)]
pub struct AirPodsDeviceState {
    pub name: String,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub battery_left: Option<(u8, BatteryStatus)>,
    pub battery_right: Option<(u8, BatteryStatus)>,
    pub battery_case: Option<(u8, BatteryStatus)>,
    pub battery_headphone: Option<(u8, BatteryStatus)>,
    pub product_id: u16,
    pub has_anc: bool,
    pub has_adaptive: bool,
    pub listening_mode: AirPodsNoiseControlMode,
    pub allow_off_mode: bool,
    pub conversation_awareness: bool,
    pub auto_connect: Option<bool>,
    pub one_bud_anc: bool,
    pub volume_swipe: bool,
    pub adaptive_volume: bool,
    // New hardware settings
    pub press_speed: Option<u8>,
    pub press_hold_duration: Option<u8>,
    pub tone_volume: Option<u8>,
    pub volume_swipe_length: Option<u8>,
    pub adaptive_noise_level: Option<u8>,
}

impl AirPodsDeviceState {
    pub fn new(name: String) -> Self {
        Self {
            name,
            model: None,
            serial_number: None,
            battery_left: None,
            battery_right: None,
            battery_case: None,
            battery_headphone: None,
            product_id: 0,
            has_anc: true,
            has_adaptive: false,
            listening_mode: AirPodsNoiseControlMode::NoiseCancellation,
            allow_off_mode: false,
            conversation_awareness: false,
            auto_connect: None,
            one_bud_anc: false,
            volume_swipe: false,
            adaptive_volume: false,
            press_speed: None,
            press_hold_duration: None,
            tone_volume: None,
            volume_swipe_length: None,
            adaptive_noise_level: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NothingDeviceState {
    pub name: String,
    pub anc_mode: NothingAncMode,
}

#[derive(Debug, Clone)]
pub enum DeviceState {
    AirPods(AirPodsDeviceState),
    Nothing(NothingDeviceState),
}

impl DeviceState {
    pub fn name(&self) -> &str {
        match self {
            DeviceState::AirPods(s) => &s.name,
            DeviceState::Nothing(s) => &s.name,
        }
    }
}

pub struct App {
    pub devices: HashMap<String, DeviceState>,
    pub device_order: Vec<String>,
    pub selected_device_idx: usize,
    pub focused_section: FocusedSection,
    pub section_row: usize,
    pub rx: UnboundedReceiver<AppEvent>,
    pub should_quit: bool,
    pub command_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, DeviceCommand)>>,
    pub rename_mode: Option<String>,
    pub audio_unavailable: bool,
}

impl App {
    pub fn new(
        rx: UnboundedReceiver<AppEvent>,
        command_tx: tokio::sync::mpsc::UnboundedSender<(String, DeviceCommand)>,
    ) -> Self {
        Self {
            devices: HashMap::new(),
            device_order: Vec::new(),
            selected_device_idx: 0,
            focused_section: FocusedSection::NoiseControl,
            section_row: 0,
            rx,
            should_quit: false,
            command_tx: Some(command_tx),
            rename_mode: None,
            audio_unavailable: false,
        }
    }

    pub fn selected_mac(&self) -> Option<&String> {
        self.device_order.get(self.selected_device_idx)
    }

    pub fn selected_device(&self) -> Option<&DeviceState> {
        self.selected_mac().and_then(|mac| self.devices.get(mac))
    }

    /// Number of rows in the Noise Control section
    pub fn noise_control_rows(&self) -> usize {
        match self.selected_device() {
            Some(DeviceState::AirPods(s)) if s.has_anc => {
                if s.has_adaptive { 3 } else { 2 }
            }
            _ => 0,
        }
    }

    /// Build the list of settings items for the current AirPods device.
    /// Returns Vec<(label, SettingsItemKind)>.
    pub fn settings_items(&self) -> Vec<SettingsItem> {
        let Some(DeviceState::AirPods(s)) = self.selected_device() else {
            return Vec::new();
        };
        let info = crate::devices::apple_models::model_info(s.product_id);
        let mut items = Vec::new();

        if s.has_anc && info.has_conversation_awareness {
            items.push(SettingsItem::Toggle {
                label: "Conversation Awareness",
                value: s.conversation_awareness,
                cmd: ControlCommandIdentifiers::ConversationDetectConfig,
            });
        }
        if s.has_anc {
            items.push(SettingsItem::Toggle {
                label: "NC with One AirPod",
                value: s.one_bud_anc,
                cmd: ControlCommandIdentifiers::OneBudAncMode,
            });
        }
        items.push(SettingsItem::Toggle {
            label: "Personalized Volume",
            value: s.adaptive_volume,
            cmd: ControlCommandIdentifiers::AdaptiveVolumeConfig,
        });
        if info.has_stem_controls {
            items.push(SettingsItem::Toggle {
                label: "Volume Swipe",
                value: s.volume_swipe,
                cmd: ControlCommandIdentifiers::VolumeSwipeMode,
            });
            items.push(SettingsItem::Enum {
                label: "Press Speed",
                value: s.press_speed.unwrap_or(0),
                options: &["Default", "Slower", "Slowest"],
                cmd: ControlCommandIdentifiers::DoubleClickInterval,
            });
            items.push(SettingsItem::Enum {
                label: "Press & Hold",
                value: s.press_hold_duration.unwrap_or(0),
                options: &["Default", "Shorter", "Shortest"],
                cmd: ControlCommandIdentifiers::ClickHoldInterval,
            });
        }
        items.push(SettingsItem::Slider {
            label: "Tone Volume",
            value: s.tone_volume.unwrap_or(50),
            min: 15,
            max: 100,
            cmd: ControlCommandIdentifiers::ChimeVolume,
        });
        if info.has_stem_controls {
            items.push(SettingsItem::Enum {
                label: "Volume Swipe Length",
                value: s.volume_swipe_length.unwrap_or(0),
                options: &["Default", "Longer", "Longest"],
                cmd: ControlCommandIdentifiers::VolumeSwipeInterval,
            });
        }
        if s.has_adaptive && s.listening_mode == AirPodsNoiseControlMode::Adaptive {
            items.push(SettingsItem::Slider {
                label: "Adaptive Noise Level",
                value: s.adaptive_noise_level.unwrap_or(50),
                min: 0,
                max: 100,
                cmd: ControlCommandIdentifiers::AutoAncStrength,
            });
        }
        items.push(SettingsItem::Toggle {
            label: "Auto Connect",
            value: s.auto_connect.unwrap_or(true),
            cmd: ControlCommandIdentifiers::AllowAutoConnect,
        });
        items
    }

    /// Drain all pending AppEvents and update state.
    pub fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AppEvent::DeviceConnected { mac, name, is_nothing, product_id } => {
                    if self.devices.contains_key(&mac) {
                        match self.devices.get_mut(&mac) {
                            Some(DeviceState::AirPods(s)) => {
                                s.name = name;
                                // Fix race: AACP events may arrive before DeviceConnected,
                                // so update product_id and model info now
                                if product_id != 0 && s.product_id == 0 {
                                    let info = crate::devices::apple_models::model_info(product_id);
                                    s.product_id = product_id;
                                    s.has_anc = info.has_anc;
                                    s.has_adaptive = info.has_adaptive;
                                    s.model = Some(info.name.to_string());
                                }
                            }
                            Some(DeviceState::Nothing(s)) => s.name = name,
                            None => {}
                        }
                    } else {
                        let state = if is_nothing {
                            DeviceState::Nothing(NothingDeviceState {
                                name,
                                anc_mode: NothingAncMode::Off,
                            })
                        } else {
                            let info = crate::devices::apple_models::model_info(product_id);
                            let mut s = AirPodsDeviceState::new(name);
                            s.product_id = product_id;
                            s.has_anc = info.has_anc;
                            s.has_adaptive = info.has_adaptive;
                            if product_id != 0 {
                                s.model = Some(info.name.to_string());
                            }
                            DeviceState::AirPods(s)
                        };
                        self.devices.insert(mac.clone(), state);
                        self.device_order.push(mac);
                    }
                }
                AppEvent::DeviceDisconnected(mac) => {
                    self.devices.remove(&mac);
                    self.device_order.retain(|m| m != &mac);
                    if self.selected_device_idx >= self.device_order.len() && !self.device_order.is_empty() {
                        self.selected_device_idx = self.device_order.len() - 1;
                    }
                }
                AppEvent::AACPEvent(mac, event) => {
                    self.handle_aacp_event(&mac, event);
                }
                AppEvent::AudioUnavailable => {
                    self.audio_unavailable = true;
                }
            }
        }
    }

    fn handle_aacp_event(&mut self, mac: &str, event: AACPEvent) {
        if !self.devices.contains_key(mac) {
            let mac_owned = mac.to_string();
            self.devices.insert(
                mac_owned.clone(),
                DeviceState::AirPods(AirPodsDeviceState::new("AirPods".to_string())),
            );
            self.device_order.push(mac_owned);
        }

        if let Some(DeviceState::AirPods(s)) = self.devices.get_mut(mac) {
            if s.name == mac {
                s.name = "AirPods".to_string();
            }
        }

        if let Some(DeviceState::AirPods(state)) = self.devices.get_mut(mac) {
            match event {
                AACPEvent::BatteryInfo(infos) => {
                    for b in infos {
                        match b.component {
                            BatteryComponent::Left => {
                                state.battery_left = Some((b.level, b.status));
                            }
                            BatteryComponent::Right => {
                                state.battery_right = Some((b.level, b.status));
                            }
                            BatteryComponent::Case => {
                                state.battery_case = Some((b.level, b.status));
                            }
                            BatteryComponent::Headphone => {
                                state.battery_headphone = Some((b.level, b.status));
                            }
                        }
                    }
                    let mut content = String::new();
                    if let Some((l, _)) = state.battery_left { content.push_str(&format!("LEFT={}\n", l)); }
                    if let Some((r, _)) = state.battery_right { content.push_str(&format!("RIGHT={}\n", r)); }
                    if let Some((c, _)) = state.battery_case { content.push_str(&format!("CASE={}\n", c)); }
                    let _ = std::fs::write(crate::utils::runtime_dir().join("airpods-battery.env"), content);
                }
                AACPEvent::DeviceInfo(info) => {
                    if !info.name.is_empty() {
                        state.name = info.name;
                    }
                    // Don't overwrite model with raw Apple model number (e.g. "A2931").
                    // The human-readable name comes from product_id lookup in DeviceConnected.
                    if !info.serial_number.is_empty() {
                        state.serial_number = Some(info.serial_number);
                    }
                }
                AACPEvent::ControlCommand(cmd) => match cmd.identifier {
                    ControlCommandIdentifiers::ListeningMode => {
                        if let Some(byte) = cmd.value.first() {
                            state.listening_mode = AirPodsNoiseControlMode::from_byte(byte);
                        }
                    }
                    ControlCommandIdentifiers::AllowOffOption => {
                        if let Some(byte) = cmd.value.first() {
                            state.allow_off_mode = *byte != 0x00;
                        }
                    }
                    ControlCommandIdentifiers::ConversationDetectConfig => {
                        if let Some(byte) = cmd.value.first() {
                            state.conversation_awareness = *byte == 0x01;
                        }
                    }
                    ControlCommandIdentifiers::AllowAutoConnect => {
                        if let Some(byte) = cmd.value.first() {
                            state.auto_connect = Some(*byte != 0x00);
                        }
                    }
                    ControlCommandIdentifiers::OneBudAncMode => {
                        if let Some(byte) = cmd.value.first() {
                            state.one_bud_anc = *byte == 0x01;
                        }
                    }
                    ControlCommandIdentifiers::VolumeSwipeMode => {
                        if let Some(byte) = cmd.value.first() {
                            state.volume_swipe = *byte == 0x01;
                        }
                    }
                    ControlCommandIdentifiers::AdaptiveVolumeConfig => {
                        if let Some(byte) = cmd.value.first() {
                            state.adaptive_volume = *byte == 0x01;
                        }
                    }
                    ControlCommandIdentifiers::DoubleClickInterval => {
                        if let Some(byte) = cmd.value.first() {
                            state.press_speed = Some(*byte);
                        }
                    }
                    ControlCommandIdentifiers::ClickHoldInterval => {
                        if let Some(byte) = cmd.value.first() {
                            state.press_hold_duration = Some(*byte);
                        }
                    }
                    ControlCommandIdentifiers::ChimeVolume => {
                        if let Some(byte) = cmd.value.first() {
                            state.tone_volume = Some(*byte);
                        }
                    }
                    ControlCommandIdentifiers::VolumeSwipeInterval => {
                        if let Some(byte) = cmd.value.first() {
                            state.volume_swipe_length = Some(*byte);
                        }
                    }
                    ControlCommandIdentifiers::AutoAncStrength => {
                        if let Some(byte) = cmd.value.first() {
                            state.adaptive_noise_level = Some(*byte);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    pub fn send_command(&self, mac: &str, id: ControlCommandIdentifiers, value: Vec<u8>) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send((mac.to_string(), DeviceCommand::ControlCommand(id, value)));
        }
    }

    pub fn send_rename(&self, mac: &str, name: String) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send((mac.to_string(), DeviceCommand::Rename(name)));
        }
    }
}

/// Describes a single settings row, used by both UI and event handling.
#[derive(Debug, Clone)]
pub enum SettingsItem {
    Toggle {
        label: &'static str,
        value: bool,
        cmd: ControlCommandIdentifiers,
    },
    Enum {
        label: &'static str,
        value: u8,
        options: &'static [&'static str],
        cmd: ControlCommandIdentifiers,
    },
    Slider {
        label: &'static str,
        value: u8,
        min: u8,
        max: u8,
        cmd: ControlCommandIdentifiers,
    },
}
