//! Application state shared with the frontend, plus the high-frequency telemetry and console
//! types that travel on their own channels (see `docs/architecture.md`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub connection: Connection,
    pub machine: Option<MachineInfo>,
    pub status: Option<MachineStatus>,
    pub discovered: Vec<DiscoveredMachine>,
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
    /// -1 = none; 9999 = wired 3D probe.
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
    /// Move X/Y to a machine position at the current height, as a jog. `speed` as in [`Request::Jog`].
    MoveTo {
        #[ts(optional)]
        x: Option<f64>,
        #[ts(optional)]
        y: Option<f64>,
        speed: f64,
    },
    /// Send a line verbatim from the console.
    Mdi {
        line: String,
    },
}
