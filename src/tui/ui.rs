use crate::bluetooth::aacp::{BatteryStatus, EarDetectionStatus};
use crate::devices::enums::AirPodsNoiseControlMode;
use crate::tui::app::{AirPodsDeviceState, App, DeviceState, FocusedSection, SettingsItem};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Row, Table, TableState},
};

const ACCENT: Color = Color::Cyan;
const FOCUS_COLOR: Color = Color::Green;
const HEADER: Color = Color::Yellow;
const FG: Color = Color::White;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    if app.device_order.is_empty() {
        let msg = Paragraph::new("No device connected.\n\nWaiting…")
            .style(Style::default().fg(DIM))
            .alignment(Alignment::Center);
        f.render_widget(msg, centered_rect(area, 50, 30));
        draw_footer(f, footer_row(area), app);
        return;
    }

    let col = centered_col(area, 80);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if app.device_order.len() > 1 { 2 } else { 0 }),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(col);

    if app.device_order.len() > 1 {
        draw_tabs(f, chunks[0], app);
    }
    draw_content(f, chunks[1], app);
    draw_footer(f, chunks[2], app);

    // Rename popup overlay
    if let Some(ref buf) = app.rename_mode {
        draw_rename_popup(f, area, buf);
    }

    // Device info popup
    if app.show_info
        && let Some(DeviceState::AirPods(state)) = app.selected_device()
    {
        draw_info_popup(f, area, state);
    }

}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let spans: Vec<Span> = app
        .device_order
        .iter()
        .enumerate()
        .flat_map(|(i, mac)| {
            let name = app
                .devices
                .get(mac)
                .map(|d| d.name().to_string())
                .unwrap_or_else(|| mac.clone());
            let style = if i == app.selected_device_idx {
                Style::default()
                    .fg(ACCENT)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(DIM)
            };
            if i == 0 {
                vec![Span::styled(format!(" {} ", name), style)]
            } else {
                vec![
                    Span::styled("  ", Style::default().fg(DIM)),
                    Span::styled(format!(" {} ", name), style),
                ]
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

fn draw_content(f: &mut Frame, area: Rect, app: &App) {
    let Some(mac) = app.selected_mac() else { return };
    let Some(device) = app.devices.get(mac) else { return };
    match device {
        DeviceState::AirPods(state) => draw_airpods(f, area, state, app),
    }
}

fn draw_airpods(f: &mut Frame, area: Rect, state: &AirPodsDeviceState, app: &App) {
    // Collect battery entries
    let bat_entries: Vec<(&str, u8, BatteryStatus)> = [
        ("Left  ", &state.battery_left),
        ("Right ", &state.battery_right),
        ("Case  ", &state.battery_case),
        ("      ", &state.battery_headphone),
    ]
    .iter()
    .filter_map(|(l, b)| b.as_ref().map(|(lvl, st)| (*l, *lvl, *st)))
    .take(3)
    .collect();

    let bat_count = bat_entries.len().max(1) as u16;
    let display_name = state.model.as_deref().unwrap_or(&state.name);

    // Battery-only view for non-ANC devices
    if !state.has_anc {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),              // name line
                Constraint::Length(bat_count + 2),   // battery box
                Constraint::Fill(1),
            ])
            .split(area);

        f.render_widget(
            Paragraph::new(name_line(display_name, state.ear_left, state.ear_right))
                .alignment(Alignment::Center),
            chunks[0],
        );
        draw_battery_box(f, chunks[1], &bat_entries);
        return;
    }

    // Full ANC view with boxes
    let noise_count = if state.has_adaptive { 3u16 } else { 2 };
    let settings_items = app.settings_items();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                    // name line
            Constraint::Length(bat_count + 2),         // Battery box
            Constraint::Length(noise_count + 2),       // Noise Control box
            Constraint::Fill(1),                      // Settings box
        ])
        .split(area);

    // Name line
    f.render_widget(
        Paragraph::new(name_line(display_name, state.ear_left, state.ear_right))
            .alignment(Alignment::Center),
        chunks[0],
    );

    // Battery box (informational, never focused)
    draw_battery_box(f, chunks[1], &bat_entries);

    // Noise Control box
    let nc_focused = app.focused_section == FocusedSection::NoiseControl;
    let nc_block = section_block("Noise Control", nc_focused);
    let nc_inner = nc_block.inner(chunks[2]);
    f.render_widget(nc_block, chunks[2]);
    draw_noise_options(f, nc_inner, state, app.section_row, nc_focused);

    // Settings box
    let st_focused = app.focused_section == FocusedSection::Settings;
    let st_block = section_block("Settings", st_focused);
    let st_inner = st_block.inner(chunks[3]);
    f.render_widget(st_block, chunks[3]);
    draw_settings_table(f, st_inner, &settings_items, app.section_row, st_focused);

}

