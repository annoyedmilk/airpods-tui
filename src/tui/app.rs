use crate::bluetooth::aacp::{
    AACPEvent, BatteryComponent, BatteryStatus, ConnectedDevice, ControlCommandIdentifiers,
    EarDetectionStatus,
};
use crate::devices::enums::AirPodsNoiseControlMode;
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
    DeviceConnected {
        mac: String,
        name: String,
        product_id: u16,
    },
    DeviceDisconnected(String),
    AACPEvent(String, Box<crate::bluetooth::aacp::AACPEvent>),
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
    pub mic_mode: Option<u8>,
    // Ear detection
    pub ear_left: Option<EarDetectionStatus>,
    pub ear_right: Option<EarDetectionStatus>,
    // Device info extras
    pub firmware: Option<String>,
    pub hardware_revision: Option<String>,
    pub left_serial: Option<String>,
    pub right_serial: Option<String>,
    // Peer devices
    pub peer_devices: Vec<ConnectedDevice>,
    // Headphone Accommodation EQ (read-only, from device)
    pub eq_bands: Option<[u8; 8]>,
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
            mic_mode: None,
            ear_left: None,
            ear_right: None,
            firmware: None,
            hardware_revision: None,
            left_serial: None,
            right_serial: None,
            peer_devices: Vec::new(),
            eq_bands: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeviceState {
    AirPods(AirPodsDeviceState),
}

impl DeviceState {
    pub fn name(&self) -> &str {
        match self {
            DeviceState::AirPods(s) => &s.name,
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
    pub show_info: bool,
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
            show_info: false,
            audio_unavailable: false,
        }
    }

    pub fn selected_mac(&self) -> Option<&String> {
        self.device_order.get(self.selected_device_idx)
    }

    pub fn selected_device(&self) -> Option<&DeviceState> {
        self.selected_mac().and_then(|mac| self.devices.get(mac))
    }

