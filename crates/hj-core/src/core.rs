//! The core actor: owns [`AppState`] and the machine connection, gates and executes requests, and
//! publishes changes through a [`Sink`].

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hj_makera::command::{self, Realtime, escape_arg};
use hj_makera::connection::{Connection as Link, Event};
use hj_makera::discovery::{self, Announcement, DEFAULT_TCP_PORT};
use hj_makera::frame::{Frame, FrameType};
use hj_makera::message::Message;
use hj_makera::sd::Listing;
use hj_makera::status::{Diagnose, Status};
use hj_makera::transfer::{self, Download, Downloaded, TransferError, Upload};
use hj_makera::transport;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use ts_rs::TS;

use crate::probe::{self, Update};
use crate::settings;
use crate::state::*;
use crate::sync::{self, Patch};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DISCOVERY_TTL: Duration = Duration::from_secs(6);
/// `diagnose` (outputs) every this many status polls.
const DIAGNOSE_EVERY: u32 = 4;
/// The machine's configuration, read for the settings.
const CONFIG_PATH: &str = "/sd/config.txt";
const CONSOLE_HISTORY: usize = 2000;
/// How far inside the soft limits jogs stop. Targeting a limit exactly can trip it through
/// rounding (seen on hardware), which halts the machine.
const LIMIT_MARGIN: f64 = 0.01;
/// Jog max rates on the Z1 in mm/s (°/s for A), as measured: `$J … F1` runs at these.
const JOG_MAX_RATE: [f64; 4] = [20.0, 20.0, 10.0, 30.0];
/// A held jog queues this much motion ahead, topped up every [`HOLD_TICK`]: enough to ride out
/// network jitter without the planner running dry, little enough to stop soon after release.
const HOLD_LEAD: Duration = Duration::from_millis(300);
const HOLD_TICK: Duration = Duration::from_millis(100);
/// A held jog stops when not renewed for this long (the window lost a key-up, or went away).
const HOLD_TIMEOUT: Duration = Duration::from_millis(500);
/// Soft-limit config keys read after connecting, per axis.
const LIMIT_KEYS: [&str; 3] = [
    "soft_endstop.x_min",
    "soft_endstop.y_min",
    "soft_endstop.z_min",
];
/// The machine cancels a transfer after ~9 s without traffic; give up a little later.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(12);
const FILE_OP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long after `play` the job may take to show up in the status before we stop waiting.
const JOB_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Console line marking the end of a tool change.
const DONE_ATC: &str = "Done ATC";
/// Console line marking the end of an `abort`.
const ABORTED: &str = "Aborted playing or paused file.";

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

type Reply = oneshot::Sender<Result<(), CoreError>>;
/// A downloaded file, or `None` if it matched the copy we have.
type DownloadReply = oneshot::Sender<Result<Option<Vec<u8>>, CoreError>>;

enum Envelope {
    Request {
        request: Request,
        reply: Reply,
    },
    Download {
        path: String,
        have_md5: Option<String>,
        reply: DownloadReply,
    },
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
        let (routine_tx, routine_rx) = mpsc::unbounded_channel();
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
            reply: None,
            upload: None,
            download: None,
            file_op: None,
            outgoing: Vec::new(),
            job_started: None,
            job_seen: false,
            routine: None,
            routine_lines: None,
            back_off: None,
            routine_tx,
            held: None,
            polls: 0,
            batch: VecDeque::new(),
            coasting: None,
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
            actor.run(rx, disc_rx, routine_rx).await;
        });
        Self { tx, shared }
    }

    pub async fn request(&self, request: Request) -> Result<(), CoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Envelope::Request { request, reply })
            .await
            .map_err(|_| CoreError::Stopped)?;
        rx.await.map_err(|_| CoreError::Stopped)?
    }

    /// Download a file from the machine's SD card. If `have` is given and the machine's copy is
    /// identical, nothing is transferred and the result is `None`.
    pub async fn download(
        &self,
        path: String,
        have: Option<&str>,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Envelope::Download {
                path,
                have_md5: have.map(|t| transfer::md5_hex(t.as_bytes())),
                reply,
            })
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
    transport: Transport,
}

struct PendingUpload {
    upload: Upload,
    reply: Option<Reply>,
    last_activity: Instant,
}

struct PendingDownload {
    download: Download,
    reply: Option<DownloadReply>,
    last_activity: Instant,
    /// The configuration, for the settings: parsed into the state instead of replied.
    settings: bool,
}

/// SD card operations answer in `LOAD_*` frames, which don't say what they answer, so only one
/// runs at a time.
enum FileOp {
    List {
        dir: String,
        listing: Listing,
        started: Instant,
    },
    Delete {
        reply: Option<Reply>,
        started: Instant,
    },
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
    /// Reply for the request being handled. Requests that finish later take it.
    reply: Option<Reply>,
    upload: Option<PendingUpload>,
    download: Option<PendingDownload>,
    file_op: Option<FileOp>,
    /// Frames queued while handling an event, with what to log for them; sent afterwards.
    outgoing: Vec<(Frame, Option<String>)>,
    job_started: Option<Instant>,
    /// The job we started has shown up in a status report.
    job_seen: bool,
    routine: Option<ActiveRoutine>,
    /// Console line sink for the routine about to be spawned.
    routine_lines: Option<mpsc::UnboundedSender<String>>,
    /// The way out of the last probe move, should the routine stop there.
    back_off: Option<[f64; 3]>,
    /// For routines to report back; the receiver is in [`Actor::run`].
    routine_tx: mpsc::UnboundedSender<Update>,
    held: Option<HeldJog>,
    polls: u32,
    /// Lines of the running batch still to send; the one sent last is waiting for its `ok`.
    batch: VecDeque<String>,
    /// Where the jogs we queued end, while the machine may still be running them: the next jog
    /// (held or a step) carries on from there instead of waiting for the machine to stop.
    coasting: Option<Coast>,
}

struct Coast {
    planned: [f64; 4],
    /// When the queued moves should be done, roughly.
    until: Instant,
}

/// A continuous jog: short `$J` moves streamed back to back while the user holds a key.
struct HeldJog {
    /// Direction per axis (X, Y, Z, A), each −1, 0 or 1.
    dir: [f64; 4],
    speed: f64,
    /// Where the queued moves end, in machine coordinates.
    planned: [f64; 4],
    /// How far ahead motion is queued.
    queued_until: Instant,
    renewed: Instant,
    at_limit: bool,
}

