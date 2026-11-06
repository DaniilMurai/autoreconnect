use std::collections::HashSet;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use tao::event::{Event as LoopEvent, StartCause};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use time::{OffsetDateTime, UtcOffset};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::core::events::{Event, EventKind, EventSink};
use crate::core::model::{Health, RecoveryKind};
use crate::platform::{PermissionState, Permissions};

#[derive(Clone, Debug, PartialEq)]
pub struct UiModel {
    pub health: Health,
    pub paused: bool,
    pub network: Option<String>,
    /// The last 5 `recovery_action` events, newest last (AC-8).
    pub recent: Vec<Event>,
    pub start_at_login: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCommand {
    Pause,
    Resume,
    SetStartAtLogin(bool),
    RetryCredential,
    Quit,
}

/// What reaches the main thread through the `EventLoopProxy`.
#[derive(Debug)]
pub enum TrayEvent {
    Model(UiModel),
    Notice(Notice),
    Menu(MenuEvent),
}

/// Facts `UiModel` does not carry yet, read off the event stream by `TapSink`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    LogFailed,
    LogWorks,
    CredentialMissing(Option<String>),
    CredentialImported(Option<String>),
}

/// Wraps the core's sink so the tray learns about write failures and missing passwords
/// without a change to `run_core`.
pub struct TapSink {
    inner: Box<dyn EventSink>,
    notify: Box<dyn Fn(Notice) + Send>,
    failing: bool,
}

impl TapSink {
    pub fn new(inner: Box<dyn EventSink>, notify: Box<dyn Fn(Notice) + Send>) -> Self {
        Self { inner, notify, failing: false }
    }
}

impl EventSink for TapSink {
    fn write(&mut self, event: &Event) -> std::io::Result<()> {
        let result = self.inner.write(event);
        if result.is_err() != self.failing {
            self.failing = result.is_err();
            (self.notify)(if self.failing { Notice::LogFailed } else { Notice::LogWorks });
        }
        match event.kind {
            EventKind::CredentialMissing { .. } => (self.notify)(Notice::CredentialMissing(event.network.clone())),
            EventKind::CredentialImported => (self.notify)(Notice::CredentialImported(event.network.clone())),
            _ => {}
        }
        result
    }

    fn recent_recovery_actions(&self, n: usize) -> Vec<Event> {
        self.inner.recent_recovery_actions(n)
    }
}

pub struct TrayState {
    pub model: UiModel,
    pub log_unavailable: bool,
    /// SSIDs whose password the core could not get this run.
    pub missing_credentials: HashSet<String>,
    pub location: PermissionState,
    /// The Location prompt is open and the core has not started yet.
    pub awaiting_location: bool,
    pub offset: UtcOffset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuSpec {
    pub status: String,
    pub location: Option<&'static str>,
    pub pause: &'static str,
    pub retry_password: bool,
    /// Newest first.
    pub recent: Vec<String>,
    pub log_unavailable: bool,
}

impl TrayState {
    pub fn new(recent: Vec<Event>, location: PermissionState, offset: UtcOffset) -> Self {
        let model = UiModel { health: Health::Unknown, paused: false, network: None, recent, start_at_login: false };
        Self {
            model,
            log_unavailable: false,
            missing_credentials: HashSet::new(),
            location,
            awaiting_location: false,
            offset,
        }
    }

    pub fn apply(&mut self, notice: Notice) {
        match notice {
            Notice::LogFailed => self.log_unavailable = true,
            Notice::LogWorks => self.log_unavailable = false,
            Notice::CredentialMissing(Some(ssid)) => {
                self.missing_credentials.insert(ssid);
            }
            Notice::CredentialImported(Some(ssid)) => {
                self.missing_credentials.remove(&ssid);
            }
            Notice::CredentialMissing(None) | Notice::CredentialImported(None) => {}
        }
    }