    /// Number of rows in the Noise Control section.
    /// Must match the length of `ui::noise_mode_list`.
    pub fn noise_control_rows(&self) -> usize {
        match self.selected_device() {
            Some(DeviceState::AirPods(s)) if s.has_anc => {
                crate::tui::ui::noise_mode_list(s.has_adaptive, s.allow_off_mode).len()
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
        items.push(SettingsItem::Enum {
            label: "Mic Mode",
            value: s.mic_mode.unwrap_or(2),
            options: &["Always Left", "Always Right", "Automatic"],
            cmd: ControlCommandIdentifiers::MicMode,
        });
        items.push(SettingsItem::Toggle {
            label: "Auto Connect",
            value: s.auto_connect.unwrap_or(true),
            cmd: ControlCommandIdentifiers::AllowAutoConnect,
        });
        items
    }

    /// Handle a single AppEvent and update state.
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::DeviceConnected {
                mac,
                name,
                product_id,
            } => {
                if self.devices.contains_key(&mac) {
                    if let Some(DeviceState::AirPods(s)) = self.devices.get_mut(&mac) {
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
                } else {
                    let info = crate::devices::apple_models::model_info(product_id);
                    let mut s = AirPodsDeviceState::new(name);
                    s.product_id = product_id;
                    s.has_anc = info.has_anc;
                    s.has_adaptive = info.has_adaptive;
                    if product_id != 0 {
                        s.model = Some(info.name.to_string());
                    }
                    self.devices.insert(mac.clone(), DeviceState::AirPods(s));
                    self.device_order.push(mac);
                }
            }
            AppEvent::DeviceDisconnected(mac) => {
                self.devices.remove(&mac);
                self.device_order.retain(|m| m != &mac);
                if self.selected_device_idx >= self.device_order.len()
                    && !self.device_order.is_empty()
                {
                    self.selected_device_idx = self.device_order.len() - 1;
                }
            }
            AppEvent::AACPEvent(mac, event) => {
                self.handle_aacp_event(&mac, *event);
            }
            AppEvent::AudioUnavailable => {
                self.audio_unavailable = true;
            }
        }
    }

    /// Drain all pending AppEvents and update state.
    pub fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.handle_event(event);
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

        if let Some(DeviceState::AirPods(s)) = self.devices.get_mut(mac)
            && s.name == mac
        {
            s.name = "AirPods".to_string();
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
                                // Only update if not disconnected — preserve last known good value
                                if b.status != BatteryStatus::Disconnected {
                                    state.battery_case = Some((b.level, b.status));
                                }
                            }
                            BatteryComponent::Headphone => {
                                state.battery_headphone = Some((b.level, b.status));
                            }
                        }
                    }
                    let bat_left = state.battery_left.map(|(l, _)| l);
                    let bat_right = state.battery_right.map(|(r, _)| r);
                    let bat_case = state.battery_case.map(|(c, _)| c);
                    let bat_headphone = state.battery_headphone.map(|(h, _)| h);
                    // Write battery env file in a background thread to avoid blocking the TUI loop
                    std::thread::spawn(move || {
                        crate::utils::write_battery_env(
                            bat_left,
                            bat_right,
                            bat_case,
                            bat_headphone,
                        );
                    });
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
                    if !info.version1.is_empty() {
                        state.firmware = Some(info.version1);
                    }
                    if !info.hardware_revision.is_empty() {
                        state.hardware_revision = Some(info.hardware_revision);
                    }
                    if !info.left_serial_number.is_empty() {
                        state.left_serial = Some(info.left_serial_number);
                    }
                    if !info.right_serial_number.is_empty() {
                        state.right_serial = Some(info.right_serial_number);
                    }
                }
                AACPEvent::EarDetection {
                    new_left,
                    new_right,
                    ..
                } => {
                    state.ear_left = new_left;
                    state.ear_right = new_right;
                }
                AACPEvent::ConnectedDevices(_, new_devices) => {
                    state.peer_devices = new_devices;
                }
                AACPEvent::EqData(bands) => {
                    state.eq_bands = Some(bands);
                }
                AACPEvent::ControlCommand(cmd) => match cmd.identifier {
                    ControlCommandIdentifiers::ListeningMode => {
                        if let Some(byte) = cmd.value.first() {
                            state.listening_mode = AirPodsNoiseControlMode::from_byte(*byte);
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
                    ControlCommandIdentifiers::MicMode => {
                        if let Some(byte) = cmd.value.first() {
                            // AACP uses 1-indexed (0x01=Left, 0x02=Right, 0x03=Auto)
                            // We store 0-indexed for the Enum widget
                            state.mic_mode = Some(byte.saturating_sub(1));
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    pub fn send_command(&self, mac: &str, id: ControlCommandIdentifiers, value: Vec<u8>) {
        if let Some(tx) = &self.command_tx
            && let Err(e) = tx.send((mac.to_string(), DeviceCommand::ControlCommand(id, value)))
        {
            log::warn!("Failed to send control command {:?}: {}", id, e);
        }
    }

    pub fn send_rename(&self, mac: &str, name: String) {
        if let Some(tx) = &self.command_tx
            && let Err(e) = tx.send((mac.to_string(), DeviceCommand::Rename(name.clone())))
        {
            log::warn!("Failed to send rename '{}': {}", name, e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::aacp::{
        AACPEvent as AE, BatteryComponent, BatteryInfo, BatteryStatus, ControlCommandIdentifiers,
        ControlCommandStatus, EarDetectionStatus,
    };
    use tokio::sync::mpsc;

    const MAC: &str = "AA:BB:CC:DD:EE:FF";
    const PRO2: u16 = 0x2014; // ANC + Adaptive + Stem + CA
    const AIRPODS3: u16 = 0x2013; // No ANC, has stem
    const MAX: u16 = 0x200a; // ANC, no stem

    /// Build an App with a wired command channel and discardable rx.
    fn mk_app() -> (App, mpsc::UnboundedReceiver<(String, DeviceCommand)>) {
        let (_event_tx, event_rx) = mpsc::unbounded_channel::<AppEvent>();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        (App::new(event_rx, cmd_tx), cmd_rx)
    }

    fn connected(mac: &str, name: &str, product_id: u16) -> AppEvent {
        AppEvent::DeviceConnected {
            mac: mac.into(),
            name: name.into(),
            product_id,
        }
    }

    fn aacp(mac: &str, e: AE) -> AppEvent {
        AppEvent::AACPEvent(mac.into(), Box::new(e))
    }

    fn airpods<'a>(app: &'a App, mac: &str) -> &'a AirPodsDeviceState {
        match app.devices.get(mac) {
            Some(DeviceState::AirPods(s)) => s,
            _ => panic!("no AirPods state for {}", mac),
        }
    }

    #[test]
    fn focused_section_cycles_two_states() {
        assert_eq!(
            FocusedSection::NoiseControl.next(),
            FocusedSection::Settings
        );
        assert_eq!(
            FocusedSection::Settings.next(),
            FocusedSection::NoiseControl
        );
        assert_eq!(
            FocusedSection::NoiseControl.prev(),
            FocusedSection::Settings
        );
        assert_eq!(
            FocusedSection::Settings.prev(),
            FocusedSection::NoiseControl
        );
    }

    #[test]
    fn device_connected_creates_state_with_model_info() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "MyPods", PRO2));
        assert_eq!(app.device_order, vec![MAC]);
        let s = airpods(&app, MAC);
        assert_eq!(s.name, "MyPods");
        assert_eq!(s.product_id, PRO2);
        assert!(s.has_anc);
        assert!(s.has_adaptive);
        assert_eq!(s.model.as_deref(), Some("AirPods Pro 2"));
    }

    #[test]
    fn device_connected_zero_product_id_keeps_model_unset() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "MyPods", 0));
        let s = airpods(&app, MAC);
        assert!(s.model.is_none());
    }

    #[test]
    fn second_connect_with_real_product_id_backfills_model() {
        let (mut app, _) = mk_app();
        // AACP event arrives before DeviceConnected (race); state is created with defaults
        app.handle_event(aacp(MAC, AE::BatteryInfo(vec![])));
        let s = airpods(&app, MAC);
        assert_eq!(s.product_id, 0);

        // Now the proper DeviceConnected arrives
        app.handle_event(connected(MAC, "MyPods", PRO2));
        let s = airpods(&app, MAC);
        assert_eq!(s.product_id, PRO2);
        assert!(s.has_adaptive);
        assert_eq!(s.model.as_deref(), Some("AirPods Pro 2"));
    }

    #[test]
    fn device_disconnected_removes_and_clamps_index() {
        let (mut app, _) = mk_app();
        app.handle_event(connected("A", "a", PRO2));
        app.handle_event(connected("B", "b", PRO2));
        app.selected_device_idx = 1;
        app.handle_event(AppEvent::DeviceDisconnected("B".into()));
        assert_eq!(app.device_order, vec!["A".to_string()]);
        assert_eq!(app.selected_device_idx, 0);
    }

    #[test]
    fn battery_info_populates_components() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(
            MAC,
            AE::BatteryInfo(vec![
                BatteryInfo {
                    component: BatteryComponent::Left,
                    level: 80,
                    status: BatteryStatus::NotCharging,
                },
                BatteryInfo {
                    component: BatteryComponent::Right,
                    level: 70,
                    status: BatteryStatus::Charging,
                },
                BatteryInfo {
                    component: BatteryComponent::Case,
                    level: 50,
                    status: BatteryStatus::NotCharging,
                },
            ]),
        ));
        let s = airpods(&app, MAC);
        assert_eq!(s.battery_left, Some((80, BatteryStatus::NotCharging)));
        assert_eq!(s.battery_right, Some((70, BatteryStatus::Charging)));
        assert_eq!(s.battery_case, Some((50, BatteryStatus::NotCharging)));
    }

    #[test]
    fn case_battery_disconnected_does_not_clobber_previous() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(
            MAC,
            AE::BatteryInfo(vec![BatteryInfo {
                component: BatteryComponent::Case,
                level: 40,
                status: BatteryStatus::NotCharging,
            }]),
        ));
        app.handle_event(aacp(
            MAC,
            AE::BatteryInfo(vec![BatteryInfo {
                component: BatteryComponent::Case,
                level: 0,
                status: BatteryStatus::Disconnected,
            }]),
        ));
        assert_eq!(
            airpods(&app, MAC).battery_case,
            Some((40, BatteryStatus::NotCharging))
        );
    }

    #[test]
    fn ear_detection_event_updates_state() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(
            MAC,
            AE::EarDetection {
                old_left: None,
                old_right: None,
                new_left: Some(EarDetectionStatus::InEar),
                new_right: Some(EarDetectionStatus::OutOfEar),
            },
        ));
        let s = airpods(&app, MAC);
        assert_eq!(s.ear_left, Some(EarDetectionStatus::InEar));
        assert_eq!(s.ear_right, Some(EarDetectionStatus::OutOfEar));
    }

    #[test]
    fn eq_data_event_stores_bands() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(MAC, AE::EqData([1, 2, 3, 4, 5, 6, 7, 8])));
        assert_eq!(airpods(&app, MAC).eq_bands, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    }

    fn cc(id: ControlCommandIdentifiers, val: u8) -> AE {
        AE::ControlCommand(ControlCommandStatus {
            identifier: id,
            value: vec![val],
        })
    }

    #[test]
    fn control_command_listening_mode_decoded() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::ListeningMode, 0x03),
        ));
        assert_eq!(
            airpods(&app, MAC).listening_mode,
            AirPodsNoiseControlMode::Transparency
        );
    }

    #[test]
    fn control_command_mic_mode_decrements_to_zero_indexed() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(MAC, cc(ControlCommandIdentifiers::MicMode, 0x03)));
        // wire 0x03 (Auto) → stored 2
        assert_eq!(airpods(&app, MAC).mic_mode, Some(2));
    }

    #[test]
    fn control_command_toggles_set_correct_booleans() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::ConversationDetectConfig, 0x01),
        ));
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::OneBudAncMode, 0x01),
        ));
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::AdaptiveVolumeConfig, 0x01),
        ));
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::AllowOffOption, 0x01),
        ));
        let s = airpods(&app, MAC);
        assert!(s.conversation_awareness);
        assert!(s.one_bud_anc);
        assert!(s.adaptive_volume);
        assert!(s.allow_off_mode);
    }

    #[test]
    fn settings_items_for_pro2_includes_stem_and_ca() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        let labels: Vec<&str> = app
            .settings_items()
            .iter()
            .map(|i| match i {
                SettingsItem::Toggle { label, .. } => *label,
                SettingsItem::Enum { label, .. } => *label,
                SettingsItem::Slider { label, .. } => *label,
            })
            .collect();
        assert!(labels.contains(&"Conversation Awareness"));
        assert!(labels.contains(&"NC with One AirPod"));
        assert!(labels.contains(&"Press Speed"));
        assert!(labels.contains(&"Volume Swipe Length"));
        assert!(labels.contains(&"Mic Mode"));
    }

    #[test]
    fn settings_items_for_airpods3_no_anc_skips_anc_specific() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", AIRPODS3));
        let labels: Vec<&str> = app
            .settings_items()
            .iter()
            .map(|i| match i {
                SettingsItem::Toggle { label, .. } => *label,
                SettingsItem::Enum { label, .. } => *label,
                SettingsItem::Slider { label, .. } => *label,
            })
            .collect();
        // No ANC → no Conversation Awareness, no One-Bud ANC
        assert!(!labels.contains(&"Conversation Awareness"));
        assert!(!labels.contains(&"NC with One AirPod"));
        // Has stem → Press Speed appears
        assert!(labels.contains(&"Press Speed"));
    }

    #[test]
    fn settings_items_for_max_no_stem_skips_stem_items() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Max", MAX));
        let labels: Vec<&str> = app
            .settings_items()
            .iter()
            .map(|i| match i {
                SettingsItem::Toggle { label, .. } => *label,
                SettingsItem::Enum { label, .. } => *label,
                SettingsItem::Slider { label, .. } => *label,
            })
            .collect();
        assert!(!labels.contains(&"Press Speed"));
        assert!(!labels.contains(&"Volume Swipe"));
        assert!(!labels.contains(&"Volume Swipe Length"));
    }

    #[test]
    fn adaptive_noise_slider_only_when_adaptive_mode_active() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        // Default listening mode is NoiseCancellation → no adaptive slider
        let labels: Vec<&str> = app
            .settings_items()
            .iter()
            .map(|i| match i {
                SettingsItem::Slider { label, .. } => *label,
                _ => "",
            })
            .collect();
        assert!(!labels.contains(&"Adaptive Noise Level"));

        // Switch to Adaptive
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::ListeningMode, 0x04),
        ));
        let labels: Vec<&str> = app
            .settings_items()
            .iter()
            .map(|i| match i {
                SettingsItem::Slider { label, .. } => *label,
                _ => "",
            })
            .collect();
        assert!(labels.contains(&"Adaptive Noise Level"));
    }

    #[test]
    fn noise_control_rows_zero_when_no_anc() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", AIRPODS3));
        assert_eq!(app.noise_control_rows(), 0);
    }

    #[test]
    fn noise_control_rows_grows_with_options() {
        let (mut app, _) = mk_app();
        app.handle_event(connected(MAC, "Pods", PRO2));
        // Default: has_adaptive=true, allow_off=false → 3 rows (Trans, Adaptive, NC)
        assert_eq!(app.noise_control_rows(), 3);
        // Enable allow_off via control command
        app.handle_event(aacp(
            MAC,
            cc(ControlCommandIdentifiers::AllowOffOption, 0x01),
        ));
        assert_eq!(app.noise_control_rows(), 4);
    }

    #[test]
    fn send_command_emits_on_channel() {
        let (app, mut cmd_rx) = mk_app();
        app.send_command(MAC, ControlCommandIdentifiers::ListeningMode, vec![0x02]);
        let received = cmd_rx.try_recv().expect("command emitted");
        assert_eq!(received.0, MAC);
        match received.1 {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ListeningMode);
                assert_eq!(val, vec![0x02]);
            }
            _ => panic!("expected ControlCommand"),
        }
    }

    #[test]
    fn send_rename_emits_rename_command() {
        let (app, mut cmd_rx) = mk_app();
        app.send_rename(MAC, "NewName".into());
        let received = cmd_rx.try_recv().expect("rename emitted");
        assert!(matches!(received.1, DeviceCommand::Rename(ref n) if n == "NewName"));
    }

    #[test]
    fn audio_unavailable_event_sets_flag() {
        let (mut app, _) = mk_app();
        assert!(!app.audio_unavailable);
        app.handle_event(AppEvent::AudioUnavailable);
        assert!(app.audio_unavailable);
    }

    #[test]
    fn aacp_event_for_unknown_mac_creates_default_state() {
        let (mut app, _) = mk_app();
        // Events arrive before DeviceConnected — App should fabricate a state
        app.handle_event(aacp(
            MAC,
            AE::BatteryInfo(vec![BatteryInfo {
                component: BatteryComponent::Left,
                level: 50,
                status: BatteryStatus::NotCharging,
            }]),
        ));
        assert_eq!(app.device_order, vec![MAC]);
        let s = airpods(&app, MAC);
        assert_eq!(s.name, "AirPods");
        assert_eq!(s.battery_left, Some((50, BatteryStatus::NotCharging)));
    }
}
