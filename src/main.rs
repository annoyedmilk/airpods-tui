mod bluetooth;
mod config;
mod devices;
mod ipc;
mod media_controller;
mod tui;
mod utils;

use crate::bluetooth::discovery::find_connected_airpods;
use crate::bluetooth::managers::DeviceManagers;
use crate::devices::enums::DeviceData;
use crate::tui::app::{App, AppEvent};
use crate::utils::get_devices_path;
use bluer::Address;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use devices::airpods::AirPodsDevice;
use futures::StreamExt;
use log::{debug, info};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::unbounded_channel;

use crate::bluetooth::AIRPODS_AACP_UUID;

#[derive(Parser)]
#[command(name = "airpods-tui", about = "AirPods TUI controls for Linux")]
struct Args {
    #[arg(long, short = 'd', help = "Enable debug logging")]
    debug: bool,
    #[arg(long, short = 'v', help = "Show version and exit")]
    version: bool,
    #[arg(long, help = "Print JSON status for waybar and exit")]
    waybar: bool,
    #[arg(
        long,
        help = "Print JSON status for waybar on each change (persistent)"
    )]
    waybar_watch: bool,
    #[arg(
        long,
        help = "Run as headless daemon (no TUI, just maintain connections)"
    )]
    daemon: bool,
}

/// Read the BlueZ Modalias property for a device and return its Apple product ID (0 if unknown).
async fn read_product_id(addr_str: &str) -> u16 {
    use crate::devices::apple_models::{APPLE_VENDOR_ID, parse_modalias};
    let Ok(conn) = zbus::Connection::system().await else {
        return 0;
    };
    let path = format!("/org/bluez/hci0/dev_{}", addr_str.replace(':', "_"));
    zbus_get_property::<String>(&conn, &path, "org.bluez.Device1", "Modalias")
        .await
        .and_then(|m| parse_modalias(&m))
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
    let proxy = match zbus::proxy::Builder::<'_, zbus::Proxy<'_>>::new(conn)
        .destination("org.bluez")
        .ok()?
        .path(obj_path)
        .ok()?
        .interface(interface)
        .ok()?
        .build()
        .await
    {
        Ok(p) => p,
        Err(e) => {
            debug!(
                "Failed to build proxy for {}.{} at {}: {}",
                interface, property, path, e
            );
            return None;
        }
    };
    match proxy.get_property(property).await {
        Ok(val) => T::try_from(val).ok(),
        Err(e) => {
            debug!(
                "Failed to read {}.{} at {}: {}",
                interface, property, path, e
            );
            None
        }
    }
}

