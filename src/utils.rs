use std::path::PathBuf;

pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir)
    } else {
        log::warn!(
            "XDG_RUNTIME_DIR not set, falling back to /tmp for runtime files. \
             This is less secure as /tmp is world-readable."
        );
        PathBuf::from("/tmp")
    }
}

/// Write battery levels to `airpods-battery.env` in the runtime directory
/// for external consumers (waybar, scripts).
pub fn write_battery_env(left: Option<u8>, right: Option<u8>, case: Option<u8>) {
    let mut content = String::new();
    if let Some(l) = left { content.push_str(&format!("LEFT={}\n", l)); }
    if let Some(r) = right { content.push_str(&format!("RIGHT={}\n", r)); }
    if let Some(c) = case { content.push_str(&format!("CASE={}\n", c)); }
    if let Err(e) = std::fs::write(runtime_dir().join("airpods-battery.env"), content) {
        log::warn!("Failed to write airpods-battery.env: {}", e);
    }
}

pub fn get_devices_path() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(data_dir)
        .join("airpods-tui")
        .join("devices.json")
}
