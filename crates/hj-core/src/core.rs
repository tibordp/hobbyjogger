//! The core actor: owns [`AppState`] and the machine connection, gates and executes requests, and
//! publishes changes through a [`Sink`].

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hj_makera::command::{self, Realtime};
use hj_makera::connection::{Connection as Link, Event};
use hj_makera::discovery::{self, Announcement, DEFAULT_TCP_PORT};
use hj_makera::message::Message;
use hj_makera::status::Status;
use hj_makera::transport;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use ts_rs::TS;

use crate::state::*;
use crate::sync::{self, Patch};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DISCOVERY_TTL: Duration = Duration::from_secs(6);
const CONSOLE_HISTORY: usize = 2000;
/// How far inside the soft limits jogs stop. Targeting a limit exactly can trip it through
/// rounding (seen on hardware), which halts the machine.
const LIMIT_MARGIN: f64 = 0.01;
/// Soft-limit config keys read after connecting, per axis.
const LIMIT_KEYS: [&str; 3] = [
    "soft_endstop.x_min",
    "soft_endstop.y_min",
    "soft_endstop.z_min",
];

/// Receives everything the core publishes. Implemented by the Tauri layer (and by tests).
pub trait Sink: Send + Sync + 'static {
    fn state(&self, update: &StateUpdate);
    fn telemetry(&self, telemetry: &Telemetry);
    fn console(&self, entries: &[ConsoleEntry]);
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct StateUpdate {
    #[ts(type = "number")]
    pub seq: u64,
    pub patches: Vec<Patch>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Snapshot {
    #[ts(type = "number")]
    pub seq: u64,
    pub state: AppState,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("not connected")]
    NotConnected,
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Failed(String),
    #[error("core stopped")]
    Stopped,
}

impl Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

struct Envelope {
    request: Request,
    reply: oneshot::Sender<Result<(), CoreError>>,
}

#[derive(Default)]
struct Shared {
    seq: u64,
    state: AppState,
    console: VecDeque<ConsoleEntry>,
}

/// Handle to the running core.
#[derive(Clone)]
pub struct Core {
    tx: mpsc::Sender<Envelope>,
    shared: Arc<Mutex<Shared>>,
}

impl Core {
    /// Start the core on the current tokio runtime.
    pub fn start(sink: Arc<dyn Sink>) -> Self {
        let (tx, rx) = mpsc::channel(64);
        let (disc_tx, disc_rx) = mpsc::channel(16);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut actor = Actor {
            sink,
            shared: shared.clone(),
            state: AppState::default(),
            published: serde_json::to_value(AppState::default()).unwrap(),
            seq: 0,
            session: None,
            last_status: None,
            discovered: HashMap::new(),
            simulator: None,
            console_id: 0,
            pending_console: Vec::new(),
        };
        tokio::spawn(async move {
            match discovery::listen() {
                Ok(listener) => {
                    tokio::spawn(async move {
                        while let Ok((Some(a), ..)) = listener.recv().await {
                            if disc_tx.send(a).await.is_err() {
                                return;
                            }
                        }
                    });
                }
                Err(e) => actor.note(
                    ConsoleKind::Error,
                    format!(
                        "WiFi discovery unavailable (UDP {}): {e}",
                        discovery::DISCOVERY_PORT
                    ),
                ),
            }
            actor.run(rx, disc_rx).await;
        });
        Self { tx, shared }
    }

    pub async fn request(&self, request: Request) -> Result<(), CoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Envelope { request, reply })
            .await
            .map_err(|_| CoreError::Stopped)?;
        rx.await.map_err(|_| CoreError::Stopped)?
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.shared.lock().unwrap();
        Snapshot {
            seq: s.seq,
            state: s.state.clone(),
        }
    }

    pub fn console_history(&self) -> Vec<ConsoleEntry> {
        self.shared
            .lock()
            .unwrap()
            .console
            .iter()
            .cloned()
            .collect()
    }
}

struct Session {
    link: Link,
}

struct Actor {
    sink: Arc<dyn Sink>,
    shared: Arc<Mutex<Shared>>,
    state: AppState,
    published: Value,
    seq: u64,
    session: Option<Session>,
    last_status: Option<Status>,
    discovered: HashMap<String, (DiscoveredMachine, Instant)>,
    simulator: Option<SocketAddr>,
    console_id: u64,
    pending_console: Vec<ConsoleEntry>,
}