/// Check that /etc/bluetooth/main.conf has the Apple vendor DeviceID set.
/// Without it the AirPods will not respond to AACP packets (no battery, no settings).
fn check_bluetooth_config() {
    const CONF: &str = "/etc/bluetooth/main.conf";
    const REQUIRED: &str = "bluetooth:004C:";

    let ok = std::fs::read_to_string(CONF)
        .map(|s| {
            s.lines().any(|l| {
                let l = l.trim();
                !l.starts_with('#') && l.contains(REQUIRED)
            })
        })
        .unwrap_or(false);

    if !ok {
        log::warn!(
            "Apple DeviceID not set in {}. \
             AirPods will not respond to AACP (no battery, no settings). \
             Add the following line under [General] and restart bluetooth, then re-pair:\n  \
             DeviceID = bluetooth:004C:0000:0000\n  \
             sudo systemctl restart bluetooth",
            CONF
        );
        eprintln!(
            "\x1b[33mWARNING\x1b[0m: Apple DeviceID missing in {}.\n\
             Add under [General]:\n  DeviceID = bluetooth:004C:0000:0000\n\
             Then: sudo systemctl restart bluetooth  (and re-pair AirPods)",
            CONF
        );
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    if args.version {
        println!("airpods-tui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let log_level = if args.debug { "debug" } else { "warn" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .target(env_logger::Target::Stderr)
        .init();

    check_bluetooth_config();

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
        let rt = tokio::runtime::Runtime::new()?;
        let exit_code = rt.block_on(async move {
            let snapshot: ipc::StateSnapshot = Arc::new(RwLock::new(Vec::new()));
            let ipc_server = Arc::new(ipc::IpcServer::new(snapshot.clone(), cmd_tx));

            // Task: update snapshot, broadcast events, and check battery thresholds
            let ipc_server_clone = ipc_server.clone();
            let snapshot_clone = snapshot.clone();
            let alert_cmd = config.battery_alert_command.clone();
            let mut app_rx = app_rx;
            tokio::spawn(async move {
                let mut battery_alerted: HashMap<String, u8> = HashMap::new();
                while let Some(event) = app_rx.recv().await {
                    {
                        let mut snap = snapshot_clone.write().await;
                        ipc::update_snapshot(&mut snap, &event);
                    }
                    ipc_server_clone.broadcast(&event);

                    if let AppEvent::AACPEvent(ref mac, ref aacp_event) = event
                        && let crate::bluetooth::aacp::AACPEvent::BatteryInfo(ref infos) =
                            **aacp_event
                    {
                        // Write battery env file from daemon so external consumers
                        // (waybar, scripts) can read it without a TUI running
                        let mut bat_left = None;
                        let mut bat_right = None;
                        let mut bat_case = None;
                        let mut bat_headphone = None;
                        for b in infos {
                            match b.component {
                                crate::bluetooth::aacp::BatteryComponent::Left => {
                                    bat_left = Some(b.level)
                                }
                                crate::bluetooth::aacp::BatteryComponent::Right => {
                                    bat_right = Some(b.level)
                                }
                                crate::bluetooth::aacp::BatteryComponent::Case
                                    if b.status
                                        != crate::bluetooth::aacp::BatteryStatus::Disconnected =>
                                {
                                    bat_case = Some(b.level)
                                }
                                crate::bluetooth::aacp::BatteryComponent::Headphone => {
                                    bat_headphone = Some(b.level)
                                }
                                _ => {}
                            }
                            if b.status == crate::bluetooth::aacp::BatteryStatus::NotCharging {
                                let key = format!("{}-{:?}", mac, b.component);
                                let threshold = if b.level <= 10 {
                                    10u8
                                } else if b.level <= 20 {
                                    20u8
                                } else {
                                    0
                                };
                                let prev = *battery_alerted.get(&key).unwrap_or(&100u8);
                                if threshold > 0 && threshold < prev {
                                    battery_alerted.insert(key, threshold);
                                    let msg = format!("{:?} battery: {}%", b.component, b.level);
                                    config::run_template_cmd(&alert_cmd, &msg);
                                } else if threshold == 0 && prev < 100 {
                                    battery_alerted.insert(key, 100);
                                }
                            }
                        }
                        crate::utils::write_battery_env(
                            bat_left,
                            bat_right,
                            bat_case,
                            bat_headphone,
                        );
                    }
                }
            });

            // Task: IPC server
            let ipc_handle = tokio::spawn(async move {
                if let Err(e) = ipc_server.run().await {
                    log::error!("IPC server error: {}", e);
                }
            });

            // Run bluetooth_main with graceful shutdown on SIGTERM/SIGINT
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");

            let exit_code: i32 = tokio::select! {
                result = bluetooth_main(app_tx_bt, dm_clone, cmd_rx, bt_config) => {
                    match result {
                        Ok(()) => 0,
                        Err(e) => {
                            log::error!("Bluetooth error: {}", e);
                            1
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    log::info!("Received SIGINT, shutting down...");
                    0
                }
                _ = sigterm.recv() => {
                    log::info!("Received SIGTERM, shutting down...");
                    0
                }
            };

            ipc_handle.abort();
            let _ = ipc::socket_path().and_then(std::fs::remove_file);
            log::info!("Daemon shutdown complete");
            exit_code
        });
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return Ok(());
    }

    // Try connecting to a running daemon via IPC first.
    // The runtime must stay alive so the IPC reader/writer tasks keep running.
    let ipc_rt = tokio::runtime::Runtime::new()?;
    let ipc_result = ipc_rt.block_on(ipc::ipc_connect());

    let (_ipc_rt_guard, app_rx, cmd_tx) = if let Ok((ipc_cmd_tx, ipc_event_rx)) = ipc_result {
        info!("Connected to daemon via IPC");
        drop(app_tx_bt);
        drop(dm_clone);
        drop(bt_config);
        drop(app_rx);
        drop(cmd_rx);
        drop(cmd_tx);
        // Keep ipc_rt alive - its spawned tasks handle the socket I/O
        (Some(ipc_rt), ipc_event_rx, ipc_cmd_tx)
    } else {
        drop(ipc_rt);
        info!("No daemon running, starting in-process Bluetooth");
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Runtime::new() else {
                log::error!("Failed to create Tokio runtime for Bluetooth");
                return;
            };
            rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx, bt_config))
                .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
        });
        (None, app_rx, cmd_tx)
    };

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

    // Try IPC first (like the TUI does) to avoid conflicting L2CAP connections
    let ipc_rt = tokio::runtime::Runtime::new()?;
    let ipc_result = ipc_rt.block_on(ipc::ipc_connect());

    let (_ipc_rt_guard, app_rx, cmd_tx) = if let Ok((ipc_cmd_tx, ipc_event_rx)) = ipc_result {
        info!("Waybar: connected to daemon via IPC");
        (Some(ipc_rt), ipc_event_rx, ipc_cmd_tx)
    } else {
        drop(ipc_rt);
        info!("Waybar: no daemon, starting in-process Bluetooth");

        let (app_tx, app_rx) = unbounded_channel::<AppEvent>();
        let (cmd_tx, cmd_rx) = unbounded_channel::<(String, crate::tui::app::DeviceCommand)>();

        let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let dm_clone = device_managers.clone();
        let app_tx_bt = app_tx.clone();

        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Runtime::new() else {
                log::error!("Failed to create Tokio runtime for waybar Bluetooth");
                return;
            };
            rt.block_on(bluetooth_main(app_tx_bt, dm_clone, cmd_rx, config))
                .unwrap_or_else(|e| log::error!("Bluetooth error: {}", e));
        });

        (None, app_rx, cmd_tx)
    };

    fn render_waybar_json(app: &App) -> String {
        match app.selected_device() {
            Some(DeviceState::AirPods(s)) => {
                let model_name = s.model.as_deref().unwrap_or(&s.name);
                let min_bat = [s.battery_left, s.battery_right, s.battery_headphone]
                    .iter()
                    .filter_map(|b| b.as_ref().map(|(l, _)| *l))
                    .min();
                let percentage = min_bat.unwrap_or(0);
                let mut tooltip_parts = vec![model_name.to_string()];
                if let Some((l, _)) = s.battery_left {
                    tooltip_parts.push(format!("L: {}%", l));
                }
                if let Some((r, _)) = s.battery_right {
                    tooltip_parts.push(format!("R: {}%", r));
                }
                if let Some((c, _)) = s.battery_case {
                    tooltip_parts.push(format!("C: {}%", c));
                }
                if let Some((h, _)) = s.battery_headphone {
                    tooltip_parts.push(format!("{}%", h));
                }
                serde_json::json!({
                    "text": format!("{}%", percentage),
                    "tooltip": tooltip_parts.join("\n"),
                    "class": "connected",
                    "percentage": percentage,
                })
                .to_string()
            }
            _ => serde_json::json!({
                "text": "",
                "tooltip": "No AirPods",
                "class": "disconnected",
                "percentage": 0,
            })
            .to_string(),
        }
    }

    let mut app = App::new(app_rx, cmd_tx);
    let deadline = if watch {
        None
    } else {
        Some(std::time::Instant::now() + Duration::from_secs(5))
    };
    let mut last_json = String::new();

    loop {
        // Block until an event arrives or timeout expires (avoids busy-wait polling)
        let remaining = match deadline {
            Some(d) => {
                let now = std::time::Instant::now();
                if now >= d {
                    break;
                }
                d - now
            }
            None => Duration::from_secs(60),
        };
        // tokio's unbounded receiver has no blocking recv-with-timeout usable
        // from sync code, so poll try_recv with short sleeps
        match app.rx.try_recv() {
            Ok(event) => {
                // Process this event plus any others that have queued up
                app.handle_event(event);
                while let Ok(event) = app.rx.try_recv() {
                    app.handle_event(event);
                }
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                // No event available - sleep for a reasonable interval
                std::thread::sleep(remaining.min(Duration::from_secs(1)));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }

        if watch {
            let json = render_waybar_json(&app);
            if json != last_json {
                println!("{}", json);
                last_json = json;
            }
        } else if matches!(app.selected_device(), Some(DeviceState::AirPods(s)) if s.battery_left.is_some() || s.battery_right.is_some())
        {
            break; // battery data settled, answer now
        }
    }

    if !watch {
        // Single-shot: exactly one line, printed after the state settled
        // (battery arrived) or the deadline passed.
        println!("{}", render_waybar_json(&app));
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

    let rule =
        "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'";
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
        debug!("Failed to create DBusProxy for AVRCP volume monitor");
        return;
    };
    if let Err(e) = proxy
        .add_match_rule(rule.try_into().expect("valid match rule"))
        .await
    {
        log::error!("Failed to add AVRCP match rule: {}", e);
        return;
    }

    let mut stream = zbus::MessageStream::from(&conn);
    // -1 = not yet seen.  First event seeds the baseline without adjusting volume.
    let mut applied_pct: i64 = -1;
    // Latest pct received but not yet dispatched (pending debounce).
    let mut pending_pct: Option<i64> = None;
    let set_cmd = config.volume_set_command.clone();
    let osd_cmd = config.volume_osd_command.clone();

    // Debounce: a single stem swipe floods ~15 AVRCP Volume events in quick succession
    // (one per ~9-unit step on the 0-127 scale).  Wait until the stream is quiet for
    // DEBOUNCE_MS, then set the volume ABSOLUTELY to the final AVRCP value.
    //
    // Using an absolute set (volume_set_command) rather than a delta avoids double-applying
    // the change on systems where WirePlumber already syncs AVRCP volume to the PipeWire
    // A2DP sink.  swayosd detects the PulseAudio event and shows the OSD automatically.
    const DEBOUNCE_MS: u64 = 200;
    let debounce_deadline = tokio::time::sleep(Duration::MAX);
    tokio::pin!(debounce_deadline);

    loop {
        tokio::select! {
            // Debounce timer fired - set the absolute target volume.
            () = &mut debounce_deadline, if pending_pct.is_some() => {
                let new_pct = pending_pct.take().unwrap();
                if applied_pct >= 0 {
                    if new_pct != applied_pct {
                        // Pass a 0.0-1.0 fraction to volume_set_command (e.g. wpctl).
                        let fraction = format!("{:.4}", new_pct as f64 / 100.0);
                        config::run_template_cmd(&set_cmd, &fraction);
                        // Show OSD without changing volume (+0 = display only)
                        config::run_template_cmd(&osd_cmd, "+0");
                        info!("AVRCP volume swipe: {}% → {}%", applied_pct, new_pct);
                    }
                } else {
                    info!("AVRCP volume baseline: {}%", new_pct);
                }
                applied_pct = new_pct;
            }

            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { break };

                let header = msg.header();
                if header.message_type() != zbus::message::Type::Signal {
                    continue;
                }
                let Some(path) = header.path() else { continue };
                if !path.as_str().contains("/org/bluez/") {
                    continue;
                }
                let Some(member) = header.member() else { continue };
                if member.as_str() != "PropertiesChanged" {
                    continue;
                }

                let Ok((iface, changed, _)) = msg.body().deserialize::<(
                    String,
                    HashMap<String, zbus::zvariant::OwnedValue>,
                    Vec<String>,
                )>() else {
                    continue;
                };
                if iface != "org.bluez.MediaTransport1" {
                    continue;
                }

                if let Some(vol_val) = changed.get("Volume") {
                    let vol: Option<u64> = u16::try_from(vol_val).ok().map(|v| v as u64)
                        .or_else(|| u32::try_from(vol_val).ok().map(|v| v as u64))
                        .or_else(|| u8::try_from(vol_val).ok().map(|v| v as u64));
                    if let Some(vol) = vol {
                        let new_pct = ((vol as f64) / 127.0 * 100.0).round() as i64;
                        // Update the pending target and reset the debounce window.
                        pending_pct = Some(new_pct);
                        debounce_deadline
                            .as_mut()
                            .reset(tokio::time::Instant::now() + Duration::from_millis(DEBOUNCE_MS));
                    }
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
    config: config::Config,
    reconnect_tx: tokio::sync::mpsc::UnboundedSender<(Address, u16)>,
) {
    let rule =
        "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'";
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
        debug!("Failed to create DBusProxy for BlueZ connection listener");
        return;
    };
    if let Err(e) = proxy
        .add_match_rule(rule.try_into().expect("valid match rule"))
        .await
    {
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

        let Some(addr_str) =
            zbus_get_property::<String>(&conn, &path_str, "org.bluez.Device1", "Address").await
        else {
            continue;
        };

        if !is_connected {
            if let Err(e) = app_tx.send(AppEvent::DeviceDisconnected(addr_str.clone())) {
                debug!("Failed to send DeviceDisconnected for {}: {}", addr_str, e);
            }
            continue;
        }

        let Ok(addr) = addr_str.parse::<Address>() else {
            continue;
        };

        // AirPods: check UUID
        let uuids: Option<Vec<String>> =
            zbus_get_property(&conn, &path_str, "org.bluez.Device1", "UUIDs").await;
        let Some(uuids) = uuids else { continue };
        if !uuids.iter().any(|u| u.to_lowercase() == AIRPODS_AACP_UUID) {
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
        let product_id = read_product_id(&addr_str).await;
        info!(
            "AirPods connected: {}, product_id=0x{:04x}, initializing",
            name, product_id
        );
        spawn_airpods_init(
            addr,
            name,
            product_id,
            AirPodsInitContext {
                app_tx: app_tx.clone(),
                device_managers: device_managers.clone(),
                config: config.clone(),
                reconnect_tx: reconnect_tx.clone(),
            },
        );
    }
}

struct AirPodsInitContext {
    app_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    config: config::Config,
    reconnect_tx: tokio::sync::mpsc::UnboundedSender<(Address, u16)>,
}

enum InitOutcome {
    /// AACP session established and DeviceConnected sent.
    Ready,
    /// Another task already owns (or is initializing) this device.
    AlreadyClaimed,
    /// Init failed; the slot was released and DeviceDisconnected sent.
    Failed,
}

async fn try_airpods_init(
    addr: Address,
    name: String,
    product_id: u16,
    ctx: &AirPodsInitContext,
) -> InitOutcome {
    let addr_str = addr.to_string();

    // Atomically claim the slot under a single write lock. If an entry
    // already exists (either fully ready or another init in progress),
    // bail before the long async init can race with us. The reconnect
    // handler removes stale entries before re-initializing, so a leftover
    // placeholder cannot strand future inits.
    {
        let mut managers = ctx.device_managers.write().await;
        if managers.contains_key(&addr_str) {
            info!(
                "Skipping init for {} - already connected or initializing",
                addr_str
            );
            return InitOutcome::AlreadyClaimed;
        }
        managers.insert(addr_str.clone(), DeviceManagers::placeholder());
    }

    match AirPodsDevice::new(
        addr,
        ctx.app_tx.clone(),
        product_id,
        ctx.config.clone(),
        Some(ctx.reconnect_tx.clone()),
    )
    .await
    {
        Ok(airpods_device) => {
            let mut managers = ctx.device_managers.write().await;
            managers
                .entry(addr_str.clone())
                .and_modify(|dm| dm.set_aacp(airpods_device.aacp_manager.clone()))
                .or_insert_with(|| DeviceManagers::with_aacp(airpods_device.aacp_manager));
            drop(managers);
            // Notify the TUI only once AACP is alive. The handle_aacp_event
            // path auto-creates a placeholder device entry if any AACP event
            // arrived during init, so this ordering is safe.
            if let Err(e) = ctx.app_tx.send(AppEvent::DeviceConnected {
                mac: addr_str.clone(),
                name,
                product_id,
            }) {
                log::warn!("Failed to send DeviceConnected for {}: {}", addr_str, e);
            }
            InitOutcome::Ready
        }
        Err(e) => {
            log::error!("Failed to initialize AirPods device {}: {}", addr_str, e);
            ctx.device_managers.write().await.remove(&addr_str);
            // No DeviceConnected was sent; nothing to roll back. If an AACP
            // event auto-created a placeholder, sweep it now.
            let _ = ctx
                .app_tx
                .send(AppEvent::DeviceDisconnected(addr_str.clone()));
            InitOutcome::Failed
        }
    }
}

fn spawn_airpods_init(addr: Address, name: String, product_id: u16, ctx: AirPodsInitContext) {
    tokio::spawn(async move {
        if matches!(
            try_airpods_init(addr, name, product_id, &ctx).await,
            InitOutcome::Failed
        ) {
            // Fresh connects often race BlueZ profile setup and die with
            // ENOTCONN/ECONNABORTED on the first L2CAP attempt. Hand the
            // device to the reconnect loop, which keeps retrying with
            // backoff while BlueZ still reports it connected.
            let _ = ctx.reconnect_tx.send((addr, product_id));
        }
    });
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

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    // AVRCP volume monitor
    let vol_config = config.clone();
    tokio::spawn(async move {
        avrcp_volume_monitor(vol_config).await;
    });

    // Command dispatcher - receives (mac, DeviceCommand) from TUI
    let dm_cmd = device_managers.clone();
    let adapter_cmd = adapter.clone();
    tokio::spawn(async move {
        while let Some((mac, cmd)) = cmd_rx.recv().await {
            let managers = dm_cmd.read().await;
            if let Some(dm) = managers.get(&mac)
                && let Some(aacp) = dm.get_aacp()
            {
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
                        // Set BlueZ alias with retry (no disconnect - avoids iPhone reclaiming the name)
                        if let Ok(addr) = mac.parse::<Address>()
                            && let Ok(device) = adapter_cmd.device(addr)
                        {
                            for _ in 0..3 {
                                if device.set_alias(name.clone()).await.is_ok() {
                                    log::info!("BlueZ alias updated to '{}'", name);
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            }
                        }
                    }
                }
            }
        }
    });

    // Reconnect channel: fed by ConnectionLost (L2CAP died) and by failed
    // inits. Retries with backoff for as long as BlueZ still reports the
    // device connected; once the BT link itself is gone, the connection
    // listener owns recovery via the next Connected=true event.
    let (reconnect_tx, mut reconnect_rx) = unbounded_channel::<(Address, u16)>();
    {
        let app_tx = app_tx.clone();
        let dm = device_managers.clone();
        let cfg = config.clone();
        let reconnect_tx = reconnect_tx.clone();
        let dl = devices_list.clone();
        let adapter = adapter.clone();
        tokio::spawn(async move {
            while let Some((addr, product_id)) = reconnect_rx.recv().await {
                let addr_str = addr.to_string();
                // Drop the dead session, but never touch a healthy or
                // still-initializing one (queued retries can be stale).
                {
                    let mut managers = dm.write().await;
                    if let Some(existing) = managers.get(&addr_str) {
                        let Some(aacp) = existing.get_aacp() else {
                            continue; // init in progress elsewhere
                        };
                        if aacp.state.lock().await.sender.is_some() {
                            continue; // healthy session, stale retry
                        }
                        managers.remove(&addr_str);
                    }
                }
                // Re-read the name from our store (may have been renamed)
                let name = dl
                    .get(&addr_str)
                    .filter(|d| !d.name.is_empty())
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "AirPods".to_string());
                let ctx = AirPodsInitContext {
                    app_tx: app_tx.clone(),
                    device_managers: dm.clone(),
                    config: cfg.clone(),
                    reconnect_tx: reconnect_tx.clone(),
                };
                let mut attempt: u32 = 0;
                loop {
                    attempt += 1;
                    let delay = Duration::from_secs((1u64 << attempt.min(5)).min(30));
                    info!(
                        "AACP reconnect: {} ({}) attempt {} in {:?}",
                        name, addr, attempt, delay
                    );
                    tokio::time::sleep(delay).await;
                    if dm.read().await.contains_key(&addr_str) {
                        break; // another path claimed the device
                    }
                    let bluez_connected = match adapter.device(addr) {
                        Ok(device) => device.is_connected().await.unwrap_or(false),
                        Err(_) => false,
                    };
                    if !bluez_connected {
                        info!(
                            "{} is gone at the BlueZ level; the connection listener resumes when it returns",
                            addr_str
                        );
                        break;
                    }
                    match try_airpods_init(addr, name.clone(), product_id, &ctx).await {
                        InitOutcome::Ready | InitOutcome::AlreadyClaimed => break,
                        InitOutcome::Failed => continue,
                    }
                }
            }
        });
    }

    // Start D-Bus listener FIRST to avoid missing connections during startup checks
    info!("Listening for Bluetooth connections via D-Bus...");
    let conn = zbus::Connection::system().await.map_err(|e| bluer::Error {
        kind: bluer::ErrorKind::Internal(bluer::InternalErrorKind::DBus(e.to_string())),
        message: e.to_string(),
    })?;
    let listener_handle = {
        let app_tx = app_tx.clone();
        let dm = device_managers.clone();
        let dl = devices_list.clone();
        let cfg = config.clone();
        let rtx = reconnect_tx.clone();
        tokio::spawn(async move {
            bluez_connection_listener(conn, app_tx, dm, dl, cfg, rtx).await;
        })
    };

    // Now check for already-connected devices (listener is already active)
    info!("Checking for connected devices...");
    match find_connected_airpods(&adapter).await {
        Ok(device) => {
            let bt_name = device
                .name()
                .await?
                .unwrap_or_else(|| "Unknown AirPods".to_string());
            let addr_str = device.address().to_string();
            let name = devices_list
                .get(&addr_str)
                .filter(|d| !d.name.is_empty())
                .map(|d| d.name.clone())
                .unwrap_or(bt_name);
            info!("Found connected AirPods: {}, initializing.", name);
            let product_id = read_product_id(&addr_str).await;
            info!("Product ID for {}: 0x{:04x}", addr_str, product_id);
            spawn_airpods_init(
                device.address(),
                name,
                product_id,
                AirPodsInitContext {
                    app_tx: app_tx.clone(),
                    device_managers: device_managers.clone(),
                    config: config.clone(),
                    reconnect_tx: reconnect_tx.clone(),
                },
            );
        }
        Err(_) => {
            info!("No connected AirPods found.");
        }
    }

    // Block on the D-Bus listener
    let _ = listener_handle.await;

    Ok(())
}
