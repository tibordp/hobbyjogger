//! Application state shared with the frontend, plus the high-frequency telemetry and console
//! types that travel on their own channels (see `docs/architecture.md`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contour::{Contour, ContourSpec};
use crate::map::{ExportFormat, MapSpec, SurfaceMap};
use crate::probe::{ProbeResult, ProbeSettings, Routine};

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub connection: Connection,
    pub machine: Option<MachineInfo>,
    pub status: Option<MachineStatus>,
    pub discovered: Vec<DiscoveredMachine>,
    /// The machine's SD card, as browsed.
    pub files: Files,
    pub transfer: Option<Transfer>,
    /// The job we started, while it runs. A job started elsewhere only shows in `status.job`.
    pub job: Option<Job>,
    /// A manual tool change in progress, however it was started (button, program, console).
    pub tool_change: Option<ToolChange>,
    pub probe: ProbeState,
    /// Switchable outputs, from the `diagnose` report; `None` until the first one.
    pub outputs: Option<Outputs>,
    /// The machine's configuration (`config.txt`), once read.
    pub settings: Settings,
    /// Lines from the console being sent one by one ([`Request::Mdi`] with several lines).
    pub batch: Option<Batch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Batch {
    /// Lines the machine has accepted.
    pub done: u32,
    pub total: u32,
}