async fn next_event(session: &mut Option<Session>) -> Option<Event> {
    match session {
        Some(s) => s.link.events.recv().await,
        None => std::future::pending().await,
    }
}

impl Actor {
    async fn run(
        &mut self,
        mut requests: mpsc::Receiver<Envelope>,
        mut discovery: mpsc::Receiver<Announcement>,
    ) {
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.flush();
        loop {
            tokio::select! {
                envelope = requests.recv() => {
                    let Some(Envelope { request, reply }) = envelope else { return };
                    let result = self.handle(request).await;
                    if let Err(e) = &result {
                        self.note(ConsoleKind::Error, e.to_string());
                    }
                    let _ = reply.send(result);
                }
                event = next_event(&mut self.session) => match event {
                    Some(event) => self.on_event(event),
                    None => self.disconnected(Some("connection closed".into())),
                },
                _ = poll.tick() => {
                    self.poll().await;
                }
                Some(a) = discovery.recv() => self.on_announcement(a),
            }
            self.flush();
        }
    }

    // ---- publishing ----

    fn flush(&mut self) {
        let value = serde_json::to_value(&self.state).expect("state serializes");
        if value != self.published {
            let patches = sync::diff(&self.published, &value);
            self.seq += 1;
            {
                let mut shared = self.shared.lock().unwrap();
                shared.seq = self.seq;
                shared.state = self.state.clone();
            }
            self.sink.state(&StateUpdate {
                seq: self.seq,
                patches,
            });
            self.published = value;
        }
        if !self.pending_console.is_empty() {
            let entries = std::mem::take(&mut self.pending_console);
            {
                let mut shared = self.shared.lock().unwrap();
                shared.console.extend(entries.iter().cloned());
                while shared.console.len() > CONSOLE_HISTORY {
                    shared.console.pop_front();
                }
            }
            self.sink.console(&entries);
        }
    }

    fn note(&mut self, kind: ConsoleKind, text: String) {
        self.console_id += 1;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.pending_console.push(ConsoleEntry {
            id: self.console_id,
            time,
            kind,
            text,
        });
    }

    // ---- machine I/O ----

    async fn send_raw(&mut self, frame: hj_makera::frame::Frame) -> Result<(), CoreError> {
        let session = self.session.as_ref().ok_or(CoreError::NotConnected)?;
        if session.link.sender.send(frame).await.is_err() {
            self.disconnected(Some("connection closed".into()));
            return Err(CoreError::NotConnected);
        }
        Ok(())
    }

    /// Send a console/G-code line, logging it to the console.
    async fn send_line(&mut self, line: &str) -> Result<(), CoreError> {
        self.send_raw(command::command(line)).await?;
        self.note(ConsoleKind::Sent, line.to_owned());
        Ok(())
    }

    async fn poll(&mut self) {
        let now = Instant::now();
        let before = self.discovered.len();
        self.discovered
            .retain(|_, (_, seen)| now.duration_since(*seen) < DISCOVERY_TTL);
        if self.discovered.len() != before {
            self.refresh_discovered();
        }
        if self.session.is_some() {
            let _ = self.send_raw(Realtime::Status.frame()).await;
        }
    }

    fn on_announcement(&mut self, a: Announcement) {
        let key = a.addr.to_string();
        let m = DiscoveredMachine {
            name: a.name,
            host: a.addr.ip().to_string(),
            port: a.addr.port(),
            busy: a.busy,
            state: a.state,
        };
        self.discovered.insert(key, (m, Instant::now()));
        self.refresh_discovered();
    }

