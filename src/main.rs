mod bluetooth;
mod devices;
mod media_controller;
mod tui;
mod utils;

use crate::bluetooth::discovery::{find_connected_airpods, find_other_managed_devices};
use crate::bluetooth::le::start_le_monitor;
use crate::bluetooth::managers::DeviceManagers;
use crate::devices::enums::DeviceData;
use crate::tui::app::{App, AppEvent};
use crate::utils::get_devices_path;
use bluer::{Address, InternalErrorKind};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use devices::airpods::AirPodsDevice;
use log::info;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::unbounded_channel;
use dbus::arg::{RefArg, Variant};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::message::MatchRule;

#[derive(Parser)]
#[command(name = "airpods-tui", about = "AirPods TUI controls for Linux")]
struct Args {
    #[arg(long, short = 'd', help = "Enable debug logging")]
    debug: bool,
    #[arg(
        long,
        help = "Enable Bluetooth LE debug logging. Only use when absolutely necessary."
    )]
    le_debug: bool,
    #[arg(long, short = 'v', help = "Show version and exit")]
    version: bool,
    #[arg(long, help = "Print JSON status for waybar and exit")]
    waybar: bool,
    #[arg(long, help = "Print JSON status for waybar on each change (persistent)")]
    waybar_watch: bool,
}

