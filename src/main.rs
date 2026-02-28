mod bluetooth;
mod config;
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
use futures::StreamExt;
use log::info;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::unbounded_channel;

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
    #[arg(long, help = "Run as headless daemon (no TUI, just maintain connections)")]
    daemon: bool,
}

/// Read the BlueZ Modalias property for a device and return its Apple product ID (0 if unknown).
async fn read_product_id(addr_str: &str) -> u16 {
    use crate::devices::apple_models::{APPLE_VENDOR_ID, parse_modalias};
    let Ok(conn) = zbus::Connection::system().await else { return 0; };
    let path = format!("/org/bluez/hci0/dev_{}", addr_str.replace(':', "_"));
    let Ok(obj_path) = zbus::zvariant::ObjectPath::try_from(path.as_str()) else { return 0; };
    let Ok(proxy) = zbus::proxy::Builder::<'_, zbus::Proxy<'_>>::new(&conn)
        .destination("org.bluez").unwrap()
        .path(obj_path).unwrap()
        .interface("org.bluez.Device1").unwrap()
        .build()
        .await else { return 0; };
    let Ok(val): Result<zbus::zvariant::OwnedValue, _> = proxy.get_property("Modalias").await else { return 0; };
    let Ok(modalias) = String::try_from(val) else { return 0; };
    parse_modalias(&modalias)
        .filter(|(v, _)| *v == APPLE_VENDOR_ID)
        .map(|(_, p)| p)
        .unwrap_or(0)
}

/// Read a single D-Bus property via zbus.
async fn zbus_get_property<T: TryFrom<zbus::zvariant::OwnedValue>>(
    conn: &zbus::Connection,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<T> {
    let obj_path = zbus::zvariant::ObjectPath::try_from(path).ok()?;
    let proxy = zbus::proxy::Builder::<'_, zbus::Proxy<'_>>::new(conn)
        .destination("org.bluez").ok()?
        .path(obj_path).ok()?
        .interface(interface).ok()?
        .build()
        .await.ok()?;
    let val: zbus::zvariant::OwnedValue = proxy.get_property(property).await.ok()?;
    T::try_from(val).ok()
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

    let config = config::Config::load();

    if args.waybar || args.waybar_watch {
        return run_waybar_mode(args.waybar_watch);
    }

    let (app_tx, app_rx) = unbounded_channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<(String, crate::tui::app::DeviceCommand)>();

    let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let dm_clone = device_managers.clone();
    let app_tx_bt = app_tx.clone();
    let bt_config = config.clone();

    if args.daemon {
        // Headless daemon mode: run bluetooth_main directly, drain events
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut app_rx = app_rx;
        rt.spawn(async move {
            while app_rx.recv().await.is_some() {}
        });
        rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx, bt_config))
            .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
        return Ok(());
    }

    // Spawn bluetooth runtime in a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx, bt_config))
            .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
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

    let config = config::Config::load();

    let (app_tx, app_rx) = unbounded_channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<(String, crate::tui::app::DeviceCommand)>();

    let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let dm_clone = device_managers.clone();
    let app_tx_bt = app_tx.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx, config))
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

/// Async task: monitor BlueZ MediaTransport1 volume changes via zbus,
/// sync AirPods stem swipe to system volume using configured commands.
async fn avrcp_volume_monitor(config: config::Config) {
    let Ok(conn) = zbus::Connection::system().await else {
        log::error!("Failed to connect to system D-Bus for AVRCP monitor");
        return;
    };

    let rule = "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'";
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else { return };
    if let Err(e) = proxy.add_match_rule(rule.try_into().unwrap()).await {
        log::error!("Failed to add AVRCP match rule: {}", e);
        return;
    }

    let mut stream = zbus::MessageStream::from(&conn);
    let mut prev_pct: i64 = -1;
    let osd_cmd = config.volume_osd_command.clone();
    let set_cmd = config.volume_set_command.clone();

    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };

        // Only process signals
        let header = msg.header();
        if header.message_type() != zbus::message::Type::Signal {
            continue;
        }

        let Some(path) = header.path() else { continue };
        let path_str = path.as_str();
        if !path_str.contains("/org/bluez/") {
            continue;
        }

        let Some(member) = header.member() else { continue };
        if member.as_str() != "PropertiesChanged" {
            continue;
        }

        // Parse PropertiesChanged body: (interface, changed_props, invalidated)
        let Ok(body) = msg.body().deserialize::<(
            String,
            HashMap<String, zbus::zvariant::OwnedValue>,
            Vec<String>,
        )>() else {
            continue;
        };

        let (iface, changed, _) = body;
        if iface != "org.bluez.MediaTransport1" {
            continue;
        }

        if let Some(vol_val) = changed.get("Volume") {
            let vol: Option<u64> = u16::try_from(vol_val).ok().map(|v| v as u64)
                .or_else(|| u32::try_from(vol_val).ok().map(|v| v as u64))
                .or_else(|| u8::try_from(vol_val).ok().map(|v| v as u64));
            if let Some(vol) = vol {
                let new_pct = ((vol as f64) / 127.0 * 100.0).round() as i64;
                let old_pct = prev_pct;
                prev_pct = new_pct;
                if old_pct >= 0 {
                    let delta = new_pct - old_pct;
                    if delta != 0 {
                        let arg = if delta > 0 { format!("+{}", delta) } else { format!("{}", delta) };
                        config::run_template_cmd(&osd_cmd, &arg);
                        info!("AVRCP volume {} → OSD delta {}%", vol, delta);
                    }
                } else {
                    let frac = (vol as f64) / 127.0;
                    config::run_template_cmd(&set_cmd, &format!("{:.2}", frac));
                    info!("AVRCP volume {} → initial {:.0}%", vol, frac * 100.0);
                }
            }
        }
    }
}

