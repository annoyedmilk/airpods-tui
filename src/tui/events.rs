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
        KeyCode::Tab => {
            if has_settings(app) {
                app.focused_section = app.focused_section.next();
                app.section_row = 0;
            }
        }
        KeyCode::BackTab => {
            if has_settings(app) {
                app.focused_section = app.focused_section.prev();
                app.section_row = 0;
            }
        }

        // Up/Down: navigate within current section
        KeyCode::Up => {
            if app.section_row > 0 {
                app.section_row -= 1;
            }
        }
        KeyCode::Down => {
            let max = section_max_row(app);
            if app.section_row < max {
                app.section_row += 1;
            }
        }

        // Left/Right: adjust sliders/enums in Settings, switch device tab otherwise
        KeyCode::Left => {
            if app.focused_section == FocusedSection::Settings {
                if let Some(item) = current_settings_item(app) {
                    match item {
                        SettingsItem::Slider { value, min, cmd, .. } => {
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
            }
            if app.selected_device_idx > 0 {
                app.selected_device_idx -= 1;
                app.section_row = 0;
                app.focused_section = FocusedSection::NoiseControl;
            }
        }
        KeyCode::Right => {
            if app.focused_section == FocusedSection::Settings {
                if let Some(item) = current_settings_item(app) {
                    match item {
                        SettingsItem::Slider { value, max, cmd, .. } => {
                            let new_val = (value + 5).min(max);
                            send_setting(app, cmd, new_val);
                            return;
                        }
                        SettingsItem::Enum { value, options, cmd, .. } => {
                            let max_idx = (options.len() as u8).saturating_sub(1);
                            if value < max_idx {
                                send_setting(app, cmd, value + 1);
                            }
                            return;
                        }
                        _ => {}
                    }
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
    let Some(ref mut buf) = app.rename_mode else { return };
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
        KeyCode::Char(c) => {
            if buf.len() < 32 {
                buf.push(c);
            }
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
    let Some(mac) = app.selected_mac().cloned() else { return };
    // Update local state
    if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
        match cmd {
            ControlCommandIdentifiers::DoubleClickInterval => state.press_speed = Some(value),
            ControlCommandIdentifiers::ClickHoldInterval => state.press_hold_duration = Some(value),
            ControlCommandIdentifiers::ChimeVolume => state.tone_volume = Some(value),
            ControlCommandIdentifiers::VolumeSwipeInterval => state.volume_swipe_length = Some(value),
            ControlCommandIdentifiers::AutoAncStrength => state.adaptive_noise_level = Some(value),
            _ => {}
        }
    }
    app.send_command(&mac, cmd, vec![value]);
}

fn set_noise_mode(app: &mut App, mode: AirPodsNoiseControlMode) {
    let Some(mac) = app.selected_mac().cloned() else { return };
    if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
        state.listening_mode = mode.clone();
    }
    app.send_command(&mac, ControlCommandIdentifiers::ListeningMode, vec![mode.to_byte()]);
}

fn toggle_conversation_awareness(app: &mut App) {
    let Some(mac) = app.selected_mac().cloned() else { return };
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
    let Some(mac) = app.selected_mac().cloned() else { return };
    let (has_anc, has_adaptive) = match app.devices.get(&mac) {
        Some(DeviceState::AirPods(s)) => (s.has_anc, s.has_adaptive),
        _ => return,
    };
    if !has_anc { return; }

    let row = app.section_row;
    let mode = match (has_adaptive, row) {
        (_, 0)        => AirPodsNoiseControlMode::Transparency,
        (true, 1)     => AirPodsNoiseControlMode::Adaptive,
        (true, 2) | _ => AirPodsNoiseControlMode::NoiseCancellation,
    };
    set_noise_mode(app, mode);
}

fn activate_settings_row(app: &mut App) {
    let Some(item) = current_settings_item(app) else { return };
    let Some(mac) = app.selected_mac().cloned() else { return };

    match item {
        SettingsItem::Toggle { value, cmd, .. } => {
            let new_val = !value;
            // Update local state
            if let Some(DeviceState::AirPods(state)) = app.devices.get_mut(&mac) {
                match cmd {
                    ControlCommandIdentifiers::ConversationDetectConfig => state.conversation_awareness = new_val,
                    ControlCommandIdentifiers::OneBudAncMode => state.one_bud_anc = new_val,
                    ControlCommandIdentifiers::AdaptiveVolumeConfig => state.adaptive_volume = new_val,
                    ControlCommandIdentifiers::VolumeSwipeMode => state.volume_swipe = new_val,
                    ControlCommandIdentifiers::AllowAutoConnect => state.auto_connect = Some(new_val),
                    _ => {}
                }
            }
            // All AACP toggle commands use 0x01 = enabled, 0x02 = disabled
            let byte: u8 = if new_val { 0x01 } else { 0x02 };
            app.send_command(&mac, cmd, vec![byte]);
        }
        SettingsItem::Enum { value, options, cmd, .. } => {
            let next = if (value as usize + 1) < options.len() { value + 1 } else { 0 };
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