    fn refresh_discovered(&mut self) {
        let mut list: Vec<DiscoveredMachine> =
            self.discovered.values().map(|(m, _)| m.clone()).collect();
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.host.cmp(&b.host)));
        self.state.discovered = list;
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Message(Message::Status(status)) => self.on_status(*status),
            Event::Message(Message::Line(line)) => self.on_line(line),
            Event::Message(Message::Malformed { line, error }) => {
                self.note(ConsoleKind::Error, format!("{error}: {line}"))
            }
            Event::Closed(err) => {
                self.disconnected(Some(match err {
                    Some(e) => format!("connection lost: {e}"),
                    None => "connection closed by machine".into(),
                }));
            }
            // Raw frames, diagnose, load and transfer frames aren't used yet.
            _ => {}
        }
    }

    fn on_line(&mut self, line: String) {
        if let Some(v) = line.strip_prefix("version = ") {
            self.state.machine.get_or_insert_default().firmware = Some(v.trim().to_owned());
        } else if let Some(v) = line.strip_prefix("model = ") {
            let model = v.split(',').next().unwrap_or_default().trim().to_owned();
            self.state.machine.get_or_insert_default().model = Some(model);
        } else if let Some(rest) = line.strip_prefix("sd: ") {
            // `sd: soft_endstop.x_min is set to -207.0`
            if let Some((key, value)) = rest.split_once(" is set to ")
                && let Some(i) = LIMIT_KEYS.iter().position(|k| *k == key)
                && let Ok(v) = value.trim().parse::<f64>()
            {
                self.state.machine.get_or_insert_default().travel.min[i] = Some(v);
            }
        }
        let lower = line.to_ascii_lowercase();
        let kind = if lower.starts_with("alarm") || lower.starts_with("error") || line == "!!" {
            ConsoleKind::Error
        } else {
            ConsoleKind::Received
        };
        self.note(kind, line);
    }

    fn on_status(&mut self, s: Status) {
        let state = MachineState::from(&s.state);
        let round = |v: f64| (v * 1e4).round() / 1e4;
        let m = s.machine_pos.0;
        let w = s.work_pos.0;
        self.sink.telemetry(&Telemetry {
            machine: m,
            work: w,
            feed: s.feed.map_or(0.0, |f| f.current),
            spindle_rpm: s.spindle.as_ref().map_or(0.0, |sp| sp.current_rpm),
            spindle_temp: s.spindle.as_ref().and_then(|sp| sp.spindle_temp),
            power_temp: s.spindle.as_ref().and_then(|sp| sp.power_temp),
            live: !matches!(
                state,
                MachineState::Hold | MachineState::Wait | MachineState::Pause
            ),
        });

        let halt = s.halt_reason.map(|h| Halt {
            code: h.code(),
            reason: match h {
                hj_makera::status::HaltReason::Other(c) => format!("Unknown ({c})"),
                other => format!("{other:?}"),
            },
            recovery: match h.recovery() {
                hj_makera::status::HaltRecovery::Clear => HaltRecovery::Clear,
                hj_makera::status::HaltRecovery::Reset => HaltRecovery::Reset,
                hj_makera::status::HaltRecovery::PowerCycle => HaltRecovery::PowerCycle,
            },
        });
        // Work offsets only change when zeroing; in Hold/Wait the reported positions are planned
        // end points, which still have the same offset between them.
        let work_offset = std::array::from_fn(|i| round(m[i] - w[i]));

        self.state.status = Some(MachineStatus {
            state,
            halt,
            tool: s.tool.map(|t| ToolInfo {
                number: t.number,
                offset: t.offset,
                target: t.target.filter(|&n| n >= 0 && n != t.number),
            }),
            spindle_target_rpm: s.spindle.as_ref().map(|sp| sp.target_rpm),
            spindle_override_pct: s.spindle.as_ref().map(|sp| sp.override_pct),
            feed_requested: s.feed.map(|f| f.requested),
            feed_override_pct: s.feed.map(|f| f.override_pct),
            work_offset,
            job: s.progress.map(|p| JobProgress {
                lines: p.lines,
                percent: p.percent,
                elapsed_secs: p.elapsed_secs,
            }),
            absolute_mode: s.machine.map(|mi| mi.absolute_mode),
        });
        if let Some(info) = s.machine {
            let machine = self.state.machine.get_or_insert_default();
            if machine.model.is_none() {
                machine.model = Some(info.model_name().to_owned());
            }
        }
        self.last_status = Some(s);
    }

    fn disconnected(&mut self, error: Option<String>) {
        let was_up = self.session.take().is_some()
            || !matches!(self.state.connection, Connection::Disconnected { .. });
        if was_up && let Some(e) = &error {
            self.note(ConsoleKind::Error, e.clone());
        }
        self.state.connection = Connection::Disconnected { error };
        self.state.status = None;
        self.state.machine = None;
        self.last_status = None;
    }

    // ---- requests ----

    fn machine_state(&self) -> Result<MachineState, CoreError> {
        if self.session.is_none() {
            return Err(CoreError::NotConnected);
        }
        self.state
            .status
            .as_ref()
            .map(|s| s.state)
            .ok_or_else(|| CoreError::Rejected("waiting for the first status report".into()))
    }

    fn job_running(&self) -> bool {
        self.state.status.as_ref().is_some_and(|s| s.job.is_some())
    }

    fn require(&self, allowed: &[MachineState], what: &str) -> Result<MachineState, CoreError> {
        let state = self.machine_state()?;
        if !allowed.contains(&state) {
            return Err(CoreError::Rejected(format!(
                "can't {what} while the machine is {state:?}"
            )));
        }
        if self.job_running() {
            return Err(CoreError::Rejected(format!(
                "can't {what} while a job is running"
            )));
        }
        Ok(state)
    }

    async fn handle(&mut self, request: Request) -> Result<(), CoreError> {
        match request {
            Request::Connect { target } => self.connect(target).await,
            Request::Disconnect => {
                if self.session.is_some() {
                    self.note(ConsoleKind::Info, "disconnected".into());
                }
                self.disconnected(None);
                Ok(())
            }
            Request::Halt => {
                self.machine_state().or_else(|e| match e {
                    // Halting must work even before the first status report.
                    CoreError::Rejected(_) => Ok(MachineState::Unknown),
                    e => Err(e),
                })?;
                self.send_raw(Realtime::Halt.frame()).await?;
                self.note(ConsoleKind::Sent, "halt (Ctrl-X)".into());
                Ok(())
            }
            Request::Reset => {
                if self.session.is_none() {
                    return Err(CoreError::NotConnected);
                }
                self.send_line("reset").await
            }
            Request::Unlock => {
                self.machine_state()?;
                self.send_line("$X").await
            }
            Request::Home => {
                self.require(&[MachineState::Idle, MachineState::Alarm], "home")?;
                self.send_line("$H").await
            }
            Request::SetWork { x, y, z, a } => {
                self.require(&[MachineState::Idle], "set work coordinates")?;
                let words: Vec<String> = ['X', 'Y', 'Z', 'A']
                    .into_iter()
                    .zip([x, y, z, a])
                    .filter_map(|(axis, v)| v.map(|v| (axis, v)))
                    .map(|(axis, v)| {
                        if v.is_finite() {
                            Ok(format!("{axis}{v:.4}"))
                        } else {
                            Err(CoreError::Rejected(format!("invalid {axis} value")))
                        }
                    })
                    .collect::<Result<_, _>>()?;
                if words.is_empty() {
                    return Err(CoreError::Rejected("no axes to set".into()));
                }
                self.send_line(&format!("G10 L20 P0 {}", words.join(" ")))
                    .await
            }
            Request::ShrinkA => {
                self.require(&[MachineState::Idle], "shrink A")?;
                self.send_line("G92.4 A0 S0").await
            }
            Request::MoveTo { x, y, speed } => {
                self.require(&[MachineState::Idle], "move")?;
                let pos = self
                    .last_status
                    .as_ref()
                    .map(|s| s.machine_pos.0)
                    .ok_or_else(|| CoreError::Rejected("position unknown".into()))?;
                let delta = |target: Option<f64>, i: usize| -> Result<Option<f64>, CoreError> {
                    match target {
                        Some(t) if !t.is_finite() => {
                            Err(CoreError::Rejected("invalid target".into()))
                        }
                        Some(t) => Ok(Some(t - pos[i])),
                        None => Ok(None),
                    }
                };
                self.jog([delta(x, 0)?, delta(y, 1)?, None, None], speed)
                    .await
            }
            Request::Jog { x, y, z, a, speed } => self.jog([x, y, z, a], speed).await,
            Request::StopJog => {
                self.require(&[MachineState::Idle, MachineState::Run], "stop jogging")?;
                self.send_raw(Realtime::Halt.frame()).await?;
                self.note(ConsoleKind::Sent, "halt (Ctrl-X) to stop jog".into());
                // Give the firmware a moment to process the halt before clearing it.
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.send_line("$X").await
            }
            Request::Mdi { line } => {
                let line = line.trim();
                if line.is_empty() {
                    return Ok(());
                }
                self.machine_state()?;
                if self.job_running() {
                    return Err(CoreError::Rejected(
                        "can't send commands while a job is running".into(),
                    ));
                }
                self.send_line(line).await
            }
        }
    }

    async fn connect(&mut self, target: ConnectTarget) -> Result<(), CoreError> {
        if self.session.is_some() {
            self.disconnected(None);
        }
        let label = match &target {
            ConnectTarget::Tcp { host, port } => {
                format!("{host}:{}", port.unwrap_or(DEFAULT_TCP_PORT))
            }
            ConnectTarget::Usb => "USB".into(),
            ConnectTarget::Simulator => "simulator".into(),
        };
        self.state.connection = Connection::Connecting {
            target: label.clone(),
        };
        self.flush();

        let result = match &target {
            ConnectTarget::Tcp { host, port } => {
                let port = port.unwrap_or(DEFAULT_TCP_PORT);
                match tokio::net::lookup_host((host.as_str(), port)).await {
                    Ok(mut addrs) => match addrs.next() {
                        Some(addr) => transport::connect_tcp(addr)
                            .await
                            .map(|l| (l, Transport::Wifi)),
                        None => Err(std::io::Error::other(format!("can't resolve {host}"))),
                    },
                    Err(e) => Err(e),
                }
            }
            ConnectTarget::Usb => transport::usb::open(None)
                .await
                .map(|l| (l, Transport::Usb)),
            ConnectTarget::Simulator => {
                let addr = match self.simulator {
                    Some(addr) => Ok(addr),
                    None => hj_sim::spawn(([127, 0, 0, 1], 0).into())
                        .await
                        .map(|sim| sim.addr),
                };
                match addr {
                    Ok(addr) => {
                        self.simulator = Some(addr);
                        transport::connect_tcp(addr)
                            .await
                            .map(|l| (l, Transport::Simulator))
                    }
                    Err(e) => Err(e),
                }
            }
        };

        let (link, transport) = match result {
            Ok(ok) => ok,
            Err(e) => {
                let msg = format!("couldn't connect to {label}: {e}");
                self.state.connection = Connection::Disconnected {
                    error: Some(msg.clone()),
                };
                return Err(CoreError::Failed(msg));
            }
        };
        let description = link.description.clone();
        self.session = Some(Session {
            link: Link::start(link),
        });
        self.state.connection = Connection::Connected {
            target: label,
            transport,
        };
        self.state.machine = Some(MachineInfo::default());
        self.note(ConsoleKind::Info, format!("connected: {description}"));
        self.send_raw(Realtime::Status.frame()).await?;
        self.send_line("version").await?;
        self.send_line("model").await?;
        for key in LIMIT_KEYS {
            self.send_line(&format!("config-get sd {key}")).await?;
        }
        Ok(())
    }

    async fn jog(&mut self, delta: [Option<f64>; 4], speed: f64) -> Result<(), CoreError> {
        let state = self.require(&[MachineState::Idle, MachineState::Run], "jog")?;
        if !(speed > 0.0 && speed <= 1.0) {
            return Err(CoreError::Rejected(format!(
                "jog speed {speed} must be in (0, 1]"
            )));
        }
        let mut delta = delta.map(|d| d.unwrap_or(0.0));
        // Keep jogs inside the soft limits. Only possible when idle: queued jogs (state Run) end
        // somewhere we can't see.
        if state == MachineState::Idle
            && let Some(s) = &self.last_status
        {
            let travel = self
                .state
                .machine
                .as_ref()
                .map(|m| m.travel.clone())
                .unwrap_or_default();
            for (i, d) in delta.iter_mut().take(3).enumerate() {
                let pos = s.machine_pos.0[i];
                let max = travel.max[i] - LIMIT_MARGIN;
                if pos + *d > max {
                    // Go as far as the limit, but never backwards if already past it.
                    *d = (max - pos).max(0.0);
                }
                if let Some(min) = travel.min[i].map(|m| m + LIMIT_MARGIN)
                    && pos + *d < min
                {
                    *d = (min - pos).min(0.0);
                }
            }
        }
        if delta.iter().all(|d| d.abs() < 1e-4) {
            return Err(CoreError::Rejected("at the axis limit".into()));
        }
        let words: Vec<String> = ["X", "Y", "Z", "A"]
            .iter()
            .zip(delta)
            .filter(|(_, d)| d.abs() >= 1e-4)
            .map(|(a, d)| format!("{a}{d:.4}"))
            .collect();
        self.send_line(&format!("$J {} F{speed:.3}", words.join(" ")))
            .await
    }
}
