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

pub fn get_devices_path() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(data_dir)
        .join("airpods-tui")
        .join("devices.json")
}