fn draw_battery_box(f: &mut Frame, area: Rect, entries: &[(&str, u8, BatteryStatus)]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            " Battery ",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if entries.is_empty() {
        f.render_widget(
            Paragraph::new("  Waiting for data…").style(Style::default().fg(DIM)),
            inner,
        );
        return;
    }

    let constraints: Vec<Constraint> = entries.iter().map(|_| Constraint::Length(1)).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, (label, level, status)) in entries.iter().enumerate() {
        f.render_widget(bat_row(label, *level, status), rows[i]);
    }
}

fn draw_noise_options(
    f: &mut Frame,
    area: Rect,
    state: &AirPodsDeviceState,
    section_row: usize,
    focused: bool,
) {
    let noise_modes = noise_mode_list(state.has_adaptive, state.allow_off_mode);

    let constraints: Vec<Constraint> = noise_modes.iter().map(|_| Constraint::Length(1)).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, mode) in noise_modes.iter().enumerate() {
        let is_focused = focused && section_row == i;
        let active = std::mem::discriminant(mode) == std::mem::discriminant(&state.listening_mode);
        f.render_widget(
            Paragraph::new(noise_row(&mode.to_string(), is_focused, active)),
            rows[i],
        );
    }
}

fn draw_settings_table(
    f: &mut Frame,
    area: Rect,
    items: &[SettingsItem],
    section_row: usize,
    focused: bool,
) {
    if items.is_empty() {
        f.render_widget(
            Paragraph::new("  No settings available").style(Style::default().fg(DIM)),
            area,
        );
        return;
    }

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_selected = focused && section_row == i;
            let cursor = if is_selected {
                Span::styled("▸ ", Style::default().fg(ACCENT))
            } else {
                Span::raw("  ")
            };
            let label_style = if is_selected {
                Style::default().fg(FG)
            } else {
                Style::default().fg(DIM)
            };

            match item {
                SettingsItem::Toggle { label, value, .. } => {
                    let val_str = if *value { "On" } else { "Off" };
                    let val_color = if *value { ACCENT } else { DIM };
                    Row::new(vec![
                        Line::from(vec![cursor, Span::styled(*label, label_style)]),
                        Line::from(Span::styled(
                            val_str,
                            Style::default().fg(val_color).add_modifier(Modifier::BOLD),
                        ))
                        .alignment(Alignment::Right),
                    ])
                }
                SettingsItem::Enum { label, value, options, .. } => {
                    let val_str = options
                        .get(*value as usize)
                        .unwrap_or(&"?");
                    Row::new(vec![
                        Line::from(vec![cursor, Span::styled(*label, label_style)]),
                        Line::from(Span::styled(
                            *val_str,
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        ))
                        .alignment(Alignment::Right),
                    ])
                }
                SettingsItem::Slider { label, value, min, max, .. } => {
                    let range = (*max - *min) as usize;
                    let filled = if range > 0 {
                        ((*value - *min) as usize * 10 / range).min(10)
                    } else {
                        0
                    };
                    let bar = format!("{}{}  {:>3}%", "█".repeat(filled), "░".repeat(10 - filled), value);
                    Row::new(vec![
                        Line::from(vec![cursor, Span::styled(*label, label_style)]),
                        Line::from(Span::styled(
                            bar,
                            Style::default().fg(if is_selected { ACCENT } else { Color::Gray }),
                        ))
                        .alignment(Alignment::Right),
                    ])
                }
            }
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Fill(1), Constraint::Length(20)],
    );

    let mut table_state = TableState::default();
    if focused {
        table_state.select(Some(section_row));
    }
    f.render_stateful_widget(table, area, &mut table_state);
}

fn section_block(title: &str, focused: bool) -> Block<'_> {
    if focused {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(FOCUS_COLOR))
            .title(Span::styled(
                format!(" {} ", title),
                Style::default()
                    .fg(FOCUS_COLOR)
                    .add_modifier(Modifier::BOLD),
            ))
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(DIM))
            .title(Span::styled(
                format!(" {} ", title),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ))
    }
}

fn ear_label(s: EarDetectionStatus) -> &'static str {
    match s {
        EarDetectionStatus::InEar => "in",
        EarDetectionStatus::OutOfEar => "out",
        EarDetectionStatus::InCase => "case",
        EarDetectionStatus::Disconnected => "off",
    }
}

