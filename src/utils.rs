use std::io;
use std::path::PathBuf;

pub fn runtime_dir() -> io::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is not set; refusing to use a world-writable fallback",
            )
        })
}

/// Write battery levels to `airpods-battery.env` in the runtime directory
/// for external consumers (waybar, scripts).
pub fn write_battery_env(
    left: Option<u8>,
    right: Option<u8>,
    case: Option<u8>,
    headphone: Option<u8>,
) {
    let dir = match runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("Skipping airpods-battery.env: {}", e);
            return;
        }
    };
    let mut content = String::new();
    if let Some(l) = left {
        content.push_str(&format!("LEFT={}\n", l));
    }
    if let Some(r) = right {
        content.push_str(&format!("RIGHT={}\n", r));
    }
    if let Some(c) = case {
        content.push_str(&format!("CASE={}\n", c));
    }
    if let Some(h) = headphone {
        content.push_str(&format!("HEADPHONE={}\n", h));
    }
    if let Err(e) = std::fs::write(dir.join("airpods-battery.env"), content) {
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
