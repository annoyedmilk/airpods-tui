# airpods-tui

A terminal UI for managing AirPods on Linux, built for [Omarchy](https://omarchy.org/). Speaks Apple's AACP control channel over Bluetooth to expose battery, noise mode, conversation awareness, stem controls, and the rest of the iOS settings panel from a keyboard-driven TUI.

![airpods-tui](airpods-tui.png)

## Features

- **Battery** per pod, case, and headphone (Max), with color indicators and low-battery desktop notifications at 20% and 10% (via the daemon). The case reports its level whenever a pod sits inside; the last known value is retained while you wear both pods
- **Noise control**: Off, Transparency, Adaptive, Noise Cancellation (model-aware, Adaptive only shown on capable devices)
- **Settings panel**, dynamically built per model:
  - Conversation Awareness (Pro 2, Pro 3, Pro USB-C, 4 ANC, Max 2)
  - Adaptive Noise Level slider (adaptive-capable models)
  - NC with One AirPod (any ANC-capable model)
  - Volume Swipe + Volume Swipe Length, Press Speed, Press & Hold (stem-equipped models)
  - Press-and-hold action per bud: Noise Control or Siri
  - Hold Cycle membership (which of Off / NC / Transparency / Adaptive the press-and-hold cycles through)
  - Crown Direction (AirPods Max)
  - Personalized Volume, Tone Volume, In-Case Tone + In-Case Tone Volume
  - Mic Mode (Automatic / Always Right / Always Left)
  - Siri Voice Trigger
  - Auto Ear Detection, Sleep Detection, Auto Connect
- **Ear detection** status in the header
- **Stem press media controls** (play/pause, next/prev) wired through MPRIS
- **Device renaming**: sets both the AACP name and the BlueZ alias
- **Volume swipe synced** to system volume via configurable commands; your Volume Swipe on/off choice is remembered per device and re-applied on connect
- **Auto audio rerouting** to the AirPods sink when playback starts or the buds go in your ears
- **Automatic iPhone ↔ Linux handoff**: pauses local media when an Apple device takes audio ownership and reclaims the audio session once the peer stops playing (playback stays paused until you press play)
- **Waybar integration** via JSON output (`--waybar` / `--waybar-watch`)
- **Background daemon** with Unix-socket IPC so the TUI launches instantly
- **28 Apple/Beats models** with per-model capability detection; unknown Apple devices fall back to safe defaults

## Installation

### Arch / Omarchy (AUR)

```bash
yay -S airpods-tui-bin    # prebuilt x86_64 binary, fastest
# or
yay -S airpods-tui-git    # builds from latest main
```

Both packages run an install hook that:

- installs the binary to `/usr/bin/airpods-tui`
- drops the systemd user unit at `/usr/lib/systemd/user/airpods-tui.service`
- adds `DeviceID = bluetooth:004C:0000:0000` under `[General]` in `/etc/bluetooth/main.conf` (removed on uninstall)

The DeviceID makes BlueZ identify itself as an Apple host. Without it, AirPods still pair and play audio (A2DP works fine), but they refuse to open the AACP control channel, which is what every feature in this tool runs over. So plain music playback works without it, but battery, noise mode, settings, ear detection, etc. all stay blank.

### From source

```bash
git clone https://github.com/annoyedmilk/airpods-tui.git
cd airpods-tui
cargo build --release
sudo install -Dm755 target/release/airpods-tui /usr/bin/airpods-tui
sudo install -Dm644 airpods-tui.service /usr/lib/systemd/user/airpods-tui.service
```

This path does **not** run the install hook, see [Apple DeviceID setup](#apple-deviceid-setup) below.

### Apple DeviceID setup

Required for AACP. Skip if you installed via the AUR, the package hook already did it.

```bash
sudo sed -i '/^\[General\]/a DeviceID = bluetooth:004C:0000:0000' /etc/bluetooth/main.conf
sudo systemctl restart bluetooth
```

If your AirPods were paired *before* adding the DeviceID, forget and re-pair them so they handshake against an Apple-identified host:

```bash
bluetoothctl remove <AIRPODS_MAC>
```

Open the AirPods case, hold the button on the back until the LED flashes white, then re-pair via Bluetooth settings or `bluetoothctl`.

### Enable the daemon

```bash
systemctl --user daemon-reload
systemctl --user enable --now airpods-tui.service
```

The daemon owns the AACP session so the TUI launches instantly via the IPC socket. Logs: `journalctl --user -u airpods-tui`.

### Floating window (Hyprland / Omarchy, optional)

Omarchy launches its own TUIs (bluetui, impala, btop) as centered floating
windows. To get the same for airpods-tui, add to `~/.config/hypr/hyprland.conf`:

```ini
windowrule = float on, match:class org.omarchy.airpods-tui
windowrule = center on, match:class org.omarchy.airpods-tui
windowrule = size 615 486, match:class org.omarchy.airpods-tui
```

The size fits the TUI (82×28 cells) at Omarchy's default terminal font
(JetBrainsMono 9pt, 14px padding); adjust if you use a different font or size.
Don't use Omarchy's `tag +floating-window` shortcut here, because its generic
`size 875 600` rule is applied in a later pass and overrides any
class-matched size.

Launch or focus the window with the stock Omarchy helper:

```bash
omarchy-launch-or-focus-tui airpods-tui
```

### Waybar module (optional)

Add to `~/.config/waybar/config.jsonc` modules list:

```jsonc
"custom/airpods": {
    "exec": "airpods-tui --waybar-watch",
    "return-type": "json",
    "format": "󰎈 {}",
    "on-click": "omarchy-launch-or-focus-tui airpods-tui"
}
```

Add `"custom/airpods"` to your bar's `modules-right` (or wherever you prefer) and restart Waybar:

```bash
omarchy restart waybar
```

For scripts that don't want to parse JSON, every battery update is also written to `$XDG_RUNTIME_DIR/airpods-battery.env` as `LEFT=`/`RIGHT=`/`CASE=`/`HEADPHONE=` lines.

## Usage

```
airpods-tui                 # launch TUI
airpods-tui --daemon        # headless background daemon (no TUI)
airpods-tui --waybar        # print one-shot JSON status and exit
airpods-tui --waybar-watch  # persistent JSON output on every change
airpods-tui -d              # debug logging (visible in journalctl)
airpods-tui -v              # show version and exit
```

## Keys

| Key | Action |
|-----|--------|
| `q` / `Ctrl+C` | Quit |
| `Tab` / `Shift+Tab` | Cycle section (Noise Control / Settings) |
| `↑` / `↓` | Navigate rows in current section |
| `←` / `→` | Adjust slider/enum in Settings; switch device tab in Noise Control |
| `Space` / `Enter` | Toggle / select focused row |
| `1` / `2` / `3` | Noise mode shortcut (Transparency / Adaptive / Noise Cancellation) |
| `c` | Toggle Conversation Awareness |
| `r` | Rename device |
| `i` | Show device info popup (model, firmware, serial) |

## Configuration

Optional config at `~/.config/airpods-tui/config.toml`:

```toml
# Pop the volume OSD after a stem swipe ({} receives "+0": display only,
# the volume itself is applied by volume_set_command)
volume_osd_command = ["swayosd-client", "--output-volume", "{}"]

# Apply absolute volume ({} is replaced with a 0.0 to 1.0 fraction)
volume_set_command = ["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "{}"]

# Battery-low desktop notification at 20% and 10%, sent by the daemon
# ({} is replaced with "Left battery: 18%" etc.)
battery_alert_command = ["notify-send", "AirPods", "{}"]

# Optional: run after the audio sink switches if you hit quality issues
# restart_audio_server = ["systemctl", "--user", "restart", "wireplumber"]
```

Set any command to `[]` to disable that integration. `restart_audio_server` defaults to `None` (disabled).

## Dependencies

Runtime:

- **BlueZ**: D-Bus interface to Bluetooth
- **libpulse**: PulseAudio client lib (also used to control PipeWire's pulse compatibility layer)
- **dbus**

Optional:

- **PipeWire + WirePlumber** (`wpctl`) for volume control
- **SwayOSD** (`swayosd-client`) for the volume overlay
- **libnotify** (`notify-send`) for battery alerts

## License

GPL-3.0-or-later