/// Async task: listen for BlueZ device connection/disconnection via zbus PropertiesChanged signals.
async fn bluez_connection_listener(
    conn: zbus::Connection,
    app_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    devices_list: HashMap<String, DeviceData>,
    managed_devices_mac: Vec<String>,
    config: config::Config,
) {
    let rule = "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'";
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else { return };
    if let Err(e) = proxy.add_match_rule(rule.try_into().unwrap()).await {
        log::error!("Failed to add BlueZ match rule: {}", e);
        return;
    }

    let mut stream = zbus::MessageStream::from(&conn);

    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };

        let header = msg.header();
        if header.message_type() != zbus::message::Type::Signal {
            continue;
        }

        let Some(path) = header.path() else { continue };
        let path_str = path.as_str().to_string();
        if !path_str.contains("/org/bluez/hci") || !path_str.contains("/dev_") {
            continue;
        }

        let Ok(body) = msg.body().deserialize::<(
            String,
            HashMap<String, zbus::zvariant::OwnedValue>,
            Vec<String>,
        )>() else {
            continue;
        };

        let (iface, changed, _) = body;
        if iface != "org.bluez.Device1" {
            continue;
        }

        let Some(connected_val) = changed.get("Connected") else {
            continue;
        };
        let Ok(is_connected) = bool::try_from(connected_val) else {
            continue;
        };

        let Some(addr_str) = zbus_get_property::<String>(&conn, &path_str, "org.bluez.Device1", "Address").await else {
            continue;
        };

        if !is_connected {
            let _ = app_tx.send(AppEvent::DeviceDisconnected(addr_str));
            continue;
        }

        let Ok(addr) = addr_str.parse::<Address>() else {
            continue;
        };

        // Nothing Ear or other managed device
        if managed_devices_mac.contains(&addr_str) {
            let Some(dev_data) = devices_list.get(&addr_str) else { continue };
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
            continue;
        }

        // AirPods: check UUID
        let uuids: Option<Vec<String>> = zbus_get_property(&conn, &path_str, "org.bluez.Device1", "UUIDs").await;
        let Some(uuids) = uuids else { continue };
        let target_uuid = "74ec2172-0bad-4d01-8f77-997b2be0722a";
        if !uuids.iter().any(|u| u.to_lowercase() == target_uuid) {
            continue;
        }

        let bt_name: String = zbus_get_property(&conn, &path_str, "org.bluez.Device1", "Name")
            .await
            .unwrap_or_else(|| "Unknown AirPods".to_string());
        let name = devices_list
            .get(&addr_str)
            .filter(|d| !d.name.is_empty())
            .map(|d| d.name.clone())
            .unwrap_or(bt_name);
        let product_id = zbus_get_property::<String>(&conn, &path_str, "org.bluez.Device1", "Modalias")
            .await
            .and_then(|m| crate::devices::apple_models::parse_modalias(&m))
            .filter(|(v, _)| *v == crate::devices::apple_models::APPLE_VENDOR_ID)
            .map(|(_, p)| p)
            .unwrap_or(0);
        info!("AirPods connected: {}, product_id=0x{:04x}, initializing", name, product_id);
        let app_tx_clone = app_tx.clone();
        let dm_clone = device_managers.clone();
        let config_clone = config.clone();
        tokio::spawn(async move {
            let airpods_device = AirPodsDevice::new(addr, app_tx_clone.clone(), product_id, config_clone).await;
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
    }
}

async fn bluetooth_main(
    app_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<(String, crate::tui::app::DeviceCommand)>,
    config: config::Config,
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

    // AVRCP volume monitor (async task, replaces dedicated thread)
    let vol_config = config.clone();
    tokio::spawn(async move {
        avrcp_volume_monitor(vol_config).await;
    });

    // Command dispatcher — receives (mac, DeviceCommand) from TUI
    let dm_cmd = device_managers.clone();
    tokio::spawn(async move {
        while let Some((mac, cmd)) = cmd_rx.recv().await {
            let managers = dm_cmd.read().await;
            if let Some(dm) = managers.get(&mac) {
                if let Some(aacp) = dm.get_aacp() {
                    match cmd {
                        tui::app::DeviceCommand::ControlCommand(id, value) => {
                            if let Err(e) = aacp.send_control_command(id, &value).await {
                                log::error!("Failed to send control command: {}", e);
                            }
                        }
                        tui::app::DeviceCommand::Rename(name) => {
                            if let Err(e) = aacp.send_rename_packet(&name).await {
                                log::error!("Failed to send rename: {}", e);
                            }
                        }
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
            let product_id = read_product_id(&addr_str).await;
            info!("Product ID for {}: 0x{:04x}", addr_str, product_id);
            let airpods_device = AirPodsDevice::new(device.address(), app_tx.clone(), product_id, config.clone()).await;
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

    // Async D-Bus listener for new Bluetooth connections (replaces blocking loop)
    info!("Listening for Bluetooth connections via D-Bus...");
    let conn = zbus::Connection::system().await.map_err(|e| bluer::Error {
        kind: bluer::ErrorKind::Internal(bluer::InternalErrorKind::DBus(e.to_string())),
        message: e.to_string(),
    })?;
    bluez_connection_listener(
        conn,
        app_tx,
        device_managers,
        devices_list,
        managed_devices_mac,
        config,
    )
    .await;

    Ok(())
}