fn name_line(
    display_name: &str,
    ear_left: Option<EarDetectionStatus>,
    ear_right: Option<EarDetectionStatus>,
) -> Line<'_> {
    let mut spans = vec![
        Span::styled(
            format!("  {} ", display_name),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled("● connected", Style::default().fg(Color::Green)),
    ];
    if let (Some(l), Some(r)) = (ear_left, ear_right) {
        spans.push(Span::styled(
            format!("  L:{}  R:{}", ear_label(l), ear_label(r)),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

fn noise_row(label: &str, focused: bool, active: bool) -> Line<'static> {
    let prefix = if focused {
        Span::styled("  ▸ ", Style::default().fg(ACCENT))
    } else {
        Span::raw("    ")
    };
    let text_style = if active {
        Style::default().fg(FG).add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(FG)
    } else {
        Style::default().fg(DIM)
    };
    let mut spans = vec![prefix, Span::styled(label.to_string(), text_style)];
    if active {
        spans.push(Span::styled("  (Active)", Style::default().fg(ACCENT)));
    }
    Line::from(spans)
}

fn bat_row<'a>(label: &'a str, level: u8, status: &BatteryStatus) -> Paragraph<'a> {
    let charging = matches!(status, BatteryStatus::Charging | BatteryStatus::InUse);
    let color = if charging {
        Color::Cyan
    } else if level > 50 {
        Color::Green
    } else if level >= 20 {
        Color::Yellow
    } else {
        Color::Red
    };
    let filled = (level as usize * 10 / 100).min(10);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
    let mut spans = vec![
        Span::styled(format!("  {}", label), Style::default().fg(DIM)),
        Span::styled(format!("{}  ", bar), Style::default().fg(color)),
        Span::styled(format!("{:>3}%", level), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ];
    if charging {
        spans.push(Span::styled("  [charging]", Style::default().fg(Color::Cyan)));
    }
    Paragraph::new(Line::from(spans))
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled("q", Style::default().fg(ACCENT)),
        Span::styled(" quit", Style::default().fg(DIM)),
        Span::styled("  tab", Style::default().fg(ACCENT)),
        Span::styled(" section", Style::default().fg(DIM)),
        Span::styled("  ↑↓", Style::default().fg(ACCENT)),
        Span::styled(" navigation", Style::default().fg(DIM)),
        Span::styled("  space", Style::default().fg(ACCENT)),
        Span::styled(" select", Style::default().fg(DIM)),
        Span::styled("  1-3", Style::default().fg(ACCENT)),
        Span::styled(" noise", Style::default().fg(DIM)),
        Span::styled("  r", Style::default().fg(ACCENT)),
        Span::styled(" rename", Style::default().fg(DIM)),
        Span::styled("  i", Style::default().fg(ACCENT)),
        Span::styled(" info", Style::default().fg(DIM)),
    ];
    if app.audio_unavailable {
        spans.push(Span::styled("  PulseAudio unavailable", Style::default().fg(Color::Red)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).alignment(Alignment::Center), area);
}

fn draw_rename_popup(f: &mut Frame, area: Rect, buf: &str) {
    let popup = centered_rect(area, 60, 30);
    // Clear the area behind the popup
    f.render_widget(ratatui::widgets::Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Rename Device ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(inner);

    // Input line with cursor
    let input_text = format!(" {}▏", buf);
    f.render_widget(
        Paragraph::new(input_text).style(Style::default().fg(FG)),
        chunks[1],
    );

    // Help text
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", Style::default().fg(ACCENT)),
            Span::styled(" confirm  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT)),
            Span::styled(" cancel", Style::default().fg(DIM)),
        ]))
        .alignment(Alignment::Center),
        chunks[3],
    );
}

fn draw_info_popup(f: &mut Frame, area: Rect, state: &AirPodsDeviceState) {
    let fields: Vec<(&str, Option<&str>)> = vec![
        ("Model",     state.model.as_deref()),
        ("Firmware",  state.firmware.as_deref()),
        ("Hardware",  state.hardware_revision.as_deref()),
        ("Serial",    state.serial_number.as_deref()),
        ("L Serial",  state.left_serial.as_deref()),
        ("R Serial",  state.right_serial.as_deref()),
    ];
    let row_count = fields.iter().filter(|(_, v)| v.is_some()).count() as u16;
    let popup_h = row_count + 2; // +2 for border
    let popup_w = 50u16.min(area.width);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(popup_w)) / 2,
        y: area.y + (area.height.saturating_sub(popup_h)) / 2,
        width: popup_w,
        height: popup_h,
    };
    f.render_widget(ratatui::widgets::Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Device Info ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let rows: Vec<Row> = fields
        .iter()
        .filter_map(|(label, val)| {
            val.map(|v| {
                Row::new(vec![
                    Line::from(Span::styled(*label, Style::default().fg(DIM))),
                    Line::from(Span::styled(v.to_owned(), Style::default().fg(FG)))
                        .alignment(Alignment::Right),
                ])
            })
        })
        .collect();

    f.render_widget(
        Table::new(rows, [Constraint::Length(12), Constraint::Fill(1)]),
        inner,
    );
}

/// Ordered list of noise control modes shown in the TUI.
/// Order: Transparency → Adaptive (if available) → Noise Cancellation → Off (if allowed).
/// Must match the row→mode mapping in `events::activate_noise_row`.
pub fn noise_mode_list(has_adaptive: bool, allow_off: bool) -> Vec<AirPodsNoiseControlMode> {
    let mut modes = vec![AirPodsNoiseControlMode::Transparency];
    if has_adaptive {
        modes.push(AirPodsNoiseControlMode::Adaptive);
    }
    modes.push(AirPodsNoiseControlMode::NoiseCancellation);
    if allow_off {
        modes.push(AirPodsNoiseControlMode::Off);
    }
    modes
}

fn centered_col(area: Rect, width: u16) -> Rect {
    let w = width.min(area.width);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y,
        width: w,
        height: area.height,
    }
}

fn footer_row(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    }
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
