use crate::bluetooth::aacp::AACPManager;
use crate::bluetooth::aacp::EarDetectionStatus;
use crate::config::Config;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect::SinkInfo;
use libpulse_binding::context::{Context, FlagSet as ContextFlagSet};
use libpulse_binding::def::Retval;
use libpulse_binding::mainloop::standard::Mainloop;
use libpulse_binding::operation::State as OperationState;
use libpulse_binding::proplist::Proplist;
use libpulse_binding::volume::{ChannelVolumes, Volume};
use log::{debug, error, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

// ── PulseAudio thread: single long-lived Mainloop + Context ──

#[derive(Clone)]
struct OwnedCardProfileInfo {
    name: Option<String>,
}

#[derive(Clone)]
struct OwnedCardInfo {
    index: u32,
    proplist: Proplist,
    profiles: Vec<OwnedCardProfileInfo>,
}

#[derive(Clone)]
struct OwnedSinkInfo {
    name: Option<String>,
    proplist: Proplist,
    volume: ChannelVolumes,
}

enum AudioCommand {
    IsA2dpAvailable {
        card_index: u32,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    GetDeviceIndex {
        mac: String,
        reply: tokio::sync::oneshot::Sender<Option<u32>>,
    },
    SetCardProfile {
        card_index: u32,
        profile: String,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    GetSinkVolume {
        sink_name: String,
        reply: tokio::sync::oneshot::Sender<Option<u32>>,
    },
    TransitionVolume {
        sink_name: String,
        target: u32,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    GetSinkNameByMac {
        mac: String,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
    IsProfileAvailable {
        card_index: u32,
        profile: String,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
}

/// Spawn a single background thread that owns the PulseAudio Mainloop + Context.
/// Returns a sender for issuing commands.
fn spawn_audio_thread(
    app_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tui::app::AppEvent>>,
) -> std::sync::mpsc::Sender<AudioCommand> {
    let (tx, rx) = std::sync::mpsc::channel::<AudioCommand>();

    std::thread::spawn(move || {
        let fail = |msg: &str| {
            error!("{}", msg);
            if let Some(ref tx) = app_tx {
                let _ = tx.send(crate::tui::app::AppEvent::AudioUnavailable);
            }
        };
        let mut mainloop = match Mainloop::new() {
            Some(m) => m,
            None => {
                fail("Failed to create PulseAudio mainloop");
                return;
            }
        };
        let mut context = match Context::new(&mainloop, "airpods-tui") {
            Some(c) => c,
            None => {
                fail("Failed to create PulseAudio context");
                return;
            }
        };
        if context
            .connect(None, ContextFlagSet::NOAUTOSPAWN, None)
            .is_err()
        {
            fail("Failed to connect PulseAudio context");
            return;
        }

        // Wait for Ready state
        loop {
            match mainloop.iterate(true) {
                _ if context.get_state() == libpulse_binding::context::State::Ready => break,
                _ if context.get_state() == libpulse_binding::context::State::Failed
                    || context.get_state() == libpulse_binding::context::State::Terminated =>
                {
                    fail("PulseAudio context failed during connect");
                    return;
                }
                _ => {}
            }
        }
        info!("PulseAudio audio thread connected and ready");

        // Process commands
        while let Ok(cmd) = rx.recv() {
            match cmd {
                AudioCommand::IsA2dpAvailable { card_index, reply } => {
                    let result = pa_is_a2dp_available(&mut mainloop, &context, card_index);
                    let _ = reply.send(result);
                }
                AudioCommand::GetDeviceIndex { mac, reply } => {
                    let result = pa_get_device_index(&mut mainloop, &context, &mac);
                    let _ = reply.send(result);
                }
                AudioCommand::SetCardProfile {
                    card_index,
                    profile,
                    reply,
                } => {
                    let result =
                        pa_set_card_profile(&mut mainloop, &mut context, card_index, &profile);
                    let _ = reply.send(result);
                }
                AudioCommand::GetSinkVolume { sink_name, reply } => {
                    let result = pa_get_sink_volume(&mut mainloop, &context, &sink_name);
                    let _ = reply.send(result);
                }
                AudioCommand::TransitionVolume {
                    sink_name,
                    target,
                    reply,
                } => {
                    let result =
                        pa_transition_volume(&mut mainloop, &mut context, &sink_name, target);
                    let _ = reply.send(result);
                }
                AudioCommand::GetSinkNameByMac { mac, reply } => {
                    let result = pa_get_sink_name_by_mac(&mut mainloop, &context, &mac);
                    let _ = reply.send(result);
                }
                AudioCommand::IsProfileAvailable {
                    card_index,
                    profile,
                    reply,
                } => {
                    let result =
                        pa_is_profile_available(&mut mainloop, &context, card_index, &profile);
                    let _ = reply.send(result);
                }
            }
        }

        mainloop.quit(Retval(0));
        info!("PulseAudio audio thread exiting");
    });

    tx
}

// ── Synchronous PA helpers (run inside the audio thread) ──

fn pa_get_card_info_list(mainloop: &mut Mainloop, context: &Context) -> Vec<OwnedCardInfo> {
    let introspector = context.introspect();
    let card_info_list = Rc::new(RefCell::new(None));
    let op = introspector.get_card_info_list({
        let card_info_list = card_info_list.clone();
        let mut list = Vec::new();
        move |result| match result {
            ListResult::Item(item) => {
                let profiles = item
                    .profiles
                    .iter()
                    .map(|p| OwnedCardProfileInfo {
                        name: p.name.as_ref().map(|n| n.to_string()),
                    })
                    .collect();
                list.push(OwnedCardInfo {
                    index: item.index,
                    proplist: item.proplist.clone(),
                    profiles,
                });
            }
            ListResult::End => *card_info_list.borrow_mut() = Some(list.clone()),
            ListResult::Error => *card_info_list.borrow_mut() = None,
        }
    });
    while op.get_state() == OperationState::Running {
        mainloop.iterate(false);
    }
    card_info_list.borrow().clone().unwrap_or_default()
}

fn pa_is_a2dp_available(mainloop: &mut Mainloop, context: &Context, card_index: u32) -> bool {
    let cards = pa_get_card_info_list(mainloop, context);
    cards
        .iter()
        .find(|c| c.index == card_index)
        .map(|card| {
            card.profiles
                .iter()
                .any(|p| p.name.as_ref().is_some_and(|n| n.starts_with("a2dp-sink")))
        })
        .unwrap_or(false)
}

fn pa_get_device_index(mainloop: &mut Mainloop, context: &Context, mac: &str) -> Option<u32> {
    let cards = pa_get_card_info_list(mainloop, context);
    for card in &cards {
        if let Some(device_string) = card.proplist.get_str("device.string") {
            if device_string.contains(mac) {
                return Some(card.index);
            }
        }
    }
    None
}

fn pa_set_card_profile(
    mainloop: &mut Mainloop,
    context: &mut Context,
    card_index: u32,
    profile: &str,
) -> bool {
    let mut introspector = context.introspect();
    let op = introspector.set_card_profile_by_index(card_index, profile, None);
    while op.get_state() == OperationState::Running {
        mainloop.iterate(false);
    }
    true
}

fn pa_get_sink_volume(mainloop: &mut Mainloop, context: &Context, sink_name: &str) -> Option<u32> {
    let introspector = context.introspect();
    let sink_info_option = Rc::new(RefCell::new(None));
    let op = introspector.get_sink_info_by_name(sink_name, {
        let sink_info_option = sink_info_option.clone();
        move |result: ListResult<&SinkInfo>| {
            if let ListResult::Item(item) = result {
                let owned_item = OwnedSinkInfo {
                    name: item.name.as_ref().map(|s| s.to_string()),
                    proplist: item.proplist.clone(),
                    volume: item.volume,
                };
                *sink_info_option.borrow_mut() = Some(owned_item);
            }
        }
    });
    while op.get_state() == OperationState::Running {
        mainloop.iterate(false);
    }
    if let Some(sink_info) = sink_info_option.borrow().as_ref() {
        let channels = sink_info.volume.len();
        if channels == 0 {
            return None;
        }
        let total: f64 = sink_info.volume.get().iter().map(|v| v.0 as f64).sum();
        let average_raw = total / channels as f64;
        let percent = ((average_raw / Volume::NORMAL.0 as f64) * 100.0).round() as u32;
        Some(percent)
    } else {
        None
    }
}

fn pa_transition_volume(
    mainloop: &mut Mainloop,
    context: &mut Context,
    sink_name: &str,
    target_volume: u32,
) -> bool {
    let introspector = context.introspect();
    let sink_info_option = Rc::new(RefCell::new(None));
    let op = introspector.get_sink_info_by_name(sink_name, {
        let sink_info_option = sink_info_option.clone();
        move |result: ListResult<&SinkInfo>| {
            if let ListResult::Item(item) = result {
                let owned_item = OwnedSinkInfo {
                    name: item.name.as_ref().map(|s| s.to_string()),
                    proplist: item.proplist.clone(),
                    volume: item.volume,
                };
                *sink_info_option.borrow_mut() = Some(owned_item);
            }
        }
    });
    while op.get_state() == OperationState::Running {
        mainloop.iterate(false);
    }
    if let Some(sink_info) = sink_info_option.borrow().as_ref() {
        let channels = sink_info.volume.len();
        let mut new_volumes = ChannelVolumes::default();
        let raw =
            (((target_volume as f64) / 100.0) * (Volume::NORMAL.0 as f64)).round() as u32;
        let vol = Volume(raw);
        new_volumes.set(channels, vol);

        let mut introspector = context.introspect();
        let op = introspector.set_sink_volume_by_name(sink_name, &new_volumes, None);
        while op.get_state() == OperationState::Running {
            mainloop.iterate(false);
        }
        true
    } else {
        error!("Sink not found: {}", sink_name);
        false
    }
}

fn pa_get_sink_name_by_mac(mainloop: &mut Mainloop, context: &Context, mac: &str) -> Option<String> {
    let introspector = context.introspect();
    let sink_info_list = Rc::new(RefCell::new(Some(Vec::new())));
    let op = introspector.get_sink_info_list({
        let sink_info_list = sink_info_list.clone();
        move |result: ListResult<&SinkInfo>| {
            if let ListResult::Item(item) = result {
                let owned_item = OwnedSinkInfo {
                    name: item.name.as_ref().map(|s| s.to_string()),
                    proplist: item.proplist.clone(),
                    volume: item.volume,
                };
                sink_info_list
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .push(owned_item);
            }
        }
    });
    while op.get_state() == OperationState::Running {
        mainloop.iterate(false);
    }

    if let Some(list) = sink_info_list.borrow().as_ref() {
        for sink in list {
            if let Some(device_string) = sink.proplist.get_str("device.string") {
                if device_string
                    .to_uppercase()
                    .contains(&mac.to_uppercase())
                {
                    if let Some(name) = &sink.name {
                        return Some(name.to_string());
                    }
                }
            }
            if let Some(bluez_path) = sink.proplist.get_str("bluez.path") {
                let mac_from_path = bluez_path
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .replace("dev_", "")
                    .replace('_', ":");
                if mac_from_path.eq_ignore_ascii_case(mac) {
                    if let Some(name) = &sink.name {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

fn pa_is_profile_available(
    mainloop: &mut Mainloop,
    context: &Context,
    card_index: u32,
    profile: &str,
) -> bool {
    let cards = pa_get_card_info_list(mainloop, context);
    cards
        .iter()
        .find(|c| c.index == card_index)
        .map(|card| {
            card.profiles
                .iter()
                .any(|p| p.name.as_ref() == Some(&profile.to_string()))
        })
        .unwrap_or(false)
}

// ── Async wrappers: send command + await oneshot reply ──

async fn audio_cmd_is_a2dp(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    card_index: u32,
) -> bool {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::IsA2dpAvailable {
        card_index,
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(false)
}

async fn audio_cmd_get_device_index(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    mac: &str,
) -> Option<u32> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::GetDeviceIndex {
        mac: mac.to_string(),
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(None)
}

async fn audio_cmd_set_card_profile(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    card_index: u32,
    profile: &str,
) -> bool {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::SetCardProfile {
        card_index,
        profile: profile.to_string(),
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(false)
}

async fn audio_cmd_get_sink_volume(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    sink_name: &str,
) -> Option<u32> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::GetSinkVolume {
        sink_name: sink_name.to_string(),
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(None)
}

async fn audio_cmd_transition_volume(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    sink_name: &str,
    target: u32,
) -> bool {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::TransitionVolume {
        sink_name: sink_name.to_string(),
        target,
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(false)
}

async fn audio_cmd_get_sink_name_by_mac(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    mac: &str,
) -> Option<String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::GetSinkNameByMac {
        mac: mac.to_string(),
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(None)
}

async fn audio_cmd_is_profile_available(
    tx: &std::sync::mpsc::Sender<AudioCommand>,
    card_index: u32,
    profile: &str,
) -> bool {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(AudioCommand::IsProfileAvailable {
        card_index,
        profile: profile.to_string(),
        reply: reply_tx,
    });
    reply_rx.await.unwrap_or(false)
}

// ── MediaController ──

struct MediaControllerState {
    connected_device_mac: String,
    local_mac: String,
    is_playing: bool,
    paused_by_app_services: Vec<String>,
    device_index: Option<u32>,
    cached_a2dp_profile: String,
    old_in_ear_data: Vec<bool>,
    user_played_the_media: bool,
    i_paused_the_media: bool,
    ear_detection_enabled: bool,
    disconnect_when_not_wearing: bool,
    conv_original_volume: Option<u32>,
    conv_conversation_started: bool,
    playback_listener_running: bool,
    config: Config,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
    session_conn: Option<zbus::Connection>,
}

impl MediaControllerState {
    fn new(
        config: Config,
        app_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tui::app::AppEvent>>,
    ) -> Self {
        let audio_tx = spawn_audio_thread(app_tx);
        MediaControllerState {
            connected_device_mac: String::new(),
            local_mac: String::new(),
            is_playing: false,
            paused_by_app_services: Vec::new(),
            device_index: None,
            cached_a2dp_profile: String::new(),
            old_in_ear_data: vec![false, false],
            user_played_the_media: false,
            i_paused_the_media: false,
            ear_detection_enabled: true,
            disconnect_when_not_wearing: true,
            conv_original_volume: None,
            conv_conversation_started: false,
            playback_listener_running: false,
            config,
            audio_tx,
            session_conn: None,
        }
    }
}

#[derive(Clone)]
pub struct MediaController {
    state: Arc<Mutex<MediaControllerState>>,
}

impl MediaController {
    pub fn new(
        connected_mac: String,
        local_mac: String,
        config: Config,
        app_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tui::app::AppEvent>>,
    ) -> Self {
        let mut state = MediaControllerState::new(config, app_tx);
        state.connected_device_mac = connected_mac;
        state.local_mac = local_mac;
        MediaController {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Get or create a cached session D-Bus connection for MPRIS calls.
    async fn session_conn(&self) -> Option<zbus::Connection> {
        let mut state = self.state.lock().await;
        if let Some(ref conn) = state.session_conn {
            return Some(conn.clone());
        }
        match zbus::Connection::session().await {
            Ok(conn) => {
                state.session_conn = Some(conn.clone());
                Some(conn)
            }
            Err(e) => {
                error!("Failed to connect to session D-Bus: {}", e);
                None
            }
        }
    }

    pub async fn start_playback_listener(
        &self,
        aacp_manager: AACPManager,
        control_tx: tokio::sync::mpsc::UnboundedSender<(
            crate::bluetooth::aacp::ControlCommandIdentifiers,
            Vec<u8>,
        )>,
    ) {
        let mut state = self.state.lock().await;
        if state.playback_listener_running {
            debug!("Playback listener already running");
            return;
        }
        state.playback_listener_running = true;
        drop(state);

        let controller_clone = self.clone();
        tokio::spawn(async move {
            controller_clone
                .playback_listener_loop(aacp_manager, control_tx)
                .await;
        });
    }

    async fn playback_listener_loop(
        &self,
        aacp_manager: AACPManager,
        control_tx: tokio::sync::mpsc::UnboundedSender<(
            crate::bluetooth::aacp::ControlCommandIdentifiers,
            Vec<u8>,
        )>,
    ) {
        info!("Starting playback listener loop");
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            let is_playing = self.check_if_playing_async().await;

            let mut state = self.state.lock().await;
            let was_playing = state.is_playing;
            state.is_playing = is_playing;
            let local_mac = state.local_mac.clone();
            drop(state);

            if !was_playing && is_playing {
                let aacp_state = aacp_manager.state.lock().await;
                if !aacp_state
                    .ear_detection_status
                    .contains(&EarDetectionStatus::InEar)
                {
                    info!("Media playback started but buds not in ear, skipping takeover");
                    continue;
                }
                info!("Media playback started, taking ownership and activating a2dp");
                let _ = control_tx.send((
                    crate::bluetooth::aacp::ControlCommandIdentifiers::OwnsConnection,
                    vec![0x01],
                ));
                self.activate_a2dp_profile().await;

                info!("already connected locally, hijacking connection by asking AirPods");

                let connected_devices = aacp_state.connected_devices.clone();
                for device in connected_devices {
                    if device.mac != local_mac {
                        if let Err(e) = aacp_manager
                            .send_media_information(&local_mac, &device.mac, true)
                            .await
                        {
                            error!("Failed to send media information to {}: {}", device.mac, e);
                        }
                        if let Err(e) = aacp_manager.send_smart_routing_show_ui(&device.mac).await {
                            error!(
                                "Failed to send smart routing show ui to {}: {}",
                                device.mac, e
                            );
                        }
                        if let Err(e) = aacp_manager.send_hijack_request(&device.mac).await {
                            error!("Failed to send hijack request to {}: {}", device.mac, e);
                        }
                    }
                }

                debug!("completed playback takeover process");
            }
        }
    }

    async fn check_if_playing_async(&self) -> bool {
        let Some(conn) = self.session_conn().await else { return false };
        let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else { return false };
        let Ok(names) = proxy.list_names().await else { return false };

        for name in names {
            let service = name.as_str();
            if !service.starts_with("org.mpris.MediaPlayer2.") {
                continue;
            }
            if Self::is_kdeconnect_service(service) {
                continue;
            }
            if let Ok(p) = zbus::Proxy::new(
                &conn,
                service,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            ).await {
                if let Ok(status) = p.get_property::<String>("PlaybackStatus").await {
                    if status == "Playing" {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn is_kdeconnect_service(service: &str) -> bool {
        service.starts_with("org.mpris.MediaPlayer2.kdeconnect.mpris_")
    }

    pub async fn handle_ear_detection(
        &self,
        old_statuses: Vec<EarDetectionStatus>,
        new_statuses: Vec<EarDetectionStatus>,
    ) {
        debug!(
            "Entering handle_ear_detection with old_statuses: {:?}, new_statuses: {:?}",
            old_statuses, new_statuses
        );

        let old_in_ear_data: Vec<bool> = old_statuses
            .iter()
            .map(|s| *s == EarDetectionStatus::InEar)
            .collect();
        let new_in_ear_data: Vec<bool> = new_statuses
            .iter()
            .map(|s| *s == EarDetectionStatus::InEar)
            .collect();

        let in_ear = new_in_ear_data.iter().all(|&b| b);

        let old_all_out = old_in_ear_data.iter().all(|&b| !b);
        let new_has_at_least_one_in = new_in_ear_data.iter().any(|&b| b);
        let new_all_out = new_in_ear_data.iter().all(|&b| !b);

        debug!(
            "Computed states: in_ear={}, old_all_out={}, new_has_at_least_one_in={}, new_all_out={}",
            in_ear, old_all_out, new_has_at_least_one_in, new_all_out
        );

        {
            let state = self.state.lock().await;
            if !state.ear_detection_enabled {
                debug!("Ear detection disabled, skipping");
                return;
            }
        }

        if new_has_at_least_one_in && old_all_out {
            debug!("Condition met: buds inserted, activating A2DP and checking play state");
            self.activate_a2dp_profile().await;
            {
                let mut state = self.state.lock().await;
                if state.is_playing {
                    state.user_played_the_media = true;
                    debug!("Set user_played_the_media to true as media was playing");
                }
            }
        } else if new_all_out {
            debug!("Condition met: buds removed, pausing media");
            self.pause().await;
            {
                let state = self.state.lock().await;
                if state.disconnect_when_not_wearing {
                    debug!("Disconnect when not wearing enabled, deactivating A2DP");
                    drop(state);
                    self.deactivate_a2dp_profile().await;
                }
            }
        }

        let reset_user_played = (old_in_ear_data.iter().any(|&b| !b)
            && new_in_ear_data.iter().all(|&b| b))
            || (new_in_ear_data.iter().any(|&b| !b) && old_in_ear_data.iter().all(|&b| b));
        if reset_user_played {
            debug!("Transition detected, resetting user_played_the_media");
            let mut state = self.state.lock().await;
            state.user_played_the_media = false;
        }

        info!(
            "Ear Detection - old_in_ear_data: {:?}, new_in_ear_data: {:?}",
            old_in_ear_data, new_in_ear_data
        );

        let mut old_sorted = old_in_ear_data.clone();
        old_sorted.sort();
        let mut new_sorted = new_in_ear_data.clone();
        new_sorted.sort();
        if new_sorted != old_sorted {
            debug!("Ear data changed, checking resume/pause logic");
            if in_ear {
                debug!("Resuming media as buds are in ear");
                self.resume().await;
                {
                    let mut state = self.state.lock().await;
                    state.i_paused_the_media = false;
                }
            } else if !old_all_out {
                debug!("Pausing media as buds are not fully in ear");
                self.pause().await;
                {
                    let mut state = self.state.lock().await;
                    state.i_paused_the_media = true;
                }
            } else {
                debug!("Playing media");
                self.resume().await;
                {
                    let mut state = self.state.lock().await;
                    state.i_paused_the_media = false;
                }
            }
        }

        {
            let mut state = self.state.lock().await;
            state.old_in_ear_data = new_in_ear_data;
            debug!("Updated old_in_ear_data to {:?}", state.old_in_ear_data);
        }
    }

    pub async fn activate_a2dp_profile(&self) {
        debug!("Entering activate_a2dp_profile");
        let state = self.state.lock().await;

        if state.connected_device_mac.is_empty() {
            warn!("Connected device MAC is empty, cannot activate A2DP profile");
            return;
        }

        let device_index = state.device_index;
        let mac = state.connected_device_mac.clone();
        let audio_tx = state.audio_tx.clone();
        drop(state);

        let mut current_device_index = device_index;

        if current_device_index.is_none() {
            warn!("Device index not found, trying to get it.");
            current_device_index = audio_cmd_get_device_index(&audio_tx, &mac).await;
            if let Some(idx) = current_device_index {
                let mut state = self.state.lock().await;
                state.device_index = Some(idx);
            } else {
                warn!("Could not get device index. Cannot activate A2DP profile.");
                return;
            }
        }

        let idx = current_device_index.unwrap();

        if !audio_cmd_is_a2dp(&audio_tx, idx).await {
            warn!("A2DP profile not available, attempting to restart audio server");
            if self.restart_wire_plumber().await {
                let mut state = self.state.lock().await;
                state.device_index = audio_cmd_get_device_index(&state.audio_tx, &state.connected_device_mac).await;
                let new_idx = state.device_index;
                let audio_tx = state.audio_tx.clone();
                drop(state);
                if let Some(new_idx) = new_idx {
                    if !audio_cmd_is_a2dp(&audio_tx, new_idx).await {
                        error!("A2DP profile still not available after audio server restart");
                        return;
                    }
                } else {
                    error!("Could not get device index after audio server restart");
                    return;
                }
            } else {
                error!("Could not restart audio server, A2DP profile unavailable");
                return;
            }
        }

        let preferred_profile = self.get_preferred_a2dp_profile().await;
        if preferred_profile.is_empty() {
            error!("No suitable A2DP profile found");
            return;
        }

        info!("Activating A2DP profile for AirPods: {}", preferred_profile);
        let state = self.state.lock().await;
        let device_index = state.device_index;
        let audio_tx = state.audio_tx.clone();
        drop(state);

        if let Some(idx) = device_index {
            let success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {})).is_ok();
            let _ = success; // unused, just keeping structure
            let ok = audio_cmd_set_card_profile(&audio_tx, idx, &preferred_profile).await;
            if ok {
                info!("Successfully activated A2DP profile: {}", preferred_profile);
            } else {
                warn!("Failed to activate A2DP profile: {}", preferred_profile);
            }
        } else {
            error!("Device index not available for activating profile.");
        }
    }

    async fn pause(&self) {
        debug!("Pausing playback");

        let Some(conn) = self.session_conn().await else { return };
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else { return };
        let Ok(names) = dbus.list_names().await else { return };
        let mut paused_services = Vec::new();

        for name in names {
            let service = name.as_str();
            if !service.starts_with("org.mpris.MediaPlayer2.") || Self::is_kdeconnect_service(service) {
                continue;
            }
            if let Ok(p) = zbus::Proxy::new(&conn, service, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await {
                if let Ok(status) = p.get_property::<String>("PlaybackStatus").await {
                    if status == "Playing" {
                        if p.call_noreply("Pause", &()).await.is_ok() {
                            info!("Paused playback for: {}", service);
                            paused_services.push(service.to_string());
                        } else {
                            error!("Failed to pause {}", service);
                        }
                    }
                }
            }
        }

        if !paused_services.is_empty() {
            info!("Paused {} media player(s) via DBus", paused_services.len());
            let mut state = self.state.lock().await;
            state.paused_by_app_services = paused_services;
            state.is_playing = false;
        } else {
            info!("No playing media players found to pause");
        }
    }

    async fn mpris_call_first(&self, method: &str) {
        let Some(conn) = self.session_conn().await else { return };
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else { return };
        let Ok(names) = dbus.list_names().await else { return };
        for name in names {
            let service = name.as_str();
            if !service.starts_with("org.mpris.MediaPlayer2.") || Self::is_kdeconnect_service(service) {
                continue;
            }
            if let Ok(p) = zbus::Proxy::new(&conn, service, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await {
                if p.call_noreply(method, &()).await.is_ok() {
                    info!("{} for: {}", method, service);
                    break;
                }
            }
        }
    }

    pub async fn toggle_play_pause(&self) {
        debug!("Toggling play/pause via MPRIS");
        self.mpris_call_first("PlayPause").await;
    }

    pub async fn next_track(&self) {
        debug!("Next track via MPRIS");
        self.mpris_call_first("Next").await;
    }

    pub async fn previous_track(&self) {
        debug!("Previous track via MPRIS");
        self.mpris_call_first("Previous").await;
    }

    pub async fn pause_all_media(&self) {
        debug!("Pausing all media (without tracking for resume)");

        let Some(conn) = self.session_conn().await else { return };
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else { return };
        let Ok(names) = dbus.list_names().await else { return };
        let mut paused_count = 0;

        for name in names {
            let service = name.as_str();
            if !service.starts_with("org.mpris.MediaPlayer2.") || Self::is_kdeconnect_service(service) {
                continue;
            }
            if let Ok(p) = zbus::Proxy::new(&conn, service, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await {
                if let Ok(status) = p.get_property::<String>("PlaybackStatus").await {
                    if status == "Playing" {
                        if p.call_noreply("Pause", &()).await.is_ok() {
                            info!("Paused playback for: {}", service);
                            paused_count += 1;
                        } else {
                            error!("Failed to pause {}", service);
                        }
                    }
                }
            }
        }

        if paused_count > 0 {
            info!(
                "Paused {} media player(s) due to ownership loss",
                paused_count
            );
            let mut state = self.state.lock().await;
            state.is_playing = false;
        }
    }

    async fn resume(&self) {
        debug!("Resuming playback");
        let state = self.state.lock().await;
        let services = state.paused_by_app_services.clone();
        drop(state);

        if services.is_empty() {
            info!("No services to resume");
            return;
        }

        let Some(conn) = self.session_conn().await else { return };
        let mut resumed_count = 0;
        for service in &services {
            if Self::is_kdeconnect_service(service) {
                continue;
            }
            if let Ok(p) = zbus::Proxy::new(&conn, service.as_str(), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await {
                if p.call_noreply("Play", &()).await.is_ok() {
                    info!("Resumed playback for: {}", service);
                    resumed_count += 1;
                } else {
                    warn!("Failed to resume {}", service);
                }
            }
        }

        if resumed_count > 0 {
            info!("Resumed {} media player(s) via DBus", resumed_count);
            let mut state = self.state.lock().await;
            state.paused_by_app_services.clear();
        } else {
            error!("Failed to resume any media players via DBus");
        }
    }

    async fn get_preferred_a2dp_profile(&self) -> String {
        let state = self.state.lock().await;
        let device_index = state.device_index;
        let cached_profile = state.cached_a2dp_profile.clone();
        let audio_tx = state.audio_tx.clone();
        drop(state);

        let index = match device_index {
            Some(i) => i,
            None => return String::new(),
        };

        if !cached_profile.is_empty()
            && audio_cmd_is_profile_available(&audio_tx, index, &cached_profile).await
        {
            return cached_profile;
        }

        let profiles_to_check = ["a2dp-sink-sbc_xq", "a2dp-sink-sbc", "a2dp-sink"];
        for profile in profiles_to_check {
            if audio_cmd_is_profile_available(&audio_tx, index, profile).await {
                info!("Selected best available A2DP profile: {}", profile);
                let mut state = self.state.lock().await;
                state.cached_a2dp_profile = profile.to_string();
                return profile.to_string();
            }
        }
        String::new()
    }

    async fn restart_wire_plumber(&self) -> bool {
        debug!("Entering restart_wire_plumber");
        let state = self.state.lock().await;
        let cmd = state.config.restart_audio_server.clone();
        drop(state);

        let Some(cmd) = cmd else {
            info!("No restart_audio_server configured, skipping audio server restart");
            return false;
        };
        if cmd.is_empty() {
            return false;
        }

        info!("Restarting audio server: {:?}", cmd);
        let result = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .output();

        match result {
            Ok(output) if output.status.success() => {
                info!("Audio server restarted successfully");
                tokio::time::sleep(Duration::from_secs(2)).await;
                true
            }
            _ => {
                error!("Failed to restart audio server via {:?}", cmd);
                false
            }
        }
    }

    pub async fn deactivate_a2dp_profile(&self) {
        debug!("Entering deactivate_a2dp_profile");
        let mut state = self.state.lock().await;

        if state.device_index.is_none() {
            let mac = state.connected_device_mac.clone();
            let audio_tx = state.audio_tx.clone();
            state.device_index = audio_cmd_get_device_index(&audio_tx, &mac).await;
        }

        if state.connected_device_mac.is_empty() || state.device_index.is_none() {
            warn!("Connected device MAC or index is empty, cannot deactivate A2DP profile");
            return;
        }
        let device_index = state.device_index.unwrap();
        let audio_tx = state.audio_tx.clone();
        drop(state);

        info!("Deactivating A2DP profile for AirPods by setting to off");
        let ok = audio_cmd_set_card_profile(&audio_tx, device_index, "off").await;
        if ok {
            info!("Successfully deactivated A2DP profile");
        } else {
            warn!("Failed to deactivate A2DP profile");
        }
    }

    pub async fn handle_conversational_awareness(&self, status: u8) {
        debug!(
            "Entering handle_conversational_awareness with status: {}",
            status
        );

        let (mac, audio_tx) = {
            let state = self.state.lock().await;
            (state.connected_device_mac.clone(), state.audio_tx.clone())
        };
        if mac.is_empty() {
            debug!("No connected device MAC, skipping conversational awareness");
            return;
        }

        let sink_name = audio_cmd_get_sink_name_by_mac(&audio_tx, &mac).await;
        let sink = match sink_name {
            Some(s) => s,
            None => {
                warn!(
                    "Could not find sink for MAC {}, skipping conversational awareness",
                    mac
                );
                return;
            }
        };

        let current_volume_opt = audio_cmd_get_sink_volume(&audio_tx, &sink).await;

        match status {
            1 => {
                let original = current_volume_opt.unwrap_or(0);
                debug!("Conversation start (1). Current volume: {}", original);
                {
                    let mut state = self.state.lock().await;
                    if !state.conv_conversation_started {
                        state.conv_original_volume = Some(original);
                        state.conv_conversation_started = true;
                    }
                }
                if original > 25 {
                    audio_cmd_transition_volume(&audio_tx, &sink, 25).await;
                    info!(
                        "Conversation start: lowered volume to 25% (original {})",
                        original
                    );
                }
            }
            2 => {
                let original = {
                    let state = self.state.lock().await;
                    state.conv_original_volume
                };
                if let Some(orig) = original {
                    if orig > 15 {
                        audio_cmd_transition_volume(&audio_tx, &sink, 15).await;
                        info!(
                            "Conversation reduce: lowered volume to 15% (original {})",
                            orig
                        );
                    }
                }
            }
            3 => {
                let maybe_orig = {
                    let state = self.state.lock().await;
                    (state.conv_conversation_started, state.conv_original_volume)
                };
                if !maybe_orig.0 {
                    return;
                }
                if let Some(orig) = maybe_orig.1 {
                    let target = if orig > 25 { 25 } else { orig };
                    audio_cmd_transition_volume(&audio_tx, &sink, target).await;
                    info!(
                        "Conversation partial increase (3): set volume to {} (original {})",
                        target, orig
                    );
                } else if let Some(orig_from_current) = current_volume_opt {
                    let target = if orig_from_current > 25 {
                        25
                    } else {
                        orig_from_current
                    };
                    audio_cmd_transition_volume(&audio_tx, &sink, target).await;
                }
            }
            4 | 6 | 7 => {
                #[allow(unused_assignments)]
                let mut maybe_original = None;
                {
                    let mut state = self.state.lock().await;
                    if state.conv_conversation_started {
                        maybe_original = state.conv_original_volume;
                        state.conv_original_volume = None;
                        state.conv_conversation_started = false;
                    } else {
                        debug!(
                            "Received status {} but conversation was not started; ignoring restore",
                            status
                        );
                        return;
                    }
                }
                if let Some(orig) = maybe_original {
                    audio_cmd_transition_volume(&audio_tx, &sink, orig).await;
                    info!("Conversation end ({}): restored volume to original {}", status, orig);
                }
            }
            _ => {
                debug!("Unknown conversational awareness status: {}", status);
            }
        }
    }
}
