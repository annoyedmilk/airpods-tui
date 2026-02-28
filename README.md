# airpods-tui

A terminal UI for managing AirPods on Linux, built for [Omarchy](https://omarchy.org/). Controls noise mode, conversation awareness, stem settings, and more over Bluetooth AACP.

![airpods-tui](airpods-tui.png)

## Features

- Battery status (Left, Right, Case)
- Noise control (Transparency, Adaptive, Noise Cancellation)
- Settings: Conversation Awareness, One-Bud ANC, Personalized Volume, Volume Swipe, Press Speed, Tone Volume, and more
- Stem press media controls (play/pause, next/prev track)
- Volume swipe synced to system volume via WirePlumber + SwayOSD
- Waybar JSON output (`--waybar` / `--waybar-watch`)
- Supports 25+ Apple/Beats models with per-model capability detection

## Usage

```
airpods-tui              # launch TUI
airpods-tui --waybar     # print JSON status and exit
airpods-tui --waybar-watch  # persistent JSON output on changes
airpods-tui -d           # enable debug logging (/tmp/airpods-tui.log)
```

## Keys

| Key | Action |
|-----|--------|
| `q` / `Ctrl+C` | Quit |
| `Tab` / `Shift+Tab` | Cycle section |
| `Up` / `Down` | Navigate rows |
| `Left` / `Right` | Adjust slider/enum; switch device tab |
| `Space` / `Enter` | Toggle / select |
| `1-3` | Noise mode shortcut |
| `c` | Toggle Conversation Awareness |

## Dependencies

- BlueZ (D-Bus)
- PipeWire + WirePlumber (`wpctl`)
- SwayOSD (volume overlay)

## Building

```
cargo build --release
```

## License

GPL-3.0-or-later
