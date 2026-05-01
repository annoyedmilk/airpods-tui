use crate::bluetooth::aacp::ControlCommandIdentifiers;
use crate::devices::enums::AirPodsNoiseControlMode;
use crate::tui::app::{App, DeviceState, FocusedSection, SettingsItem};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Rename mode intercepts all keys
    if app.rename_mode.is_some() {
        handle_rename_key(app, key);
        return;
    }

    match key.code {
        // Quit
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        // Tab / Shift+Tab: cycle focused section
        KeyCode::Tab if has_settings(app) => {
            app.focused_section = app.focused_section.next();
            app.section_row = 0;
        }
        KeyCode::BackTab if has_settings(app) => {
            app.focused_section = app.focused_section.prev();
            app.section_row = 0;
        }

        // Up/Down: navigate within current section
        KeyCode::Up if app.section_row > 0 => {
            app.section_row -= 1;
        }
        KeyCode::Down => {
            let max = section_max_row(app);
            if app.section_row < max {
                app.section_row += 1;
            }
        }

        // Left/Right: adjust sliders/enums in Settings, switch device tab otherwise
        KeyCode::Left => {
            if app.focused_section == FocusedSection::Settings
                && let Some(item) = current_settings_item(app)
            {
                match item {
                    SettingsItem::Slider {
                        value, min, cmd, ..
                    } => {
                        let new_val = value.saturating_sub(5).max(min);
                        send_setting(app, cmd, new_val);
                        return;
                    }
                    SettingsItem::Enum { value, cmd, .. } => {
                        if value > 0 {
                            send_setting(app, cmd, value - 1);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            if app.selected_device_idx > 0 {
                app.selected_device_idx -= 1;
                app.section_row = 0;
                app.focused_section = FocusedSection::NoiseControl;
            }
        }
        KeyCode::Right => {
            if app.focused_section == FocusedSection::Settings
                && let Some(item) = current_settings_item(app)
            {
                match item {
                    SettingsItem::Slider {
                        value, max, cmd, ..
                    } => {
                        let new_val = (value + 5).min(max);
                        send_setting(app, cmd, new_val);
                        return;
                    }
                    SettingsItem::Enum {
                        value,
                        options,
                        cmd,
                        ..
                    } => {
                        let max_idx = (options.len() as u8).saturating_sub(1);
                        if value < max_idx {
                            send_setting(app, cmd, value + 1);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            if app.selected_device_idx + 1 < app.device_order.len() {
                app.selected_device_idx += 1;
                app.section_row = 0;
                app.focused_section = FocusedSection::NoiseControl;
            }
        }

        // Direct noise mode shortcuts
        KeyCode::Char('1') => set_noise_mode(app, AirPodsNoiseControlMode::Transparency),
        KeyCode::Char('2') => {
            let has_adaptive = matches!(
                app.selected_device(),
                Some(DeviceState::AirPods(s)) if s.has_adaptive
            );
            if has_adaptive {
                set_noise_mode(app, AirPodsNoiseControlMode::Adaptive);
            } else {
                set_noise_mode(app, AirPodsNoiseControlMode::NoiseCancellation);
            }
        }
        KeyCode::Char('3') => {
            if matches!(app.selected_device(), Some(DeviceState::AirPods(s)) if s.has_adaptive) {
                set_noise_mode(app, AirPodsNoiseControlMode::NoiseCancellation);
            }
        }

        // Toggle conversation awareness directly
        KeyCode::Char('c') => toggle_conversation_awareness(app),

        // Space/Enter — activate the focused row
        KeyCode::Char(' ') | KeyCode::Enter => activate_row(app),

        // Device info popup
        KeyCode::Char('i') => app.show_info = !app.show_info,

        // Enter rename mode
        KeyCode::Char('r') => {
            if let Some(DeviceState::AirPods(s)) = app.selected_device() {
                app.rename_mode = Some(s.name.clone());
            }
        }

        _ => {}
    }
}

fn handle_rename_key(app: &mut App, key: KeyEvent) {
    let Some(ref mut buf) = app.rename_mode else {
        return;
    };
    match key.code {
        KeyCode::Enter => {
            let new_name = buf.clone();
            if let Some(mac) = app.selected_mac().cloned() {
                if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(&mac) {
                    s.name = new_name.clone();
                }
                app.send_rename(&mac, new_name);
            }
            app.rename_mode = None;
        }
        KeyCode::Esc => {
            app.rename_mode = None;
        }
        KeyCode::Backspace => {
            buf.pop();
        }
        KeyCode::Char(c) if buf.len() < 32 => {
            buf.push(c);
        }
        _ => {}
    }
}

fn has_settings(app: &App) -> bool {
    matches!(app.selected_device(), Some(DeviceState::AirPods(s)) if s.has_anc)
}

fn section_max_row(app: &App) -> usize {
    match app.focused_section {
        FocusedSection::NoiseControl => app.noise_control_rows().saturating_sub(1),
        FocusedSection::Settings => {
            let items = app.settings_items();
            items.len().saturating_sub(1)
        }
    }
}

fn current_settings_item(app: &App) -> Option<SettingsItem> {
    let items = app.settings_items();
    items.into_iter().nth(app.section_row)
}

fn send_setting(app: &mut App, cmd: ControlCommandIdentifiers, value: u8) {
    let Some(mac) = app.selected_mac().cloned() else {
        return;
    };
    // Update local state
    if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
        match cmd {
            ControlCommandIdentifiers::DoubleClickInterval => state.press_speed = Some(value),
            ControlCommandIdentifiers::ClickHoldInterval => state.press_hold_duration = Some(value),
            ControlCommandIdentifiers::ChimeVolume => state.tone_volume = Some(value),
            ControlCommandIdentifiers::VolumeSwipeInterval => {
                state.volume_swipe_length = Some(value)
            }
            ControlCommandIdentifiers::AutoAncStrength => state.adaptive_noise_level = Some(value),
            ControlCommandIdentifiers::MicMode => state.mic_mode = Some(value),
            _ => {}
        }
    }
    // MicMode uses 1-indexed AACP values (0x01=Left, 0x02=Right, 0x03=Auto)
    let wire_value = if cmd == ControlCommandIdentifiers::MicMode {
        value + 1
    } else {
        value
    };
    app.send_command(&mac, cmd, vec![wire_value]);
}

fn set_noise_mode(app: &mut App, mode: AirPodsNoiseControlMode) {
    let Some(mac) = app.selected_mac().cloned() else {
        return;
    };
    if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
        state.listening_mode = mode.clone();
    }
    app.send_command(
        &mac,
        ControlCommandIdentifiers::ListeningMode,
        vec![mode.to_byte()],
    );
}

fn toggle_conversation_awareness(app: &mut App) {
    let Some(mac) = app.selected_mac().cloned() else {
        return;
    };
    let new_val = match app.devices.get(&mac) {
        Some(DeviceState::AirPods(s)) => !s.conversation_awareness,
        _ => return,
    };
    if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(&mac) {
        s.conversation_awareness = new_val;
    }
    app.send_command(
        &mac,
        ControlCommandIdentifiers::ConversationDetectConfig,
        vec![if new_val { 0x01 } else { 0x02 }],
    );
}

fn activate_row(app: &mut App) {
    match app.focused_section {
        FocusedSection::NoiseControl => activate_noise_row(app),
        FocusedSection::Settings => activate_settings_row(app),
    }
}

fn activate_noise_row(app: &mut App) {
    let Some(mac) = app.selected_mac().cloned() else {
        return;
    };
    let (has_anc, has_adaptive, allow_off) = match app.devices.get(&mac) {
        Some(DeviceState::AirPods(s)) => (s.has_anc, s.has_adaptive, s.allow_off_mode),
        _ => return,
    };
    if !has_anc {
        return;
    }

    let modes = crate::tui::ui::noise_mode_list(has_adaptive, allow_off);
    if let Some(mode) = modes.into_iter().nth(app.section_row) {
        set_noise_mode(app, mode);
    }
}

fn activate_settings_row(app: &mut App) {
    let Some(item) = current_settings_item(app) else {
        return;
    };
    let Some(mac) = app.selected_mac().cloned() else {
        return;
    };

    match item {
        SettingsItem::Toggle { value, cmd, .. } => {
            let new_val = !value;
            // Update local state
            if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
                match cmd {
                    ControlCommandIdentifiers::ConversationDetectConfig => {
                        state.conversation_awareness = new_val
                    }
                    ControlCommandIdentifiers::OneBudAncMode => state.one_bud_anc = new_val,
                    ControlCommandIdentifiers::AdaptiveVolumeConfig => {
                        state.adaptive_volume = new_val
                    }
                    ControlCommandIdentifiers::VolumeSwipeMode => state.volume_swipe = new_val,
                    ControlCommandIdentifiers::AllowAutoConnect => {
                        state.auto_connect = Some(new_val)
                    }
                    _ => {}
                }
            }
            // All AACP toggle commands use 0x01 = enabled, 0x02 = disabled
            let byte: u8 = if new_val { 0x01 } else { 0x02 };
            app.send_command(&mac, cmd, vec![byte]);
        }
        SettingsItem::Enum {
            value,
            options,
            cmd,
            ..
        } => {
            let next = if (value as usize + 1) < options.len() {
                value + 1
            } else {
                0
            };
            send_setting(app, cmd, next);
        }
        SettingsItem::Slider { .. } => {
            // Sliders are adjusted with Left/Right, not Space/Enter
        }
    }
}

pub fn handle_event(app: &mut App, event: Event) {
    if let Event::Key(key) = event {
        handle_key(app, key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{AppEvent, DeviceCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc::{self, UnboundedReceiver};

    const MAC_A: &str = "AA:BB:CC:DD:EE:FF";
    const MAC_B: &str = "11:22:33:44:55:66";
    const PRO2: u16 = 0x2014;
    const AIRPODS3: u16 = 0x2013; // no ANC

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn mk_app(product_id: u16) -> (App, UnboundedReceiver<(String, DeviceCommand)>) {
        let (_etx, erx) = mpsc::unbounded_channel::<AppEvent>();
        let (ctx, crx) = mpsc::unbounded_channel();
        let mut app = App::new(erx, ctx);
        app.handle_event(AppEvent::DeviceConnected {
            mac: MAC_A.into(),
            name: "Pods".into(),
            product_id,
        });
        (app, crx)
    }

    #[test]
    fn q_quits() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn lone_c_toggles_conversation_awareness_and_sends_byte_01() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('c')));
        let s = match app.devices.get(MAC_A) {
            Some(DeviceState::AirPods(s)) => s,
            _ => panic!(),
        };
        assert!(s.conversation_awareness);
        let (mac, cmd) = cmd_rx.try_recv().expect("command sent");
        assert_eq!(mac, MAC_A);
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ConversationDetectConfig);
                assert_eq!(val, vec![0x01]); // enable
            }
            _ => panic!(),
        }
    }

    #[test]
    fn tab_cycles_section_when_anc_capable() {
        let (mut app, _) = mk_app(PRO2);
        assert_eq!(app.focused_section, FocusedSection::NoiseControl);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_section, FocusedSection::Settings);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_section, FocusedSection::NoiseControl);
    }

    #[test]
    fn tab_noop_without_anc() {
        let (mut app, _) = mk_app(AIRPODS3);
        let before = app.focused_section;
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.focused_section, before);
    }

    #[test]
    fn down_clamps_to_max_row() {
        let (mut app, _) = mk_app(PRO2);
        // NoiseControl rows = 3 → max idx 2
        for _ in 0..10 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        assert_eq!(app.section_row, 2);
    }

    #[test]
    fn up_clamps_to_zero() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.section_row, 0);
    }

    #[test]
    fn left_right_switches_devices_in_noise_control() {
        let (mut app, _) = mk_app(PRO2);
        app.handle_event(AppEvent::DeviceConnected {
            mac: MAC_B.into(),
            name: "Pods 2".into(),
            product_id: PRO2,
        });
        assert_eq!(app.selected_device_idx, 0);
        handle_key(&mut app, key(KeyCode::Right));
        assert_eq!(app.selected_device_idx, 1);
        handle_key(&mut app, key(KeyCode::Left));
        assert_eq!(app.selected_device_idx, 0);
        // Left at index 0 stays at 0
        handle_key(&mut app, key(KeyCode::Left));
        assert_eq!(app.selected_device_idx, 0);
    }

    #[test]
    fn key_1_sets_transparency() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('1')));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ListeningMode);
                assert_eq!(val, vec![AirPodsNoiseControlMode::Transparency.to_byte()]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn key_2_picks_adaptive_when_supported() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('2')));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(_, val) => {
                assert_eq!(val, vec![AirPodsNoiseControlMode::Adaptive.to_byte()]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn key_2_falls_back_to_nc_when_no_adaptive() {
        let (mut app, mut cmd_rx) = mk_app(0x200e); // AirPods Pro (no adaptive)
        handle_key(&mut app, key(KeyCode::Char('2')));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(_, val) => {
                assert_eq!(
                    val,
                    vec![AirPodsNoiseControlMode::NoiseCancellation.to_byte()]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn key_3_sets_nc_only_when_adaptive_capable() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('3')));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(_, val) => {
                assert_eq!(
                    val,
                    vec![AirPodsNoiseControlMode::NoiseCancellation.to_byte()]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn key_3_does_nothing_when_no_adaptive() {
        let (mut app, mut cmd_rx) = mk_app(0x200e);
        handle_key(&mut app, key(KeyCode::Char('3')));
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn enter_in_noise_control_sends_listening_mode() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        // Default rows: [Transparency, Adaptive, NoiseCancellation]; row 0 = Transparency
        handle_key(&mut app, key(KeyCode::Enter));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ListeningMode);
                assert_eq!(val, vec![AirPodsNoiseControlMode::Transparency.to_byte()]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn space_in_settings_toggles_active_row_to_enabled() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        // Switch to Settings
        handle_key(&mut app, key(KeyCode::Tab));
        // First row for PRO2 is "Conversation Awareness" — toggle on
        handle_key(&mut app, key(KeyCode::Char(' ')));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ConversationDetectConfig);
                assert_eq!(val, vec![0x01]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn left_in_settings_decrements_slider_within_range() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        // Set tone_volume so we can decrement deterministically
        if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(MAC_A) {
            s.tone_volume = Some(50);
        }
        handle_key(&mut app, key(KeyCode::Tab)); // → Settings
        // Walk down to "Tone Volume" — for PRO2, ordered: CA, OneBudANC, Personalized Volume,
        // Volume Swipe, Press Speed, Press & Hold, Tone Volume
        for _ in 0..6 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Left));
        let (_, cmd) = cmd_rx.try_recv().expect("slider command");
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::ChimeVolume);
                assert_eq!(val, vec![45]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn slider_clamps_to_min() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(MAC_A) {
            s.tone_volume = Some(15); // = min
        }
        handle_key(&mut app, key(KeyCode::Tab));
        for _ in 0..6 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Left));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(_, val) => assert_eq!(val, vec![15]),
            _ => panic!(),
        }
    }

    #[test]
    fn slider_clamps_to_max() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(MAC_A) {
            s.tone_volume = Some(98);
        }
        handle_key(&mut app, key(KeyCode::Tab));
        for _ in 0..6 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Right));
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        match cmd {
            DeviceCommand::ControlCommand(_, val) => assert_eq!(val, vec![100]),
            _ => panic!(),
        }
    }

    #[test]
    fn r_enters_rename_mode_with_current_name() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('r')));
        assert_eq!(app.rename_mode.as_deref(), Some("Pods"));
    }

    #[test]
    fn rename_mode_buffers_chars_and_commits_on_enter() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('r')));
        // Replace existing name: backspace through "Pods" then type new
        for _ in 0..4 {
            handle_key(&mut app, key(KeyCode::Backspace));
        }
        for c in "New".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.rename_mode.is_none());
        let s = match app.devices.get(MAC_A) {
            Some(DeviceState::AirPods(s)) => s,
            _ => panic!(),
        };
        assert_eq!(s.name, "New");
        let (_, cmd) = cmd_rx.try_recv().unwrap();
        assert!(matches!(cmd, DeviceCommand::Rename(ref n) if n == "New"));
    }

    #[test]
    fn rename_mode_esc_discards() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('r')));
        for c in "X".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.rename_mode.is_none());
        // Name remains the original
        let s = match app.devices.get(MAC_A) {
            Some(DeviceState::AirPods(s)) => s,
            _ => panic!(),
        };
        assert_eq!(s.name, "Pods");
    }

    #[test]
    fn rename_mode_caps_at_32_chars() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('r')));
        for _ in 0..4 {
            handle_key(&mut app, key(KeyCode::Backspace));
        }
        for _ in 0..40 {
            handle_key(&mut app, key(KeyCode::Char('a')));
        }
        assert_eq!(app.rename_mode.as_deref().unwrap().len(), 32);
    }

    #[test]
    fn mic_mode_setting_writes_one_indexed_wire_value() {
        let (mut app, mut cmd_rx) = mk_app(PRO2);
        // Force MicMode value, then dispatch through enum Right/Left from Settings
        if let Some(DeviceState::AirPods(s)) = app.devices.get_mut(MAC_A) {
            s.mic_mode = Some(0); // Always Left (UI 0-indexed)
        }
        // Walk to Mic Mode row — for PRO2 with default state the order is:
        // CA, OneBudANC, Personalized Vol, Volume Swipe, Press Speed, Press & Hold,
        // Tone Volume, Volume Swipe Length, Mic Mode
        handle_key(&mut app, key(KeyCode::Tab));
        for _ in 0..8 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        handle_key(&mut app, key(KeyCode::Right));
        let (_, cmd) = cmd_rx.try_recv().expect("mic mode command");
        match cmd {
            DeviceCommand::ControlCommand(id, val) => {
                assert_eq!(id, ControlCommandIdentifiers::MicMode);
                // UI new value = 1 → wire = 1 + 1 = 2
                assert_eq!(val, vec![2]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn i_toggles_info_overlay() {
        let (mut app, _) = mk_app(PRO2);
        assert!(!app.show_info);
        handle_key(&mut app, key(KeyCode::Char('i')));
        assert!(app.show_info);
        handle_key(&mut app, key(KeyCode::Char('i')));
        assert!(!app.show_info);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let (mut app, _) = mk_app(PRO2);
        handle_key(&mut app, key(KeyCode::Char('z')));
        handle_key(&mut app, key(KeyCode::Char('9')));
        handle_key(&mut app, key(KeyCode::F(5)));
        assert!(!app.should_quit);
    }
}