/// A probing routine running as its own task.
struct ActiveRoutine {
    /// Console lines go to the routine while it runs.
    lines: mpsc::UnboundedSender<String>,
    task: tokio::task::AbortHandle,
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
        mut routine_updates: mpsc::UnboundedReceiver<Update>,
    ) {
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut hold = tokio::time::interval(HOLD_TICK);
        hold.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.flush();
        loop {
            tokio::select! {
                envelope = requests.recv() => {
                    let (request, reply) = match envelope {
                        None => return,
                        Some(Envelope::Request { request, reply }) => (request, reply),
                        Some(Envelope::Download { path, have_md5, reply }) => {
                            self.start_download(path, have_md5, reply);
                            let _ = self.send_outgoing().await;
                            self.flush();
                            continue;
                        }
                    };
                    self.reply = Some(reply);
                    let mut result = self.handle(request).await;
                    if result.is_ok() {
                        result = self.send_outgoing().await;
                    }
                    if let Err(e) = &result {
                        self.note(ConsoleKind::Error, e.to_string());
                    }
                    // Publish first, so the state already shows what the request did when its
                    // caller hears back.
                    self.flush();
                    if let Some(reply) = self.reply.take() {
                        let _ = reply.send(result);
                    }
                }
                event = next_event(&mut self.session) => {
                    match event {
                        Some(event) => self.on_event(event),
                        None => self.disconnected(Some("connection closed".into())),
                    }
                    let _ = self.send_outgoing().await;
                }
                _ = poll.tick() => {
                    self.poll().await;
                }
                _ = hold.tick(), if self.held.is_some() => self.hold_tick().await,
                Some(a) = discovery.recv() => self.on_announcement(a),
                Some(update) = routine_updates.recv() => self.on_routine(update).await,
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

    /// Queue a line for [`Self::send_outgoing`], from code that can't await.
    fn queue_line(&mut self, line: String) {
        self.outgoing.push((command::command(&line), Some(line)));
    }

    async fn send_outgoing(&mut self) -> Result<(), CoreError> {
        for (frame, log) in std::mem::take(&mut self.outgoing) {
            self.send_raw(frame).await?;
            if let Some(line) = log {
                self.note(ConsoleKind::Sent, line);
            }
        }
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
            self.polls = self.polls.wrapping_add(1);
            if self.polls.is_multiple_of(DIAGNOSE_EVERY) && !self.transferring() {
                let _ = self.send_raw(command::command("diagnose")).await;
            }
        }
        if self
            .upload
            .as_ref()
            .is_some_and(|u| now.duration_since(u.last_activity) > TRANSFER_TIMEOUT)
        {
            self.outgoing
                .push((Frame::new(FrameType::FILE_CANCEL, []), None));
            self.upload_finished(Err(TransferError::Timeout(TRANSFER_TIMEOUT).to_string()));
            let _ = self.send_outgoing().await;
        }
        if let Some(d) = &mut self.download
            && now.duration_since(d.last_activity) > TRANSFER_TIMEOUT
        {
            // If we had everything and only the machine's confirmation is missing, keep it.
            match d.download.take_outcome() {
                Some(outcome) => self.download_finished(Ok(outcome)),
                None => {
                    self.outgoing
                        .push((Frame::new(FrameType::FILE_CANCEL, []), None));
                    self.download_finished(Err(
                        TransferError::Timeout(TRANSFER_TIMEOUT).to_string()
                    ));
                    let _ = self.send_outgoing().await;
                }
            }
        }
        let started = match &self.file_op {
            Some(FileOp::List { started, .. } | FileOp::Delete { started, .. }) => Some(*started),
            None => None,
        };
        if started.is_some_and(|t| now.duration_since(t) > FILE_OP_TIMEOUT) {
            self.file_op_finished(Err("no response from the machine".into()));
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
            Event::Message(Message::Diagnose(d)) => self.on_diagnose(&d),
            Event::Message(Message::Malformed { line, error }) => {
                self.note(ConsoleKind::Error, format!("{error}: {line}"))
            }
            Event::Frame(frame)
                if (FrameType::FILE_START.0..=FrameType::FILE_RETRY.0).contains(&frame.kind.0) =>
            {
                self.on_transfer_frame(&frame)
            }
            Event::Message(Message::LoadInfo(payload)) => {
                if let Some(FileOp::List { listing, .. }) = &mut self.file_op {
                    listing.feed(&payload);
                }
            }
            Event::Message(Message::LoadFinish) => self.file_op_finished(Ok(())),
            Event::Message(Message::LoadError(payload)) => {
                self.file_op_finished(Err(String::from_utf8_lossy(&payload).trim().to_owned()))
            }
            Event::Closed(err) => {
                self.disconnected(Some(match err {
                    Some(e) => format!("connection lost: {e}"),
                    None => "connection closed by machine".into(),
                }));
            }
            // Raw frames and the rest aren't used yet.
            _ => {}
        }
    }

    fn on_line(&mut self, line: String) {
        if self.state.batch.is_some() {
            self.batch_reply(&line);
        }
        if let Some(r) = &self.routine {
            let _ = r.lines.send(line.clone());
        }
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
        if line == DONE_ATC {
            self.state.tool_change = None;
        }
        if line == ABORTED {
            // `abort` leaves the job's modal state behind; a job that used G91 would leave the
            // machine in incremental mode.
            if let Some(job) = &mut self.state.job {
                job.aborting = false;
            }
            self.queue_line("G90".into());
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
            laser_power: s.laser.filter(|l| l.mode).map(|l| l.power),
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
            laser: s.laser.map(|l| LaserStatus {
                mode: l.mode,
                on: l.on,
                testing: l.testing,
                scale: l.scale,
            }),
        });
        // Tool changes show as the `Tool` state with the target in `T:`, also when a program or
        // the console started them. The state reads `Idle` for moments during the routine, so
        // only `Done ATC` (or a halt) ends it.
        if state == MachineState::Tool {
            let target = s.tool.as_ref().and_then(|t| t.target).filter(|&t| t >= 0);
            match &mut self.state.tool_change {
                Some(tc) if tc.phase != ToolChangePhase::Measuring => {
                    tc.phase = ToolChangePhase::Waiting;
                    tc.target = target.unwrap_or(tc.target);
                }
                Some(_) => {}
                None => {
                    self.state.tool_change = target.map(|target| ToolChange {
                        target,
                        phase: ToolChangePhase::Waiting,
                    });
                }
            }
        } else if state == MachineState::Alarm {
            self.state.tool_change = None;
        }
        if s.progress.is_some() {
            self.job_seen = true;
        } else if self.state.job.is_some()
            && !matches!(state, MachineState::Pause | MachineState::Wait)
            && (self.job_seen
                || self
                    .job_started
                    .is_none_or(|t| t.elapsed() > JOB_START_TIMEOUT))
        {
            self.state.job = None;
            self.job_seen = false;
            self.job_started = None;
        }
        if let Some(info) = s.machine {
            let machine = self.state.machine.get_or_insert_default();
            if machine.model.is_none() {
                machine.model = Some(info.model_name().to_owned());
            }
        }
        // Done coasting once it's stopped well after the queued jogs should have.
        if state == MachineState::Alarm
            || (state == MachineState::Idle
                && self
                    .coasting
                    .as_ref()
                    .is_some_and(|c| Instant::now() > c.until + Duration::from_millis(500)))
        {
            self.coasting = None;
        }
        self.last_status = Some(s);
    }

    fn on_diagnose(&mut self, d: &Diagnose) {
        let get = |k: &str| d.0.get(k).filter(|v| !v.is_empty());
        let pwm = |v: &Vec<i64>| Pwm {
            on: v[0] != 0,
            power: v.get(1).copied().unwrap_or(0) as f64,
        };
        // G is light, beep, external input, external output state and power (the last four on
        // the Carvera Air and Z1 only).
        let g = get("G");
        self.state.outputs = Some(Outputs {
            light: g.map(|v| v[0] != 0),
            // V is the vacuum on machines with an ATC, the power fan otherwise; both switch with
            // M801/M802.
            power_fan: get("V").map(pwm),
            spindle_fan: get("F").map(pwm),
            ext_out: g.filter(|v| v.len() >= 5).map(|v| Pwm {
                on: v[3] != 0,
                power: v[4] as f64,
            }),
            air: get("R").map(|v| v[0] != 0),
        });
    }

    fn settings_read(&mut self, result: Result<Option<Vec<u8>>, CoreError>) {
        let s = &mut self.state.settings;
        s.loading = false;
        match result {
            Ok(Some(data)) => {
                let entries = settings::parse_config(&String::from_utf8_lossy(&data));
                if entries.is_empty() {
                    s.error = Some(format!("{CONFIG_PATH} has no settings"));
                } else {
                    s.entries = Some(settings::arrange(entries));
                }
            }
            Ok(None) => {}
            Err(e) => s.error = Some(e.to_string()),
        }
    }

    /// A console line while a batch runs: `ok` sends the next line, an error ends it.
    fn batch_reply(&mut self, line: &str) {
        let lower = line.to_ascii_lowercase();
        if line == "ok" {
            let Some(b) = &mut self.state.batch else {
                return;
            };
            b.done += 1;
            match self.batch.pop_front() {
                Some(next) => self.queue_line(next),
                None => self.state.batch = None,
            }
        } else if lower.starts_with("error") || lower.starts_with("alarm") || line == "!!" {
            self.end_batch(Some("the machine reported an error"));
        }
    }

    /// Stop sending; `why` is noted when lines were left unsent.
    fn end_batch(&mut self, why: Option<&str>) {
        let Some(b) = self.state.batch.take() else {
            return;
        };
        let left = b.total - b.done - 1;
        self.batch.clear();
        if let Some(why) = why
            && left > 0
        {
            self.note(
                ConsoleKind::Error,
                format!(
                    "{why}: {left} line{} not sent",
                    if left == 1 { "" } else { "s" }
                ),
            );
        }
    }

    fn current_tool(&self) -> Option<i32> {
        self.state.status.as_ref()?.tool.as_ref().map(|t| t.number)
    }

    fn laser(&self) -> Option<&LaserStatus> {
        self.state.status.as_ref().and_then(|s| s.laser.as_ref())
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
        self.state.files = Files::default();
        self.state.job = None;
        self.state.tool_change = None;
        self.state.outputs = None;
        self.state.batch = None;
        self.batch.clear();
        self.state.settings = Settings::default();
        self.stop_routine(Some("disconnected".into()));
        self.held = None;
        self.coasting = None;
        self.last_status = None;
        self.outgoing.clear();
        if self.upload.is_some() {
            self.upload_finished(Err("disconnected".into()));
        }
        if self.download.is_some() {
            self.download_finished(Err("disconnected".into()));
        }
        if self.file_op.is_some() {
            self.file_op_finished(Err("disconnected".into()));
        }
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
        if self.state.tool_change.is_some() {
            return Err(CoreError::Rejected(format!(
                "can't {what} during a tool change"
            )));
        }
        if self.routine.is_some() {
            return Err(CoreError::Rejected(format!("can't {what} while probing")));
        }
        if self.held.is_some() {
            return Err(CoreError::Rejected(format!("can't {what} while jogging")));
        }
        if self.state.batch.is_some() {
            return Err(CoreError::Rejected(format!(
                "can't {what} while console commands are being sent"
            )));
        }
        Ok(state)
    }

    async fn handle(&mut self, request: Request) -> Result<(), CoreError> {
        // Anything but another jog might move the machine some other way (or stop it).
        if !matches!(
            request,
            Request::Jog { .. }
                | Request::JogHold { .. }
                | Request::JogRelease
                | Request::MoveTo { .. }
        ) {
            self.coasting = None;
        }
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
                self.held = None;
                self.end_batch(Some("halted"));
                self.send_raw(Realtime::Halt.frame()).await?;
                self.note(ConsoleKind::Sent, "halt (Ctrl-X)".into());
                Ok(())
            }
            Request::Reset => {
                self.held = None;
                self.end_batch(Some("reset"));
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
            Request::SetOrigin { x, y, z } => {
                self.require(&[MachineState::Idle], "set the work origin")?;
                let pos = self
                    .last_status
                    .as_ref()
                    .map(|s| s.machine_pos.0)
                    .ok_or_else(|| CoreError::Rejected("position unknown".into()))?;
                // G10 L20 sets the work position *here*; to put zero at `target`, make here read
                // as the distance from it.
                let mut words = vec![];
                for (i, (axis, target)) in [('X', x), ('Y', y), ('Z', z)].into_iter().enumerate() {
                    let Some(t) = target else { continue };
                    if !t.is_finite() {
                        return Err(CoreError::Rejected(format!("invalid {axis} value")));
                    }
                    words.push(format!("{axis}{:.4}", pos[i] - t));
                }
                if words.is_empty() {
                    return Err(CoreError::Rejected("no axes to set".into()));
                }
                self.send_line(&format!("G10 L20 P0 {}", words.join(" ")))
                    .await
            }
            Request::MoveTo { x, y, speed } => {
                let pos = self.jog_start("move")?;
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
            Request::JogHold { x, y, z, a, speed } => self.hold([x, y, z, a], speed).await,
            Request::JogRelease => {
                if let Some(h) = self.held.take() {
                    self.coast(h);
                    self.note(ConsoleKind::Info, "jog released".into());
                }
                Ok(())
            }
            Request::StopJog => {
                self.held = None;
                self.require(&[MachineState::Idle, MachineState::Run], "stop jogging")?;
                self.send_raw(Realtime::Halt.frame()).await?;
                self.note(ConsoleKind::Sent, "halt (Ctrl-X) to stop jog".into());
                // Give the firmware a moment to process the halt before clearing it.
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.send_line("$X").await
            }
            Request::ListFiles { dir } => {
                self.machine_state()?;
                let dir = dir.unwrap_or_else(|| self.state.files.dir.clone());
                self.list(dir)
            }
            Request::DeleteFile { path } => {
                self.machine_state()?;
                check_sd_path(&path)?;
                if self.state.job.as_ref().is_some_and(|j| j.path == path) {
                    return Err(CoreError::Rejected("can't delete the running job".into()));
                }
                if self.file_op.is_some() {
                    return Err(CoreError::Rejected("the SD card is busy".into()));
                }
                self.queue_line(format!("rm {}", escape_arg(&path)));
                self.file_op = Some(FileOp::Delete {
                    reply: self.reply.take(),
                    started: Instant::now(),
                });
                self.state.files.busy = true;
                Ok(())
            }
            Request::Upload { name, text, run } => self.start_upload(name, text, run),
            Request::Play { path } => {
                check_sd_path(&path)?;
                self.play(path)
            }
            Request::Pause => {
                self.require_job()?;
                if self.machine_state()? != MachineState::Run {
                    return Err(CoreError::Rejected("the job isn't running".into()));
                }
                self.send_line("suspend").await
            }
            Request::Resume => {
                if self.machine_state()? != MachineState::Pause {
                    return Err(CoreError::Rejected("the job isn't paused".into()));
                }
                self.send_line("resume").await
            }
            Request::Abort => {
                self.require_job()?;
                if let Some(job) = &mut self.state.job {
                    job.aborting = true;
                }
                self.send_line("abort").await
            }
            Request::Probe { routine } => self.start_routine(routine),
            Request::CancelProbe => {
                if self.routine.is_none() {
                    return Err(CoreError::Rejected("not probing".into()));
                }
                self.stop_routine(Some("cancelled".into()));
                self.send_raw(Realtime::Halt.frame()).await?;
                self.note(ConsoleKind::Sent, "halt (Ctrl-X) to stop probing".into());
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.send_line("$X").await
            }
            Request::RecoverProbe => {
                if self.routine.is_some() {
                    return Err(CoreError::Rejected("a routine is running".into()));
                }
                let halted = self.machine_state()? == MachineState::Alarm;
                match self.back_off.take() {
                    Some(dir) => {
                        let io = self.routine_io();
                        self.state.probe.running = Some(RunningProbe {
                            label: "Recovering".into(),
                            step: "Backing the probe off".into(),
                        });
                        self.spawn_routine(probe::back_off(halted, dir, io));
                    }
                    None => {
                        if halted {
                            self.send_line("$X").await?;
                        }
                        self.state.probe.error = None;
                    }
                }
                self.state.probe.can_back_off = false;
                Ok(())
            }
            Request::DismissProbe => {
                if self.routine.is_none() {
                    self.state.probe.error = None;
                    self.state.probe.result = None;
                    self.state.probe.can_back_off = false;
                    self.back_off = None;
                }
                Ok(())
            }
            Request::SetProbeSettings { settings } => {
                if self.routine.is_some() {
                    return Err(CoreError::Rejected(
                        "can't change settings while probing".into(),
                    ));
                }
                let s = &settings;
                let positive = [
                    s.ball_diameter,
                    s.fast_feed,
                    s.slow_feed,
                    s.travel_feed,
                    s.z_search,
                ];
                if !positive.iter().all(|v| v.is_finite() && *v > 0.0)
                    || !(s.clearance.is_finite() && s.clearance >= 0.0)
                {
                    return Err(CoreError::Rejected("invalid probe settings".into()));
                }
                self.state.probe.settings = settings;
                Ok(())
            }
            Request::ChangeTool { tool } => {
                self.require(&[MachineState::Idle], "change tools")?;
                let laser = tool == LASER_TOOL;
                if !(0..=999).contains(&tool) && tool != PROBE_TOOL && !laser {
                    return Err(CoreError::Rejected(format!("invalid tool number {tool}")));
                }
                let laser_mode = self.laser().map(|l| l.mode);
                if laser && laser_mode.is_none() {
                    return Err(CoreError::Rejected(
                        "this machine doesn't report a laser".into(),
                    ));
                }
                let laser_mode = laser_mode.unwrap_or(false);
                if self.current_tool() == Some(tool) {
                    // In, but not in laser mode (left by hand): just switch the mode back on.
                    if laser && !laser_mode {
                        return self.send_line("M321").await;
                    }
                    // The firmware does nothing then, so the change would never end.
                    return Err(CoreError::Rejected(format!("T{tool} is already in")));
                }
                if laser && !laser_mode {
                    // M321 does the tool change to the laser itself (M6 T8888).
                    self.send_line("M321").await?;
                } else {
                    // Out of laser mode first, which also restores the Z offset it changed.
                    if !laser && laser_mode {
                        self.send_line("M322").await?;
                    }
                    self.send_line(&format!("M6 T{tool}")).await?;
                }
                self.state.tool_change = Some(ToolChange {
                    target: tool,
                    phase: ToolChangePhase::Moving,
                });
                Ok(())
            }
            Request::ConfirmToolChange => {
                if self.machine_state()? != MachineState::Tool {
                    return Err(CoreError::Rejected(
                        "the machine isn't waiting for a tool change".into(),
                    ));
                }
                self.send_line("M490.2").await?;
                if let Some(tc) = &mut self.state.tool_change {
                    tc.phase = ToolChangePhase::Measuring;
                }
                Ok(())
            }
            Request::SetOverride {
                feed,
                spindle,
                laser,
            } => {
                self.machine_state()?;
                for (value, code, range) in [
                    (feed, "M220", 10.0..=300.0),
                    (spindle, "M223", 50.0..=200.0),
                    (laser, "M325", 10.0..=200.0),
                ] {
                    let Some(v) = value else { continue };
                    if !range.contains(&v) {
                        return Err(CoreError::Rejected(format!(
                            "{code} takes {}–{} %",
                            range.start(),
                            range.end()
                        )));
                    }
                    self.send_line(&format!("{code} S{}", v.round())).await?;
                }
                Ok(())
            }
            Request::SetOutput { output, on, power } => {
                self.machine_state()?;
                let power = power.unwrap_or(100.0);
                if !(1.0..=100.0).contains(&power) {
                    return Err(CoreError::Rejected("output power is 1–100 %".into()));
                }
                let p = power.round();
                let line = match (output, on) {
                    (Output::Light, true) => "M821".into(),
                    (Output::Light, false) => "M822".into(),
                    (Output::PowerFan, true) => format!("M801 S{p}"),
                    (Output::PowerFan, false) => "M802".into(),
                    (Output::SpindleFan, true) => format!("M811 S{p}"),
                    (Output::SpindleFan, false) => "M812".into(),
                    (Output::ExtOut, true) => format!("M851 S{p}"),
                    (Output::ExtOut, false) => "M852".into(),
                    (Output::Air, true) => "M7".into(),
                    (Output::Air, false) => "M9".into(),
                };
                self.send_line(&line).await?;
                // Show the change without waiting for the next diagnose.
                self.send_raw(command::command("diagnose")).await
            }
            Request::LaserTest { on } => {
                if on {
                    self.require(&[MachineState::Idle], "test fire")?;
                    if !self.laser().is_some_and(|l| l.mode) {
                        return Err(CoreError::Rejected("change to the laser first".into()));
                    }
                    self.send_line("M323").await
                } else {
                    // Always allowed: it only ever turns the beam off.
                    self.machine_state()?;
                    self.send_line("M324").await
                }
            }
            Request::LoadSettings => {
                // Downloaded like any file, as the official controller does: `config-get-all`
                // printed nothing on the Z1.
                self.machine_state()?;
                if self.state.settings.loading {
                    return Ok(());
                }
                if self.transferring() || self.job_running() {
                    return Err(CoreError::Rejected(
                        "can't read settings during a transfer or a job".into(),
                    ));
                }
                let (reply, _) = oneshot::channel();
                self.start_download(CONFIG_PATH.into(), None, reply);
                if let Some(d) = &mut self.download {
                    d.settings = true;
                }
                self.state.settings.loading = true;
                self.state.settings.error = None;
                Ok(())
            }
            Request::StopBatch => {
                self.end_batch(Some("stopped"));
                Ok(())
            }
            Request::Mdi { line } => {
                let lines: Vec<&str> = line
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !is_comment(l))
                    .collect();
                if lines.is_empty() {
                    return Ok(());
                }
                self.machine_state()?;
                if self.job_running() {
                    return Err(CoreError::Rejected(
                        "can't send commands while a job is running".into(),
                    ));
                }
                if self.state.batch.is_some() {
                    return Err(CoreError::Rejected(
                        "still sending the previous commands".into(),
                    ));
                }
                if let [line] = lines[..] {
                    return self.send_line(line).await;
                }
                if self.routine.is_some() || self.held.is_some() {
                    return Err(CoreError::Rejected(
                        "can't send several lines while probing or jogging".into(),
                    ));
                }
                if let Some(bad) = lines.iter().find(|l| !is_gcode(l)) {
                    return Err(CoreError::Rejected(format!(
                        "only G-code lines can be sent together (the machine doesn't acknowledge \
                         others); send {bad:?} on its own"
                    )));
                }
                let mut lines = lines.into_iter().map(str::to_owned);
                let first = lines.next().expect("several lines");
                self.batch = lines.collect();
                self.state.batch = Some(Batch {
                    done: 0,
                    total: self.batch.len() as u32 + 1,
                });
                self.send_line(&first).await
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
            transport,
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
        self.list(GCODE_DIR.into())?;
        self.send_outgoing().await
    }

    // ---- probing ----

    fn start_routine(&mut self, routine: probe::Routine) -> Result<(), CoreError> {
        self.require(&[MachineState::Idle], "probe")?;
        let depth_only = match self.current_tool() {
            Some(PROBE_TOOL) => false,
            Some(DEPTH_PROBE_TOOL) => true,
            _ => {
                return Err(CoreError::Rejected(
                    "put a probe in first (tool change to the 3D or depth probe)".into(),
                ));
            }
        };
        if depth_only && routine != probe::Routine::Surface {
            return Err(CoreError::Rejected(
                "the depth probe only measures Z; use the 3D probe for this".into(),
            ));
        }
        // Before 1.1.3, changing to the probe didn't switch it on (docs/research/probing.md).
        let firmware = self.state.machine.as_ref().and_then(|m| m.firmware.clone());
        let version: Vec<u32> = firmware
            .as_deref()
            .unwrap_or_default()
            .split('.')
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        if version < vec![1, 1, 3] {
            return Err(CoreError::Rejected(format!(
                "probing needs firmware 1.1.3 or newer (this machine has {})",
                firmware.as_deref().unwrap_or("an unknown version")
            )));
        }
        let sizes: &[f64] = match &routine {
            probe::Routine::Surface => &[],
            probe::Routine::Edge { search, .. } => &[*search],
            probe::Routine::Corner { depth, reach, .. } => &[*depth, *reach],
            probe::Routine::Bore { diameter, .. } => &[*diameter],
            probe::Routine::Boss {
                diameter, depth, ..
            } => &[*diameter, *depth],
        };
        if !sizes.iter().all(|v| v.is_finite() && *v > 0.0) {
            return Err(CoreError::Rejected(
                "probe distances must be positive".into(),
            ));
        }
        if self.last_status.is_none() {
            return Err(CoreError::Rejected("position unknown".into()));
        }
        let io = self.routine_io();
        self.note(ConsoleKind::Info, format!("probing: {}", routine.label()));
        self.state.probe.running = Some(RunningProbe {
            label: routine.label(),
            step: "Starting".into(),
        });
        self.state.probe.contacts.clear();
        self.state.probe.result = None;
        self.state.probe.error = None;
        self.state.probe.can_back_off = false;
        self.back_off = None;
        // The firmware's own Z probing switches the depth probe on around it.
        self.spawn_routine(probe::run(routine, io, depth_only));
        Ok(())
    }

    /// The machine side for a new routine task; [`Self::spawn_routine`] connects it.
    fn routine_io(&mut self) -> probe::Io {
        let here = self
            .last_status
            .as_ref()
            .map_or([0.0; 5], |s| s.machine_pos.0);
        let (lines_tx, lines) = mpsc::unbounded_channel();
        self.routine_lines = Some(lines_tx);
        probe::Io {
            updates: self.routine_tx.clone(),
            lines,
            here: [here[0], here[1], here[2]],
            settings: self.state.probe.settings.clone(),
        }
    }

    fn spawn_routine(&mut self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let lines = self.routine_lines.take().expect("routine_io first");
        let task = tokio::spawn(task).abort_handle();
        self.routine = Some(ActiveRoutine { lines, task });
    }

    fn stop_routine(&mut self, error: Option<String>) {
        if let Some(r) = self.routine.take() {
            r.task.abort();
            self.state.probe.running = None;
            self.state.probe.error = error;
            self.state.probe.can_back_off = self.back_off.is_some();
        }
    }

    async fn on_routine(&mut self, update: Update) {
        // Updates from a routine that was cancelled meanwhile are dropped.
        if self.routine.is_none() {
            return;
        }
        match update {
            Update::Send(line) => {
                if self.send_line(&line).await.is_err() {
                    self.stop_routine(Some("disconnected".into()));
                }
            }
            Update::Step(step) => {
                if let Some(r) = &mut self.state.probe.running {
                    r.step = step;
                }
            }
            Update::Contact(p) => self.state.probe.contacts.push(p),
            Update::BackOff(dir) => self.back_off = Some(dir),
            Update::Recovered(result) => {
                self.routine = None;
                self.state.probe.running = None;
                match result {
                    Ok(()) => {
                        self.note(ConsoleKind::Info, "probe backed off".into());
                        self.state.probe.error = None;
                    }
                    Err(e) => {
                        self.note(
                            ConsoleKind::Error,
                            format!("couldn't back the probe off: {e}"),
                        );
                        self.state.probe.error = Some(format!("couldn't back the probe off: {e}"));
                    }
                }
            }
            Update::Done(result) => {
                self.routine = None;
                self.state.probe.running = None;
                match result {
                    Ok(r) => {
                        let fmt = |l: &str, v: Option<f64>| {
                            v.map(|v| format!(" {l}{v:.3}")).unwrap_or_default()
                        };
                        self.note(
                            ConsoleKind::Info,
                            format!(
                                "probed {}:{}{}{}{}",
                                r.label.to_lowercase(),
                                fmt("X", r.x),
                                fmt("Y", r.y),
                                fmt("Z", r.z),
                                fmt("Ø", r.diameter)
                            ),
                        );
                        self.state.probe.result = Some(r);
                    }
                    Err(e) => {
                        self.note(ConsoleKind::Error, format!("probing stopped: {e}"));
                        self.state.probe.error = Some(e);
                        self.state.probe.can_back_off = self.back_off.is_some();
                    }
                }
            }
        }
    }

    fn require_job(&self) -> Result<(), CoreError> {
        self.machine_state()?;
        if !self.job_running() {
            return Err(CoreError::Rejected("no job is running".into()));
        }
        Ok(())
    }

    // ---- SD card and jobs ----

    fn list(&mut self, dir: String) -> Result<(), CoreError> {
        let dir = dir.trim_end_matches('/').to_owned();
        check_sd_path(&dir)?;
        if self.file_op.is_some() {
            return Err(CoreError::Rejected("the SD card is busy".into()));
        }
        if dir != self.state.files.dir {
            self.state.files.entries = None;
        }
        self.state.files.dir = dir.clone();
        self.state.files.busy = true;
        self.queue_line(format!("ls -s {}", escape_arg(&dir)));
        self.file_op = Some(FileOp::List {
            dir,
            listing: Listing::default(),
            started: Instant::now(),
        });
        Ok(())
    }

    fn file_op_finished(&mut self, result: Result<(), String>) {
        self.state.files.busy = false;
        match self.file_op.take() {
            Some(FileOp::List { dir, listing, .. }) => {
                if let Err(e) = &result {
                    self.note(ConsoleKind::Error, format!("listing {dir}: {e}"));
                }
                if dir == self.state.files.dir {
                    let mut entries: Vec<FileEntry> = listing
                        .finish()
                        .into_iter()
                        .filter(|e| !e.name.starts_with('.'))
                        .map(|e| FileEntry {
                            modified: e.modified.map(|[y, mo, d, h, mi, _]| {
                                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
                            }),
                            name: e.name,
                            dir: e.dir,
                            size: e.size,
                        })
                        .collect();
                    entries.sort_by(|a, b| {
                        b.dir
                            .cmp(&a.dir)
                            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    self.state.files.entries = Some(entries);
                }
            }
            Some(FileOp::Delete { reply, .. }) => {
                let result = result.map_err(CoreError::Failed);
                match &result {
                    Ok(()) => {
                        let dir = self.state.files.dir.clone();
                        let _ = self.list(dir);
                    }
                    Err(e) => self.note(ConsoleKind::Error, e.to_string()),
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
            None => {}
        }
    }

    fn start_upload(&mut self, name: String, text: String, run: bool) -> Result<(), CoreError> {
        let transport = self
            .session
            .as_ref()
            .map(|s| s.transport)
            .ok_or(CoreError::NotConnected)?;
        self.machine_state()?;
        let name = name.trim();
        if name.is_empty()
            || name.starts_with('.')
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
        {
            return Err(CoreError::Rejected(format!("invalid file name {name:?}")));
        }
        if self.transferring() {
            return Err(CoreError::Rejected(
                "a transfer is already in progress".into(),
            ));
        }
        if self.job_running() {
            return Err(CoreError::Rejected(
                "can't upload while a job is running".into(),
            ));
        }
        if run {
            self.require(&[MachineState::Idle], "start a job")?;
        }
        let path = format!("{GCODE_DIR}/{name}");
        let block_size = match transport {
            Transport::Usb => transfer::USB_BLOCK_SIZE,
            Transport::Wifi | Transport::Simulator => transfer::TCP_BLOCK_SIZE,
        };
        let mut upload = Upload::new(&path, text.into_bytes(), block_size)
            .map_err(|e| CoreError::Rejected(e.to_string()))?;
        for frame in upload.start() {
            self.outgoing.push((frame, None));
        }
        self.note(
            ConsoleKind::Info,
            format!("uploading {path} ({} bytes)", upload.len()),
        );
        self.state.transfer = Some(Transfer {
            direction: TransferDirection::Upload,
            path,
            done: 0,
            total: upload.len() as u64,
            run,
        });
        self.upload = Some(PendingUpload {
            upload,
            reply: self.reply.take(),
            last_activity: Instant::now(),
        });
        Ok(())
    }

    fn transferring(&self) -> bool {
        self.upload.is_some() || self.download.is_some()
    }

    /// Publish transfer progress in whole-percent steps: USB transfers go in 128-byte blocks.
    fn transfer_progress(&mut self, done: u64, total: Option<u64>) {
        if let Some(t) = &mut self.state.transfer {
            if let Some(total) = total {
                t.total = total.max(done);
            }
            let pct = |v: u64| v * 100 / t.total.max(1);
            if pct(done) != pct(t.done) {
                t.done = done;
            }
        }
    }

    fn start_download(&mut self, path: String, have_md5: Option<String>, reply: DownloadReply) {
        let check = || -> Result<(), CoreError> {
            self.machine_state()?;
            check_sd_path(&path)?;
            if self.transferring() {
                return Err(CoreError::Rejected(
                    "a transfer is already in progress".into(),
                ));
            }
            if self.job_running() {
                return Err(CoreError::Rejected(
                    "can't download while a job is running".into(),
                ));
            }
            Ok(())
        };
        if let Err(e) = check() {
            self.note(ConsoleKind::Error, e.to_string());
            let _ = reply.send(Err(e));
            return;
        }
        let download = Download::new(&path, have_md5);
        self.outgoing.push((download.start(), None));
        self.note(ConsoleKind::Info, format!("downloading {path}"));
        self.state.transfer = Some(Transfer {
            direction: TransferDirection::Download,
            path,
            done: 0,
            total: 0,
            run: false,
        });
        self.download = Some(PendingDownload {
            download,
            reply: Some(reply),
            last_activity: Instant::now(),
            settings: false,
        });
    }

    fn download_finished(&mut self, result: Result<Downloaded, String>) {
        let Some(d) = self.download.take() else {
            return;
        };
        let path = self
            .state
            .transfer
            .take()
            .map(|t| t.path)
            .unwrap_or_default();
        let result = match result {
            Ok(Downloaded::Data(data)) => {
                self.note(
                    ConsoleKind::Info,
                    format!("downloaded {path} ({} bytes)", data.len()),
                );
                Ok(Some(data))
            }
            Ok(Downloaded::Unchanged) => Ok(None),
            Err(e) => {
                let e = CoreError::Failed(format!("download failed: {e}"));
                self.note(ConsoleKind::Error, e.to_string());
                Err(e)
            }
        };
        if d.settings {
            self.settings_read(result);
        } else if let Some(reply) = d.reply {
            let _ = reply.send(result);
        }
    }

    fn on_transfer_frame(&mut self, frame: &Frame) {
        if let Some(d) = &mut self.download {
            d.last_activity = Instant::now();
            match d.download.receive(frame) {
                Ok(transfer::DownloadStep::Send(frames)) => {
                    let (done, total) = (
                        d.download.received_bytes() as u64,
                        d.download.expected_bytes(),
                    );
                    self.outgoing.extend(frames.into_iter().map(|f| (f, None)));
                    self.transfer_progress(done, total);
                }
                Ok(transfer::DownloadStep::Done(result)) => self.download_finished(Ok(result)),
                Ok(transfer::DownloadStep::Ignored) => {}
                Err(e) => {
                    if !matches!(e, TransferError::Cancelled(_)) {
                        self.outgoing
                            .push((Frame::new(FrameType::FILE_CANCEL, []), None));
                    }
                    self.download_finished(Err(e.to_string()));
                }
            }
            return;
        }
        // Frames after a transfer ended (the machine's `FILE_END ok`, a cancel echo) land here.
        let Some(up) = &mut self.upload else { return };
        up.last_activity = Instant::now();
        match up.upload.receive(frame) {
            Ok(transfer::Step::Send(frames)) => {
                let sent = up.upload.sent_bytes() as u64;
                self.outgoing.extend(frames.into_iter().map(|f| (f, None)));
                self.transfer_progress(sent, None);
            }
            Ok(transfer::Step::Done) => self.upload_finished(Ok(())),
            Ok(transfer::Step::Ignored) => {}
            Err(e) => {
                if !matches!(e, TransferError::Cancelled(_)) {
                    self.outgoing
                        .push((Frame::new(FrameType::FILE_CANCEL, []), None));
                }
                self.upload_finished(Err(e.to_string()));
            }
        }
    }

    fn upload_finished(&mut self, result: Result<(), String>) {
        let Some(up) = self.upload.take() else { return };
        let Some(transfer) = self.state.transfer.take() else {
            return;
        };
        let result = match result {
            Ok(()) => {
                self.note(
                    ConsoleKind::Info,
                    format!("uploaded {} ({} bytes)", transfer.path, transfer.total),
                );
                if self.state.files.dir == GCODE_DIR && self.file_op.is_none() {
                    let _ = self.list(GCODE_DIR.into());
                }
                if transfer.run {
                    self.play(transfer.path).map_err(|e| {
                        CoreError::Rejected(format!("uploaded, but couldn't start the job: {e}"))
                    })
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(CoreError::Failed(format!("upload failed: {e}"))),
        };
        if let Err(e) = &result {
            self.note(ConsoleKind::Error, e.to_string());
        }
        if let Some(reply) = up.reply {
            let _ = reply.send(result);
        }
    }

    fn play(&mut self, path: String) -> Result<(), CoreError> {
        self.require(&[MachineState::Idle], "start a job")?;
        if self.transferring() {
            return Err(CoreError::Rejected("a file transfer is in progress".into()));
        }
        self.queue_line(format!("play {}", escape_arg(&path)));
        self.state.job = Some(Job {
            path,
            aborting: false,
        });
        self.job_started = Some(Instant::now());
        self.job_seen = false;
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
        // Where it starts: the end of our jogs still running, or here at standstill. Moves queued
        // some other way (state Run) end somewhere we can't see.
        let start = match &self.coasting {
            Some(c) => Some(c.planned),
            None if state == MachineState::Idle => self
                .last_status
                .as_ref()
                .map(|s| s.machine_pos.0[..4].try_into().unwrap()),
            None => None,
        };
        // Keep jogs inside the soft limits, when we know where they start.
        if let Some(start) = start {
            let travel = self
                .state
                .machine
                .as_ref()
                .map(|m| m.travel.clone())
                .unwrap_or_default();
            for (i, d) in delta.iter_mut().take(3).enumerate() {
                let pos = start[i];
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
        if let Some(start) = start {
            let len = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
            let secs = len / (jog_rate(delta) * speed);
            let from = self.coasting.as_ref().map_or(Instant::now(), |c| c.until);
            self.coasting = Some(Coast {
                planned: std::array::from_fn(|i| start[i] + delta[i]),
                until: from.max(Instant::now()) + Duration::from_secs_f64(secs),
            });
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

impl Actor {
    async fn hold(&mut self, dir: [Option<f64>; 4], speed: f64) -> Result<(), CoreError> {
        if !(speed > 0.0 && speed <= 1.0) {
            return Err(CoreError::Rejected(format!(
                "jog speed {speed} must be in (0, 1]"
            )));
        }
        let dir = dir.map(|d| match d {
            Some(d) if d > 0.0 => 1.0,
            Some(d) if d < 0.0 => -1.0,
            _ => 0.0,
        });
        if dir.iter().all(|d| *d == 0.0) {
            return Err(CoreError::Rejected("no jog direction".into()));
        }
        if dir[3] != 0.0 && dir[..3].iter().any(|d| *d != 0.0) {
            return Err(CoreError::Rejected(
                "A can't be jogged together with X, Y or Z".into(),
            ));
        }
        let now = Instant::now();
        if let Some(h) = &mut self.held {
            if h.dir != dir {
                h.at_limit = false;
            }
            h.dir = dir;
            h.speed = speed;
            h.renewed = now;
            return Ok(());
        }
        // From standstill, or on from jogs of ours still running, so the planned end is exact.
        let planned = self.jog_start("jog")?;
        self.note(ConsoleKind::Info, "jogging while held".into());
        self.held = Some(HeldJog {
            dir,
            speed,
            planned,
            queued_until: self.coasting.take().map_or(now, |c| c.until.max(now)),
            renewed: now,
            at_limit: false,
        });
        self.hold_tick().await;
        Ok(())
    }

    /// Where a new jog starts: the end of our jogs still running, or else the position at
    /// standstill. Errors if the machine is busy some other way.
    fn jog_start(&self, what: &str) -> Result<[f64; 4], CoreError> {
        if let Some(c) = &self.coasting {
            self.require(&[MachineState::Idle, MachineState::Run], what)?;
            return Ok(c.planned);
        }
        self.require(&[MachineState::Idle], what)?;
        let pos = self
            .last_status
            .as_ref()
            .map(|s| s.machine_pos.0)
            .ok_or_else(|| CoreError::Rejected("position unknown".into()))?;
        Ok([pos[0], pos[1], pos[2], pos[3]])
    }

    fn coast(&mut self, h: HeldJog) {
        self.coasting = Some(Coast {
            planned: h.planned,
            until: h.queued_until,
        });
    }

    /// Top up a held jog's queue to [`HOLD_LEAD`] ahead, or end it.
    async fn hold_tick(&mut self) {
        let now = Instant::now();
        let alarm = self
            .state
            .status
            .as_ref()
            .is_none_or(|s| s.state == MachineState::Alarm);
        let Some(h) = &mut self.held else { return };
        if alarm {
            self.held = None;
            return;
        }
        if now.duration_since(h.renewed) > HOLD_TIMEOUT {
            let h = self.held.take().expect("held");
            self.coast(h);
            self.note(ConsoleKind::Info, "jog stopped: no longer held".into());
            return;
        }
        let from = h.queued_until.max(now);
        let target = now + HOLD_LEAD;
        // Skip slivers: top up in steps of about a tick.
        if target < from + HOLD_TICK / 2 {
            return;
        }
        let len = jog_rate(h.dir) * h.speed * (target - from).as_secs_f64();
        let norm = h.dir.iter().map(|d| d * d).sum::<f64>().sqrt();
        let mut delta = h.dir.map(|d| d / norm * len);
        // Shorten the move to stay inside the soft limits, keeping its direction.
        let travel = self
            .state
            .machine
            .as_ref()
            .map(|m| m.travel.clone())
            .unwrap_or_default();
        let mut fraction: f64 = 1.0;
        for (i, &d) in delta.iter().enumerate().take(3) {
            let end = h.planned[i] + d;
            let max = travel.max[i] - LIMIT_MARGIN;
            if d > 0.0 && end > max {
                fraction = fraction.min(((max - h.planned[i]) / d).max(0.0));
            }
            if let Some(min) = travel.min[i].map(|m| m + LIMIT_MARGIN)
                && d < 0.0
                && end < min
            {
                fraction = fraction.min(((min - h.planned[i]) / d).max(0.0));
            }
        }
        delta = delta.map(|d| d * fraction);
        h.queued_until = target;
        if delta.iter().all(|d| d.abs() < 1e-3) {
            if !h.at_limit {
                h.at_limit = true;
                self.note(ConsoleKind::Info, "jog stopped at the axis limit".into());
            }
            return;
        }
        for (p, d) in h.planned.iter_mut().zip(delta) {
            *p += d;
        }
        let words: Vec<String> = ["X", "Y", "Z", "A"]
            .iter()
            .zip(delta)
            .filter(|(_, d)| d.abs() >= 1e-4)
            .map(|(a, d)| format!("{a}{d:.4}"))
            .collect();
        // Not logged: ten a second would bury the console.
        let line = format!("$J {} F{:.3}", words.join(" "), h.speed);
        if self.send_raw(command::command(&line)).await.is_err() {
            self.held = None;
        }
    }
}

/// `$J` runs at its speed fraction of this: the slowest involved axis' max rate, along the path.
fn jog_rate(delta: [f64; 4]) -> f64 {
    (0..4)
        .filter(|&i| delta[i].abs() >= 1e-4)
        .map(|i| JOG_MAX_RATE[i])
        .fold(f64::INFINITY, f64::min)
}

/// Paths must be absolute on the SD card, without `..`.
/// A comment-only line, which the firmware wouldn't acknowledge.
fn is_comment(line: &str) -> bool {
    line.starts_with(';') || (line.starts_with('(') && line.ends_with(')'))
}

/// A G-code line (words like `G1`, `X10`, `M3`), as opposed to `$` and console commands. The
/// machine acknowledges every one with `ok` once it's queued.
fn is_gcode(line: &str) -> bool {
    let mut chars = line.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && matches!(chars.next(), Some(c) if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ' '))
}

fn check_sd_path(path: &str) -> Result<(), CoreError> {
    if (path == "/sd" || path.starts_with("/sd/")) && !path.split('/').any(|p| p == "..") {
        Ok(())
    } else {
        Err(CoreError::Rejected(format!(
            "not a path on the SD card: {path}"
        )))
    }
}
