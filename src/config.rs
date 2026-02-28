use log::info;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Command to show volume OSD. `{}` is replaced with the signed delta (e.g. "+5" or "-3").
    pub volume_osd_command: Vec<String>,
    /// Command to set absolute volume. `{}` is replaced with a 0.0–1.0 fraction.
    pub volume_set_command: Vec<String>,
    /// Optional command to restart the audio server (e.g. WirePlumber).
    /// Set to `None` (the default) to disable the automatic restart.
    pub restart_audio_server: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            volume_osd_command: vec![
                "swayosd-client".into(),
                "--output-volume".into(),
                "{}".into(),
            ],
            volume_set_command: vec![
                "wpctl".into(),
                "set-volume".into(),
                "@DEFAULT_AUDIO_SINK@".into(),
                "{}".into(),
            ],
            restart_audio_server: None,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<Config>(&contents) {
                Ok(cfg) => {
                    info!("Loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    log::warn!("Failed to parse {}: {}, using defaults", path.display(), e);
                    Config::default()
                }
            },
            Err(_) => {
                info!("No config file at {}, using defaults", path.display());
                Config::default()
            }
        }
    }
}

fn config_path() -> PathBuf {
    dirs_path().join("config.toml")
}

fn dirs_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("airpods-tui")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("airpods-tui")
    } else {
        PathBuf::from(".config").join("airpods-tui")
    }
}

/// Run a template command, replacing `{}` in each argument with `value`.
pub fn run_template_cmd(template: &[String], value: &str) {
    if template.is_empty() {
        return;
    }
    let args: Vec<String> = template
        .iter()
        .map(|arg| arg.replace("{}", value))
        .collect();
    let _ = std::process::Command::new(&args[0])
        .args(&args[1..])
        .output();
}