    pub fn menu(&self, now: OffsetDateTime) -> MenuSpec {
        let m = &self.model;
        let location = match self.location {
            _ if self.awaiting_location => Some("Waiting for the Location answer…"),
            PermissionState::Denied => Some("Location off: network name unknown"),
            _ => None,
        };
        let today = now.to_offset(self.offset).date();
        MenuSpec {
            status: status_line(m.health, m.network.as_deref()),
            location,
            pause: if m.paused { "Resume" } else { "Pause" },
            retry_password: m.network.as_ref().is_some_and(|n| self.missing_credentials.contains(n)),
            recent: m.recent.iter().rev().filter_map(|e| recent_line(e, self.offset, today)).collect(),
            log_unavailable: self.log_unavailable,
        }
    }
}

fn status_line(health: Health, network: Option<&str>) -> String {
    let text = match health {
        Health::Healthy => "Connected",
        Health::Degraded => "Connection is poor",
        Health::NoInternet => "No internet, recovering",
        Health::UpstreamDown => "Router has no internet",
        Health::NotAssociated => "Not connected, rejoining",
        Health::WifiOffByUser => "Wi-Fi is off",
        Health::Paused => "Paused",
        Health::Unknown => "Starting…",
    };
    match network {
        Some(n) if matches!(health, Health::Healthy | Health::Degraded | Health::NoInternet | Health::UpstreamDown) => {
            format!("{text}: {n}")
        }
        _ => text.to_string(),
    }
}

fn recent_line(event: &Event, offset: UtcOffset, today: time::Date) -> Option<String> {
    let EventKind::RecoveryAction { kind, ok, duration_ms, error } = &event.kind else { return None };
    let ts = event.ts.to_offset(offset);
    let when = if ts.date() == today {
        format!("{:02}:{:02}", ts.hour(), ts.minute())
    } else {
        format!("{} {} {:02}:{:02}", &ts.month().to_string()[..3], ts.day(), ts.hour(), ts.minute())
    };
    let what = match kind {
        RecoveryKind::Rejoin => "Rejoin",
        RecoveryKind::Reassociate => "Reassociate",
        RecoveryKind::PowerCycle => "Power cycle",
    };
    let result = match (ok, error) {
        (true, _) => format!("ok, {:.1} s", *duration_ms as f64 / 1000.0),
        (false, Some(e)) => format!("failed: {e}"),
        (false, None) => "failed".to_string(),
    };
    Some(format!("{when}  {what}, {result}"))
}

const PROMPT_WAIT: Duration = Duration::from_secs(60);
const PROMPT_POLL: Duration = Duration::from_millis(250);
/// After the core started without an answer, a later one only changes the menu line.
const LATE_ANSWER_POLL: Duration = Duration::from_secs(2);

pub fn event_loop() -> EventLoop<TrayEvent> {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<TrayEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop
}

/// Never returns. The core starts once Location is answered or `PROMPT_WAIT` passes, so a
/// first-launch prompt no longer blocks the tray (T-09 pumped `NSRunLoop` in `main` instead).
pub fn run(
    event_loop: EventLoop<TrayEvent>,
    mut state: TrayState,
    permissions: Box<dyn Permissions>,
    commands: Sender<UiCommand>,
    start_core: Box<dyn FnOnce(PermissionState) -> JoinHandle<()>>,
) -> ! {
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(TrayEvent::Menu(e));
    }));
    let mut start_core = Some(start_core);
    let mut core: Option<JoinHandle<()>> = None;
    let mut tray: Option<(TrayIcon, Menu)> = None;
    let mut prompt_deadline = Instant::now();

    event_loop.run(move |event, _, flow| {
        let mut changed = false;
        match event {
            // tray-icon wants the icon created once the macOS event loop is running.
            LoopEvent::NewEvents(StartCause::Init) => {
                state.location = permissions.location();
                if state.location == PermissionState::NotDetermined {
                    permissions.request_location();
                    state.awaiting_location = true;
                    prompt_deadline = Instant::now() + PROMPT_WAIT;
                } else if let Some(start) = start_core.take() {
                    core = Some(start(state.location));
                }
                tray = Some(build_tray(&state));
            }
            LoopEvent::NewEvents(_) if state.location == PermissionState::NotDetermined => {
                let answer = permissions.location();
                if answer != state.location {
                    state.location = answer;
                    changed = true;
                }
                if state.awaiting_location && (answer != PermissionState::NotDetermined || Instant::now() >= prompt_deadline)
                {
                    state.awaiting_location = false;
                    changed = true;
                    if let Some(start) = start_core.take() {
                        core = Some(start(answer));
                    }
                }
            }
            LoopEvent::UserEvent(TrayEvent::Model(model)) => {
                state.model = model;
                changed = true;
            }
            LoopEvent::UserEvent(TrayEvent::Notice(notice)) => {
                state.apply(notice);
                changed = true;
            }
            LoopEvent::UserEvent(TrayEvent::Menu(e)) => {
                let command = match e.id.as_ref() {
                    "pause" => UiCommand::Pause,
                    "resume" => UiCommand::Resume,
                    "retry" => UiCommand::RetryCredential,
                    "quit" => UiCommand::Quit,
                    _ => return,
                };
                let _ = commands.send(command);
                if command == UiCommand::Quit {
                    // An unfinished power-cycle would leave Wi-Fi off, so the core finishes its step first.
                    if let Some(core) = core.take() {
                        let _ = core.join();
                    }
                    *flow = ControlFlow::Exit;
                    return;
                }
            }
            _ => {}
        }
        if changed && let Some((tray, menu)) = &tray {
            update_tray(tray, menu, &state);
        }
        *flow = match state.location {
            PermissionState::NotDetermined if state.awaiting_location => ControlFlow::WaitUntil(Instant::now() + PROMPT_POLL),
            PermissionState::NotDetermined => ControlFlow::WaitUntil(Instant::now() + LATE_ANSWER_POLL),
            _ => ControlFlow::Wait,
        };
    })
}