/// The outputs the firmware reports in `diagnose`. Each is `None` when this machine doesn't have
/// it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Outputs {
    pub light: Option<bool>,
    /// `M801`/`M802`. The firmware calls it the power fan on machines without an ATC (the Z1);
    /// laser mode runs it at 40 %.
    pub power_fan: Option<Pwm>,
    pub spindle_fan: Option<Pwm>,
    /// The external output port (`M851`/`M852`).
    pub ext_out: Option<Pwm>,
    /// Air assist (`M7`/`M9`).
    pub air: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Pwm {
    pub on: bool,
    /// Percent.
    pub power: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Output {
    Light,
    PowerFan,
    SpindleFan,
    ExtOut,
    Air,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    /// In the order of the config file; `None` until read.
    pub entries: Option<Vec<Setting>>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Setting {
    pub key: String,
    pub value: String,
    /// What we know about the key (see `crate::settings`), for the ones worth explaining.
    pub title: Option<String>,
    pub detail: Option<String>,
    pub group: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeState {
    pub settings: ProbeSettings,
    pub running: Option<RunningProbe>,
    /// Measured contacts of the current or last routine (tool tip, machine coordinates).
    pub contacts: Vec<[f64; 3]>,
    pub result: Option<ProbeResult>,
    /// Why the last routine stopped, if it didn't finish.
    pub error: Option<String>,
    /// After a failure: whether `RecoverProbe` knows which way to back the probe off.
    pub can_back_off: bool,
    /// The surface map being measured, or the last one.
    pub map: Option<SurfaceMap>,
    /// The contour being traced, or the last one.
    pub contour: Option<Contour>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunningProbe {
    pub label: String,
    pub step: String,
}

/// The 3D probe's tool number on the Z1.
pub const PROBE_TOOL: i32 = 9999;
/// The laser module's tool number. Changing to it puts the firmware in laser mode (`M321`).
pub const LASER_TOOL: i32 = 8888;
/// The depth probe's tool number: a switch that only touches down, so it measures Z only. The
/// firmware treats it as a probe like [`PROBE_TOOL`] (same input, same motion guard) and switches
/// it on with `M494.1` around its own Z probing.
pub const DEPTH_PROBE_TOOL: i32 = 0;

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolChange {
    pub target: i32,
    pub phase: ToolChangePhase,
    /// Measuring the tool that's in again (`RemeasureTool`), not changing it.
    pub remeasure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolChangePhase {
    /// Going to the change position.
    Moving,
    /// In the `Tool` state, waiting for the user to swap the tool and confirm.
    Waiting,
    /// Confirmed; the machine measures the new tool on the sensor.
    Measuring,
}

/// Where uploads go and browsing starts.
pub const GCODE_DIR: &str = "/sd/gcodes";

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Files {
    pub dir: String,
    /// `None` until listed.
    pub entries: Option<Vec<FileEntry>>,
    /// A listing or delete is in progress.
    pub busy: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            dir: GCODE_DIR.into(),
            entries: None,
            busy: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileEntry {
    pub name: String,
    pub dir: bool,
    #[ts(type = "number")]
    pub size: u64,
    /// `YYYY-MM-DD hh:mm`; `None` when the machine's clock wasn't set.
    pub modified: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Transfer {
    pub direction: TransferDirection,
    /// The file on the machine.
    pub path: String,
    #[ts(type = "number")]
    pub done: u64,
    /// For downloads, an estimate (whole blocks) until the transfer ends; 0 until known.
    #[ts(type = "number")]
    pub total: u64,
    /// Start the job once uploaded.
    pub run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Job {
    pub path: String,
    /// `abort` sent; the machine finishes the queued moves first, which takes a few seconds.
    pub aborting: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Connection {
    Disconnected {
        #[ts(optional)]
        error: Option<String>,
    },
    Connecting {
        target: String,
    },
    Connected {
        target: String,
        transport: Transport,
    },
}

impl Default for Connection {
    fn default() -> Self {
        Self::Disconnected { error: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Transport {
    Wifi,
    Usb,
    Simulator,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ConnectTarget {
    Tcp {
        host: String,
        #[ts(optional)]
        port: Option<u16>,
    },
    Usb,
    Simulator,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MachineInfo {
    /// e.g. "Z1"
    pub model: Option<String>,
    /// As reported by `version`: motion firmware version followed by the ESP32's.
    pub firmware: Option<String>,
    pub travel: Travel,
}

/// Soft limits in machine coordinates for X, Y, Z. The max side is the homed position; the min
/// side is read from the machine's config (`soft_endstop.*_min`) after connecting.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Travel {
    pub min: [Option<f64>; 3],
    pub max: [f64; 3],
}

impl Default for Travel {
    fn default() -> Self {
        Self {
            min: [None; 3],
            max: [-1.0; 3],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MachineState {
    Idle,
    Run,
    Hold,
    Home,
    Alarm,
    Sleep,
    Pause,
    Wait,
    Tool,
    Unknown,
}

impl From<&hj_makera::status::MachineState> for MachineState {
    fn from(s: &hj_makera::status::MachineState) -> Self {
        use hj_makera::status::MachineState as M;
        match s {
            M::Idle => Self::Idle,
            M::Run => Self::Run,
            M::Hold => Self::Hold,
            M::Home => Self::Home,
            M::Alarm => Self::Alarm,
            M::Sleep => Self::Sleep,
            M::Pause => Self::Pause,
            M::Wait => Self::Wait,
            M::Tool => Self::Tool,
            M::Unknown(_) => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HaltRecovery {
    Clear,
    Reset,
    PowerCycle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Halt {
    pub code: u32,
    /// Firmware name of the reason, e.g. "SoftLimit".
    pub reason: String,
    pub recovery: HaltRecovery,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolInfo {
    /// -1 = none; 0 = depth probe; 8888 = laser; 9999 = wired 3D probe.
    pub number: i32,
    /// Tool length offset relative to the reference tool, mm.
    pub offset: f64,
    /// Requested tool during a manual tool change.
    pub target: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobProgress {
    #[ts(type = "number")]
    pub lines: u64,
    pub percent: u32,
    #[ts(type = "number")]
    pub elapsed_secs: u64,
}

/// Slow-changing parts of the status report. Positions and live rates are in [`Telemetry`].
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MachineStatus {
    pub state: MachineState,
    pub halt: Option<Halt>,
    pub tool: Option<ToolInfo>,
    pub spindle_target_rpm: Option<f64>,
    pub spindle_override_pct: Option<f64>,
    pub feed_requested: Option<f64>,
    pub feed_override_pct: Option<f64>,
    /// Active work offset (machine − work) for X, Y, Z, A, B, rounded to 0.1 µm.
    pub work_offset: [f64; 5],
    pub job: Option<JobProgress>,
    pub absolute_mode: Option<bool>,
    pub laser: Option<LaserStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaserStatus {
    /// The firmware's laser mode (`M321`), on while the laser is the tool: `M3 S` sets its power.
    pub mode: bool,
    /// The beam is on (`M3` in laser mode, or a test fire).
    pub on: bool,
    /// Test fire (`M323`) at the configured test power.
    pub testing: bool,
    /// Percent of the power scale (`M325`).
    pub scale: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscoveredMachine {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub busy: bool,
    pub state: Option<String>,
}

/// Emitted with every status report.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Telemetry {
    /// Machine position X, Y, Z, A, B.
    pub machine: [f64; 5],
    /// Work position X, Y, Z, A, B.
    pub work: [f64; 5],
    /// Live feed, mm/min.
    pub feed: f64,
    pub spindle_rpm: f64,
    pub spindle_temp: Option<f64>,
    pub power_temp: Option<f64>,
    /// Laser output, percent of full PWM, while in laser mode.
    pub laser_power: Option<f64>,
    /// False when the firmware reports a planned end position rather than the live one
    /// (`Hold`, `Wait`, `Pause`).
    pub live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConsoleKind {
    /// Sent to the machine.
    Sent,
    /// Received from the machine.
    Received,
    /// An error or alarm from the machine, or a rejected request.
    Error,
    /// A note from Hobby Jogger itself.
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsoleEntry {
    #[ts(type = "number")]
    pub id: u64,
    /// Milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub time: u64,
    pub kind: ConsoleKind,
    pub text: String,
}

/// Everything the frontend can ask for.
#[derive(Clone, Debug, PartialEq, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Request {
    Connect {
        target: ConnectTarget,
    },
    Disconnect,
    /// Relative jog. `speed` is a fraction of the axis max rate (0, 1].
    Jog {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        #[ts(optional)]
        z: Option<f64>,
        #[ts(optional)]
        a: Option<f64>,
        speed: f64,
    },
    /// Jog continuously while this keeps coming: `x`…`a` give the direction (their signs; A can't
    /// be combined with the others), `speed` as in [`Request::Jog`]. Repeat it at least every
    /// half a second or the core stops. It starts only when idle; repeats may
    /// change the direction and speed.
    JogHold {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        #[ts(optional)]
        z: Option<f64>,
        #[ts(optional)]
        a: Option<f64>,
        speed: f64,
    },
    /// End a [`Request::JogHold`]: nothing more is queued, so the machine stops once it has run
    /// what is (a fraction of a second's motion).
    JogRelease,
    /// Stop jogging: halt, then clear the halt (`$X`).
    StopJog,
    Home,
    Halt,
    Unlock,
    /// Reboot the motion controller (`reset`). Aborts whatever is running.
    Reset,
    /// Set the work position of the given axes to the given values (`G10 L20 P0`).
    SetWork {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        #[ts(optional)]
        z: Option<f64>,
        #[ts(optional)]
        a: Option<f64>,
    },
    /// Remove whole turns from the A axis so the work A is within ±360° (`G92.4 A0 S0`). No motion.
    ShrinkA,
    /// Put the work origin of X/Y at the given machine coordinates, without moving (`G10 L20`
    /// relative to the current position).
    SetOrigin {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        #[ts(optional)]
        z: Option<f64>,
    },
    /// Run a probing routine from the current position. Needs the 3D probe in the spindle.
    Probe {
        routine: Routine,
    },
    /// Stop a running routine: halt, then clear the halt.
    CancelProbe,
    /// After a routine stopped partway: clear a halt, back the probe off the way it came until it
    /// releases, and clear the error.
    RecoverProbe,
    /// Measure the tool that's in on the sensor again (`M491`), after it was taken out and put
    /// back, say. Shows as a tool change in its measuring phase. Not for the laser.
    RemeasureTool,
    /// Clear the probing error or result without moving.
    DismissProbe,
    /// Start a surface map from the current position, which is its top height: the probe must
    /// be above everything in the area. Replaces the last map.
    MapSurface {
        spec: MapSpec,
    },
    /// Carry on with a paused or stopped map, from the point after the last one measured.
    ResumeMap,
    /// Stop a running map after the point it's on, at its top height.
    PauseMap,
    /// Forget the map.
    DiscardMap,
    /// Trace a closed contour at the current height with the 3D probe, from where it is: outside a
    /// part (within `spec`'s box) or inside a pocket. Replaces the last contour.
    TraceContour {
        spec: ContourSpec,
    },
    /// Forget the contour.
    DiscardContour,
    /// Write the contour to `path` in work coordinates: CSV, TSV or DXF. With `compensate`, on the
    /// wall rather than where the ball's centre was ([`crate::contour::wall`]).
    ExportContour {
        path: String,
        format: ExportFormat,
        compensate: bool,
    },
    /// Write the map to `path` in work coordinates. With `compensate`, corrected for the ball
    /// touching slopes off its bottom ([`crate::map::surface`]).
    ExportMap {
        path: String,
        format: ExportFormat,
        compensate: bool,
    },
    SetProbeSettings {
        settings: ProbeSettings,
    },
    /// Move to a machine position, as a jog; axes left out stay where they are. `speed` as in
    /// [`Request::Jog`].
    MoveTo {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        #[ts(optional)]
        z: Option<f64>,
        speed: f64,
    },
    /// List a directory on the SD card (the current one if `dir` is omitted).
    ListFiles {
        #[ts(optional)]
        dir: Option<String>,
    },
    DeleteFile {
        path: String,
    },
    /// Upload a program to [`GCODE_DIR`] as `name`, optionally starting it once uploaded. Resolves
    /// when the upload is done.
    Upload {
        name: String,
        text: String,
        run: bool,
    },
    /// Run a program from the SD card.
    Play {
        path: String,
    },
    /// Pause the running job (`suspend`). The machine finishes its queued moves first.
    Pause,
    Resume,
    /// Stop the running job (`abort`). The machine finishes its queued moves first.
    Abort,
    /// Manual tool change to `tool` (`M6 T<n>`; [`PROBE_TOOL`] for the 3D probe,
    /// [`DEPTH_PROBE_TOOL`] for the depth probe, [`LASER_TOOL`] for the laser). The machine
    /// moves to the change position and waits for [`Request::ConfirmToolChange`]. The firmware's
    /// laser mode follows the tool: changing to the laser enters it (`M321`), changing away
    /// leaves it (`M322`).
    ChangeTool {
        tool: i32,
    },
    /// The new tool is in: measure it and carry on (`M490.2`). Also for tool changes in a job.
    ConfirmToolChange,
    /// Feed (`M220`, 10–300 %), spindle (`M223`, 50–200 %) and laser power (`M325`, 10–200 %)
    /// overrides. Also while a job runs; moves already queued keep their speed.
    SetOverride {
        #[ts(optional)]
        feed: Option<f64>,
        #[ts(optional)]
        spindle: Option<f64>,
        #[ts(optional)]
        laser: Option<f64>,
    },
    /// Switch an output; `power` (percent) for the PWM ones, full power if omitted.
    SetOutput {
        output: Output,
        on: bool,
        #[ts(optional)]
        power: Option<f64>,
    },
    /// Fire the laser at its test power (`M323`) or stop (`M324`), with the laser in.
    LaserTest {
        on: bool,
    },
    /// Read the machine's configuration (`/sd/config.txt`, downloaded) into [`AppState::settings`].
    LoadSettings,
    /// Stop sending the rest of a batch. What the machine has queued still runs (Halt stops it).
    StopBatch,
    /// Send from the console. A single line goes verbatim. Several lines (G-code only: they're
    /// the ones the machine acknowledges) are sent one at a time, each after the machine accepted
    /// the previous one (`ok`, given once it's queued), stopping at the first error; see
    /// [`AppState::batch`].
    Mdi {
        line: String,
    },
}