/// Read the BlueZ Modalias property for a device and return its Apple product ID (0 if unknown).
fn read_product_id(addr_str: &str) -> u16 {
    use crate::devices::apple_models::{APPLE_VENDOR_ID, parse_modalias};
    let Ok(conn) = dbus::blocking::Connection::new_system() else { return 0; };
    let path = format!("/org/bluez/hci0/dev_{}", addr_str.replace(':', "_"));
    let proxy = conn.with_proxy("org.bluez", path, Duration::from_millis(500));
    let Ok(modalias) = proxy.get::<String>("org.bluez.Device1", "Modalias") else { return 0; };
    parse_modalias(&modalias)
        .filter(|(v, _)| *v == APPLE_VENDOR_ID)
        .map(|(_, p)| p)
        .unwrap_or(0)
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    if args.version {
        println!("airpods-tui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let log_level = if args.debug { "debug" } else { "warn" };
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var(
                "RUST_LOG",
                format!(
                    "{},airpods_tui::bluetooth::le={}",
                    log_level,
                    if args.le_debug { "debug" } else { "warn" }
                ),
            );
        }
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/airpods-tui.log")
        .expect("Failed to open log file");
    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();

    if args.waybar || args.waybar_watch {
        return run_waybar_mode(args.waybar_watch);
    }

    let (app_tx, app_rx) = unbounded_channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<(String, crate::bluetooth::aacp::ControlCommandIdentifiers, Vec<u8>)>();

    let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let dm_clone = device_managers.clone();
    let app_tx_bt = app_tx.clone();

    // Spawn bluetooth runtime in a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx))
            .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
    });

    // BlueZ MediaTransport1 volume monitor — sync AirPods stem swipe to system volume
    std::thread::spawn(move || {
        let Ok(conn) = dbus::blocking::Connection::new_system() else { return };
        let prev_vol = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(-1));
        let prev_vol_cb = prev_vol.clone();
        let rule = MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged");
        let _ = conn.add_match(rule, move |_: (), _conn, msg| {
            let Some(path) = msg.path() else { return true };
            let path_str = path.to_string();
            if !path_str.contains("/org/bluez/") {
                return true;
            }
            let Ok((iface, changed, _)) =
                msg.read3::<String, HashMap<String, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>>, Vec<String>>()
            else {
                return true;
            };
            if iface != "org.bluez.MediaTransport1" {
                return true;
            }
            if let Some(vol_var) = changed.get("Volume") {
                let vol = vol_var.0.as_u64()
                    .or_else(|| vol_var.0.as_i64().map(|v| v as u64));
                if let Some(vol) = vol {
                    // AVRCP volume is 0-127, map to percentage
                    let new_pct = ((vol as f64) / 127.0 * 100.0).round() as i64;
                    let old_pct = prev_vol_cb.swap(new_pct, std::sync::atomic::Ordering::Relaxed);
                    if old_pct >= 0 {
                        let delta = new_pct - old_pct;
                        if delta != 0 {
                            // swayosd-client expects signed relative change like "+5" or "-5"
                            let arg = if delta > 0 { format!("+{}", delta) } else { format!("{}", delta) };
                            let _ = std::process::Command::new("swayosd-client")
                                .args(["--output-volume", &arg])
                                .output();
                            info!("AVRCP volume {} → swayosd delta {}%", vol, delta);
                        }
                    } else {
                        // First reading — set absolute volume via wpctl, no OSD
                        let frac = (vol as f64) / 127.0;
                        let _ = std::process::Command::new("wpctl")
                            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", frac)])
                            .output();
                        info!("AVRCP volume {} → wpctl initial {:.0}%", vol, frac * 100.0);
                    }
                }
            }
            true
        });
        loop {
            let _ = conn.process(Duration::from_millis(1000));
        }
    });

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(app_rx, cmd_tx);

    // Main TUI loop
    loop {
        app.process_events();

        terminal.draw(|f| tui::ui::draw(f, &app))?;

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            tui::events::handle_event(&mut app, ev);
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn run_waybar_mode(watch: bool) -> io::Result<()> {
    use crate::tui::app::DeviceState;

    let (app_tx, app_rx) = unbounded_channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<(String, crate::bluetooth::aacp::ControlCommandIdentifiers, Vec<u8>)>();

    let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let dm_clone = device_managers.clone();
    let app_tx_bt = app_tx.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx))
            .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
    });

    let mut app = App::new(app_rx, cmd_tx);
    let timeout = if watch { Duration::from_secs(u64::MAX) } else { Duration::from_secs(5) };
    let start = std::time::Instant::now();
    let mut last_json = String::new();

    loop {
        app.process_events();

        let json = match app.selected_device() {
            Some(DeviceState::AirPods(s)) => {
                let model_name = s.model.as_deref().unwrap_or(&s.name);
                let min_bat = [s.battery_left, s.battery_right]
                    .iter()
                    .filter_map(|b| b.as_ref().map(|(l, _)| *l))
                    .min();
                let percentage = min_bat.unwrap_or(0);
                let text = format!("{}%", percentage);
                let mut tooltip_parts = vec![model_name.to_string()];
                if let Some((l, _)) = s.battery_left { tooltip_parts.push(format!("L: {}%", l)); }
                if let Some((r, _)) = s.battery_right { tooltip_parts.push(format!("R: {}%", r)); }
                if let Some((c, _)) = s.battery_case { tooltip_parts.push(format!("C: {}%", c)); }
                let tooltip = tooltip_parts.join("\\n");
                format!(
                    r#"{{"text":"{}","tooltip":"{}","class":"connected","percentage":{}}}"#,
                    text, tooltip, percentage
                )
            }
            Some(DeviceState::Nothing(s)) => {
                format!(
                    r#"{{"text":"{}","tooltip":"{}","class":"connected","percentage":0}}"#,
                    s.name, s.name
                )
            }
            None => {
                r#"{"text":"","tooltip":"No AirPods","class":"disconnected","percentage":0}"#.to_string()
            }
        };

        if json != last_json {
            println!("{}", json);
            last_json = json;
            if !watch {
                // In single-shot mode, exit once we have battery data
                if matches!(app.selected_device(), Some(DeviceState::AirPods(s)) if s.battery_left.is_some() || s.battery_right.is_some()) {
                    break;
                }
            }
        }

        if !watch && start.elapsed() >= timeout {
            break;
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    Ok(())
}

async fn bluetooth_main(
    app_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<(String, crate::bluetooth::aacp::ControlCommandIdentifiers, Vec<u8>)>,
) -> bluer::Result<()> {
    let devices_path = get_devices_path();
    let devices_json = std::fs::read_to_string(&devices_path).unwrap_or_else(|_| "{}".to_string());
    let devices_list: HashMap<String, DeviceData> =
        serde_json::from_str(&devices_json).unwrap_or_default();

    let managed_devices_mac: Vec<String> = devices_list
        .iter()
        .filter(|(_, d)| d.type_ == devices::enums::DeviceType::Nothing)
        .map(|(mac, _)| mac.clone())
        .collect();

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    // LE monitor for BLE battery advertisements
    let le_tx = app_tx.clone();
    tokio::spawn(async move {
        info!("Starting LE monitor...");
        if let Err(e) = start_le_monitor(le_tx).await {
            log::error!("LE monitor error: {}", e);
        }
    });

    // Command dispatcher — receives (mac, command, value) from TUI
    let dm_cmd = device_managers.clone();
    tokio::spawn(async move {
        while let Some((mac, id, value)) = cmd_rx.recv().await {
            let managers = dm_cmd.read().await;
            if let Some(dm) = managers.get(&mac) {
                if let Some(aacp) = dm.get_aacp() {
                    if let Err(e) = aacp.send_control_command(id, &value).await {
                        log::error!("Failed to send control command: {}", e);
                    }
                }
            }
        }
    });

    // Check for already-connected AirPods at startup
    info!("Checking for connected devices...");
    match find_connected_airpods(&adapter).await {
        Ok(device) => {
            let bt_name = device
                .name()
                .await?
                .unwrap_or_else(|| "Unknown AirPods".to_string());
            let addr_str_pre = device.address().to_string();
            let name = devices_list
                .get(&addr_str_pre)
                .filter(|d| !d.name.is_empty())
                .map(|d| d.name.clone())
                .unwrap_or(bt_name);
            info!("Found connected AirPods: {}, initializing.", name);
            let addr_str = addr_str_pre;
            let product_id = read_product_id(&addr_str);
            info!("Product ID for {}: 0x{:04x}", addr_str, product_id);
            let airpods_device = AirPodsDevice::new(device.address(), app_tx.clone(), product_id).await;
            let mut managers = device_managers.write().await;
            let dev_managers = DeviceManagers::with_aacp(airpods_device.aacp_manager.clone());
            managers
                .entry(addr_str.clone())
                .or_insert(dev_managers)
                .set_aacp(airpods_device.aacp_manager);
            drop(managers);
            let _ = app_tx.send(AppEvent::DeviceConnected { mac: addr_str, name, is_nothing: false, product_id });
        }
        Err(_) => {
            info!("No connected AirPods found.");
        }
    }

    // Check for Nothing Ear and other managed devices
    match find_other_managed_devices(&adapter, managed_devices_mac.clone()).await {
        Ok(devices) => {
            for device in devices {
                let addr_str = device.address().to_string();
                let device_data = devices_list.get(&addr_str).unwrap();
                let type_ = device_data.type_.clone();
                let device_name = device_data.name.clone();
                let app_tx_clone = app_tx.clone();
                let dm_clone = device_managers.clone();
                tokio::spawn(async move {
                    let mut managers = dm_clone.write().await;
                    if type_ == devices::enums::DeviceType::Nothing {
                        let dev = devices::nothing::NothingDevice::new(
                            device.address(),
                            app_tx_clone.clone(),
                        )
                        .await;
                        let dev_managers = DeviceManagers::with_att(dev.att_manager.clone());
                        managers
                            .entry(addr_str.clone())
                            .or_insert(dev_managers)
                            .set_att(dev.att_manager);
                        let _ = app_tx_clone.send(AppEvent::DeviceConnected {
                            mac: addr_str,
                            name: device_name,
                            is_nothing: true,
                            product_id: 0,
                        });
                    }
                });
            }
        }
        Err(e) => {
            if e.kind
                != bluer::ErrorKind::Internal(InternalErrorKind::Io(
                    std::io::ErrorKind::NotFound,
                ))
            {
                log::error!("Error finding other managed devices: {}", e);
            }
        }
    }

    // D-Bus listener for new Bluetooth connections
    let conn = Connection::new_system()?;
    let rule = MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged");
    conn.add_match(rule, move |_: (), conn, msg| {
        let Some(path) = msg.path() else { return true };
        if !path.contains("/org/bluez/hci") || !path.contains("/dev_") {
            return true;
        }
        let Ok((iface, changed, _)) =
            msg.read3::<String, HashMap<String, Variant<Box<dyn RefArg>>>, Vec<String>>()
        else {
            return true;
        };
        if iface != "org.bluez.Device1" {
            return true;
        }
        let Some(connected_var) = changed.get("Connected") else {
            return true;
        };
        let Some(is_connected) = connected_var.0.as_ref().as_u64() else {
            return true;
        };
        let proxy = conn.with_proxy("org.bluez", path, Duration::from_millis(5000));
        let Ok(addr_str) = proxy.get::<String>("org.bluez.Device1", "Address") else {
            return true;
        };

        // Handle disconnects
        if is_connected == 0 {
            let _ = app_tx.send(AppEvent::DeviceDisconnected(addr_str));
            return true;
        }

        let Ok(addr) = addr_str.parse::<Address>() else {
            return true;
        };

        // Nothing Ear or other managed device
        if managed_devices_mac.contains(&addr_str) {
            let Some(dev_data) = devices_list.get(&addr_str) else { return true };
            let type_ = dev_data.type_.clone();
            let device_name = dev_data.name.clone();
            if type_ == devices::enums::DeviceType::Nothing {
                let app_tx_clone = app_tx.clone();
                let dm_clone = device_managers.clone();
                tokio::spawn(async move {
                    let mut managers = dm_clone.write().await;
                    let dev =
                        devices::nothing::NothingDevice::new(addr, app_tx_clone.clone()).await;
                    let dev_managers = DeviceManagers::with_att(dev.att_manager.clone());
                    managers
                        .entry(addr_str.clone())
                        .or_insert(dev_managers)
                        .set_att(dev.att_manager);
                    drop(managers);
                    let _ = app_tx_clone.send(AppEvent::DeviceConnected {
                        mac: addr_str,
                        name: device_name,
                        is_nothing: true,
                        product_id: 0,
                    });
                });
            }
            return true;
        }

        // AirPods: check UUID
        let Ok(uuids) = proxy.get::<Vec<String>>("org.bluez.Device1", "UUIDs") else {
            return true;
        };
        let target_uuid = "74ec2172-0bad-4d01-8f77-997b2be0722a";
        if !uuids.iter().any(|u| u.to_lowercase() == target_uuid) {
            return true;
        }

        let bt_name = proxy
            .get::<String>("org.bluez.Device1", "Name")
            .unwrap_or_else(|_| "Unknown AirPods".to_string());
        let name = devices_list
            .get(&addr_str)
            .filter(|d| !d.name.is_empty())
            .map(|d| d.name.clone())
            .unwrap_or(bt_name);
        let product_id = proxy.get::<String>("org.bluez.Device1", "Modalias")
            .ok()
            .and_then(|m| crate::devices::apple_models::parse_modalias(&m))
            .filter(|(v, _)| *v == crate::devices::apple_models::APPLE_VENDOR_ID)
            .map(|(_, p)| p)
            .unwrap_or(0);
        info!("AirPods connected: {}, product_id=0x{:04x}, initializing", name, product_id);
        let app_tx_clone = app_tx.clone();
        let dm_clone = device_managers.clone();
        tokio::spawn(async move {
            let airpods_device = AirPodsDevice::new(addr, app_tx_clone.clone(), product_id).await;
            let mut managers = dm_clone.write().await;
            let dev_managers = DeviceManagers::with_aacp(airpods_device.aacp_manager.clone());
            managers
                .entry(addr_str.clone())
                .or_insert(dev_managers)
                .set_aacp(airpods_device.aacp_manager);
            drop(managers);
            let _ = app_tx_clone.send(AppEvent::DeviceConnected {
                mac: addr_str,
                name,
                is_nothing: false,
                product_id,
            });
        });
        true
    })?;

    info!("Listening for Bluetooth connections via D-Bus...");
    loop {
        conn.process(Duration::from_millis(1000))?;
    }
}