fn build_tray(state: &TrayState) -> (TrayIcon, Menu) {
    let spec = state.menu(OffsetDateTime::now_utc());
    let menu = Menu::new();
    fill_menu(&menu, &spec);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_tooltip(format!("AutoReconnect: {}", spec.status))
        .with_icon(icon(state.model.health))
        .build()
        .expect("cannot create the tray icon");
    (tray, menu)
}

fn update_tray(tray: &TrayIcon, menu: &Menu, state: &TrayState) {
    let spec = state.menu(OffsetDateTime::now_utc());
    fill_menu(menu, &spec);
    let _ = tray.set_tooltip(Some(format!("AutoReconnect: {}", spec.status)));
    let _ = tray.set_icon(Some(icon(state.model.health)));
}

/// Refilled on every change: the recent list varies in length and muda items cannot be hidden.
/// The same `Menu` is kept because `set_menu` with a new one left it unreachable to Accessibility.
fn fill_menu(menu: &Menu, spec: &MenuSpec) {
    while menu.remove_at(0).is_some() {}
    let info = |text: &str| MenuItem::new(text, false, None);
    let _ = menu.append(&info(&spec.status));
    if let Some(location) = spec.location {
        let _ = menu.append(&info(location));
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let pause_id = if spec.pause == "Pause" { "pause" } else { "resume" };
    let _ = menu.append(&MenuItem::with_id(pause_id, spec.pause, true, None));
    if spec.retry_password {
        let _ = menu.append(&MenuItem::with_id("retry", "Retry password", true, None));
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&info("Recent reconnects"));
    if spec.recent.is_empty() {
        let _ = menu.append(&info("  none yet"));
    }
    for line in &spec.recent {
        let _ = menu.append(&info(&format!("  {line}")));
    }
    if spec.log_unavailable {
        let _ = menu.append(&info("Event log unavailable"));
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("quit", "Quit AutoReconnect", true, None));
}

fn icon(health: Health) -> Icon {
    const SIZE: u32 = 32;
    let (rgb, hollow) = match health {
        Health::Healthy => ([52, 199, 89], false),
        Health::Degraded | Health::UpstreamDown => ([255, 159, 10], false),
        Health::NoInternet | Health::NotAssociated => ([255, 59, 48], false),
        Health::Paused => ([142, 142, 147], true),
        Health::WifiOffByUser | Health::Unknown => ([142, 142, 147], false),
    };
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let c = SIZE as f32 / 2.0 - 0.5;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let inside = d <= 11.0 && (!hollow || d >= 8.0);
            let [r, g, b] = rgb;
            rgba.extend_from_slice(&[r, g, b, if inside { 255 } else { 0 }]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("icon buffer has the declared size")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use time::macros::{datetime, offset};

    use super::*;

    fn action(ts: OffsetDateTime, kind: RecoveryKind, ok: bool, ms: u64, error: Option<&str>) -> Event {
        Event {
            ts,
            health: Health::NoInternet,
            signal: None,
            network: Some("Home".into()),
            kind: EventKind::RecoveryAction { kind, ok, duration_ms: ms, error: error.map(str::to_string) },
        }
    }

    fn state(recent: Vec<Event>) -> TrayState {
        TrayState::new(recent, PermissionState::Granted, offset!(+1))
    }

    const NOW: OffsetDateTime = datetime!(2026-11-06 15:00 +1);

    #[test]
    fn recent_lines_are_the_last_five_actions_newest_first_in_local_time() {
        let recent = vec![
            action(datetime!(2026-11-05 22:30 UTC), RecoveryKind::Rejoin, true, 2100, None),
            action(datetime!(2026-11-06 09:01 +1), RecoveryKind::Reassociate, true, 6200, None),
            action(datetime!(2026-11-06 12:15 UTC), RecoveryKind::PowerCycle, false, 3, Some("dry_run")),
            action(datetime!(2026-11-06 13:20 +1), RecoveryKind::Rejoin, false, 10, None),
            action(datetime!(2026-11-06 14:59:59.999 +1), RecoveryKind::Reassociate, true, 50, None),
        ];
        let spec = state(recent).menu(NOW);
        assert_eq!(
            spec.recent,
            vec![
                "14:59  Reassociate, ok, 0.1 s",
                "13:20  Rejoin, failed",
                "13:15  Power cycle, failed: dry_run",
                "09:01  Reassociate, ok, 6.2 s",
                "Nov 5 23:30  Rejoin, ok, 2.1 s",
            ]
        );
    }

    #[test]
    fn non_action_events_are_not_listed() {
        let mut other = action(NOW, RecoveryKind::Rejoin, true, 1, None);
        other.kind = EventKind::Paused;
        assert!(state(vec![other]).menu(NOW).recent.is_empty());
    }

    #[test]
    fn paused_model_offers_resume() {
        let mut s = state(Vec::new());
        assert_eq!(s.menu(NOW).pause, "Pause");
        s.model.paused = true;
        s.model.health = Health::Paused;
        let spec = s.menu(NOW);
        assert_eq!(spec.pause, "Resume");
        assert_eq!(spec.status, "Paused");
    }

    #[test]
    fn retry_password_shows_only_for_the_current_network_while_missing() {
        let mut s = state(Vec::new());
        s.model.network = Some("Home".into());
        assert!(!s.menu(NOW).retry_password);
        s.apply(Notice::CredentialMissing(Some("Home".into())));
        assert!(s.menu(NOW).retry_password);
        s.model.network = Some("Cafe".into());
        assert!(!s.menu(NOW).retry_password);
        s.model.network = Some("Home".into());
        s.apply(Notice::CredentialImported(Some("Home".into())));
        assert!(!s.menu(NOW).retry_password);
    }

    #[test]
    fn location_line_while_waiting_and_when_denied() {
        let mut s = state(Vec::new());
        assert_eq!(s.menu(NOW).location, None);
        s.awaiting_location = true;
        assert_eq!(s.menu(NOW).location, Some("Waiting for the Location answer…"));
        s.awaiting_location = false;
        s.location = PermissionState::Denied;
        assert_eq!(s.menu(NOW).location, Some("Location off: network name unknown"));
    }

    struct FlakySink(Arc<Mutex<bool>>);

    impl EventSink for FlakySink {
        fn write(&mut self, _: &Event) -> std::io::Result<()> {
            if *self.0.lock().unwrap() { Err(std::io::Error::other("disk full")) } else { Ok(()) }
        }
        fn recent_recovery_actions(&self, _: usize) -> Vec<Event> {
            Vec::new()
        }
    }

    #[test]
    fn tap_sink_reports_log_failure_once_and_recovery() {
        let fail = Arc::new(Mutex::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let notify = {
            let seen = seen.clone();
            Box::new(move |n| seen.lock().unwrap().push(n))
        };
        let mut sink = TapSink::new(Box::new(FlakySink(fail.clone())), notify);
        let mut missing = action(NOW, RecoveryKind::Rejoin, true, 1, None);
        missing.kind = EventKind::CredentialMissing { reason: "denied".into() };
        sink.write(&missing).unwrap();
        *fail.lock().unwrap() = true;
        assert!(sink.write(&action(NOW, RecoveryKind::Rejoin, true, 1, None)).is_err());
        assert!(sink.write(&action(NOW, RecoveryKind::Rejoin, true, 1, None)).is_err());
        *fail.lock().unwrap() = false;
        sink.write(&action(NOW, RecoveryKind::Rejoin, true, 1, None)).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![Notice::CredentialMissing(Some("Home".into())), Notice::LogFailed, Notice::LogWorks]
        );

        let mut s = state(Vec::new());
        s.apply(Notice::LogFailed);
        assert!(s.menu(NOW).log_unavailable);
        s.apply(Notice::LogWorks);
        assert!(!s.menu(NOW).log_unavailable);
    }
}
