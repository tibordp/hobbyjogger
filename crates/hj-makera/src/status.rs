//! Parsing of the status (`<...>`) and diagnose (`{...}`) reports.
//!
//! The status line is built by `Kernel::get_query_string` in the Z1 firmware:
//!
//! ```text
//! <Idle|MPos:x,y,z,a,b|WPos:x,y,z,a,b|F:cur,req,ovr|S:rpm,target,ovr,vac,spindleT,powerT,...
//!      |T:tool,offset[,target]|W:probeV|L:mode,state,testing,power,scale|P:lines,pct,secs
//!      |A:atc|O:maxDelta|H:halt|C:model,func,inch,abs>
//! ```
//!
//! Every field except the state and positions is optional.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub enum MachineState {
    Idle,
    Run,
    Hold,
    Home,
    Alarm,
    Sleep,
    /// Job suspended (`suspend`); reported as "Pause".
    Pause,
    Wait,
    /// Waiting for a manual tool change.
    Tool,
    Unknown(String),
}

impl MachineState {
    fn parse(s: &str) -> Self {
        match s {
            "Idle" => Self::Idle,
            "Run" => Self::Run,
            "Hold" => Self::Hold,
            "Home" => Self::Home,
            "Alarm" => Self::Alarm,
            "Sleep" => Self::Sleep,
            "Pause" => Self::Pause,
            "Wait" => Self::Wait,
            "Tool" => Self::Tool,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

/// Why the machine halted (`H:` field, `enum HALT_REASON` in the firmware).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum HaltReason {
    Manual,
    HomeFail,
    ProbeFail,
    CalibrateFail,
    AtcHomeFail,
    AtcToolInvalid,
    AtcNoTool,
    AtcHasTool,
    SpindleOverheated,
    SoftLimit,
    CoverOpen,
    ProbeInvalid,
    EStop,
    PowerOverheated,
    NonHome,
    HardLimit,
    MotorErrorX,
    MotorErrorY,
    MotorErrorZ,
    SpindleStall,
    SdError,
    SpindleAlarm,
    Other(u32),
}

/// What it takes to get out of a halt.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum HaltRecovery {
    /// `$X` / `M999` is enough.
    Clear,
    /// The controller needs a reset.
    Reset,
    /// The machine needs to be power cycled.
    PowerCycle,
}

impl HaltReason {
    pub fn from_code(code: u32) -> Self {
        use HaltReason::*;
        match code {
            1 => Manual,
            2 => HomeFail,
            3 => ProbeFail,
            4 => CalibrateFail,
            5 => AtcHomeFail,
            6 => AtcToolInvalid,
            7 => AtcNoTool,
            8 => AtcHasTool,
            9 => SpindleOverheated,
            10 => SoftLimit,
            11 => CoverOpen,
            12 => ProbeInvalid,
            13 => EStop,
            14 => PowerOverheated,
            15 => NonHome,
            21 => HardLimit,
            22 => MotorErrorX,
            23 => MotorErrorY,
            24 => MotorErrorZ,
            25 => SpindleStall,
            26 => SdError,
            41 => SpindleAlarm,
            other => Other(other),
        }
    }

    pub fn code(self) -> u32 {
        use HaltReason::*;
        match self {
            Manual => 1,
            HomeFail => 2,
            ProbeFail => 3,
            CalibrateFail => 4,
            AtcHomeFail => 5,
            AtcToolInvalid => 6,
            AtcNoTool => 7,
            AtcHasTool => 8,
            SpindleOverheated => 9,
            SoftLimit => 10,
            CoverOpen => 11,
            ProbeInvalid => 12,
            EStop => 13,
            PowerOverheated => 14,
            NonHome => 15,
            HardLimit => 21,
            MotorErrorX => 22,
            MotorErrorY => 23,
            MotorErrorZ => 24,
            SpindleStall => 25,
            SdError => 26,
            SpindleAlarm => 41,
            Other(code) => code,
        }
    }

    /// Recovery class, following the ranges documented in the firmware's `HALT_REASON`.
    pub fn recovery(self) -> HaltRecovery {
        match self {
            Self::SpindleAlarm => HaltRecovery::PowerCycle,
            Self::HardLimit
            | Self::MotorErrorX
            | Self::MotorErrorY
            | Self::MotorErrorZ
            | Self::SpindleStall
            | Self::SdError => HaltRecovery::Reset,
            Self::Other(code) if code >= 40 => HaltRecovery::PowerCycle,
            Self::Other(code) if code >= 20 => HaltRecovery::Reset,
            _ => HaltRecovery::Clear,
        }
    }
}

/// Positions in X, Y, Z, A, B order. Linear axes in mm, rotary in degrees.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize)]
pub struct Axes(pub [f64; 5]);

impl Axes {
    pub fn x(&self) -> f64 {
        self.0[0]
    }
    pub fn y(&self) -> f64 {
        self.0[1]
    }
    pub fn z(&self) -> f64 {
        self.0[2]
    }
    pub fn a(&self) -> f64 {
        self.0[3]
    }
    pub fn b(&self) -> f64 {
        self.0[4]
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub struct Feed {
    /// mm/min. Only non-zero while moving.
    pub current: f64,
    /// mm/min.
    pub requested: f64,
    /// Percent.
    pub override_pct: f64,
}

#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Spindle {
    pub current_rpm: f64,
    pub target_rpm: f64,
    /// Percent.
    pub override_pct: f64,
    pub vacuum_mode: Option<i32>,
    /// °C
    pub spindle_temp: Option<f64>,
    /// °C
    pub power_temp: Option<f64>,
    /// Remaining mode flags (blowing, bed clean, ..., static removal). Meaning varies by model.
    pub modes: Vec<f64>,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub struct Tool {
    /// Active tool; -1 for none.
    pub number: i32,
    /// Tool length offset, mm.
    pub offset: f64,
    /// Requested tool during a manual tool change (only reported on non-ATC machines like the Z1);
    /// -1 when none is pending.
    pub target: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub struct Laser {
    pub mode: bool,
    pub on: bool,
    pub testing: bool,
    pub power: f64,
    pub scale: f64,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub struct Progress {
    pub lines: u64,
    pub percent: u32,
    pub elapsed_secs: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub struct MachineInfo {
    /// 1 Carvera, 2 Carvera Air, 3 Z1, 4 Z1 Pro.
    pub model: u32,
    /// Feature bitmask from the factory settings; bit 2 = ATC.
    pub func_setting: u32,
    pub inch_mode: bool,
    pub absolute_mode: bool,
}

impl MachineInfo {
    pub fn model_name(&self) -> &'static str {
        match self.model {
            1 => "Carvera",
            2 => "Carvera Air",
            3 => "Z1",
            4 => "Z1 Pro",
            _ => "unknown",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Status {
    pub state: MachineState,
    pub machine_pos: Axes,
    pub work_pos: Axes,
    pub feed: Option<Feed>,
    pub spindle: Option<Spindle>,
    pub tool: Option<Tool>,
    /// Wireless probe battery voltage.
    pub probe_voltage: Option<f64>,
    pub laser: Option<Laser>,
    /// Present while a file is playing.
    pub progress: Option<Progress>,
    pub atc_state: Option<u32>,
    /// Present while auto-leveling compensation is active.
    pub leveling_max_delta: Option<f64>,
    pub halt_reason: Option<HaltReason>,
    pub machine: Option<MachineInfo>,
    /// Fields we don't interpret, verbatim.
    pub extra: BTreeMap<String, String>,
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("malformed status report: {reason}")]
pub struct ParseError {
    pub reason: String,
}

fn err(reason: impl Into<String>) -> ParseError {
    ParseError {
        reason: reason.into(),
    }
}

fn floats(field: &str, s: &str) -> Result<Vec<f64>, ParseError> {
    s.split(',')
        .map(|v| {
            v.trim()
                .parse::<f64>()
                .map_err(|_| err(format!("bad number {v:?} in {field}")))
        })
        .collect()
}

fn need(field: &str, v: &[f64], n: usize) -> Result<(), ParseError> {
    if v.len() < n {
        return Err(err(format!("{field} has {} values, need {n}", v.len())));
    }
    Ok(())
}

fn axes(field: &str, v: &[f64]) -> Result<Axes, ParseError> {
    need(field, v, 3)?;
    let mut out = [0.0; 5];
    for (o, i) in out.iter_mut().zip(v) {
        *o = *i;
    }
    Ok(Axes(out))
}

impl Status {
    /// Parse a status report. Accepts surrounding whitespace.
    ///
    /// Linear positions and feeds are converted to mm if the machine reports inch mode.
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let inner = line
            .trim()
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .ok_or_else(|| err("not enclosed in <>"))?;

        let mut parts = inner.split('|');
        let state = MachineState::parse(parts.next().unwrap_or_default());

        let mut status = Status {
            state,
            machine_pos: Axes::default(),
            work_pos: Axes::default(),
            feed: None,
            spindle: None,
            tool: None,
            probe_voltage: None,
            laser: None,
            progress: None,
            atc_state: None,
            leveling_max_delta: None,
            halt_reason: None,
            machine: None,
            extra: BTreeMap::new(),
        };
        let (mut has_mpos, mut has_wpos) = (false, false);

        for part in parts {
            let Some((key, value)) = part.split_once(':') else {
                status.extra.insert(part.to_owned(), String::new());
                continue;
            };
            // Only the first occurrence of a key is interpreted; temperature controllers can
            // reuse single-letter designators.
            match key {
                "MPos" if !has_mpos => {
                    status.machine_pos = axes(key, &floats(key, value)?)?;
                    has_mpos = true;
                }
                "WPos" if !has_wpos => {
                    status.work_pos = axes(key, &floats(key, value)?)?;
                    has_wpos = true;
                }
                "F" if status.feed.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 3)?;
                    status.feed = Some(Feed {
                        current: v[0],
                        requested: v[1],
                        override_pct: v[2],
                    });
                }
                "S" if status.spindle.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 3)?;
                    status.spindle = Some(Spindle {
                        current_rpm: v[0],
                        target_rpm: v[1],
                        override_pct: v[2],
                        vacuum_mode: v.get(3).map(|x| *x as i32),
                        spindle_temp: v.get(4).copied(),
                        power_temp: v.get(5).copied(),
                        modes: v.get(6..).unwrap_or_default().to_vec(),
                    });
                }
                "T" if status.tool.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 2)?;
                    status.tool = Some(Tool {
                        number: v[0] as i32,
                        offset: v[1],
                        target: v.get(2).map(|x| *x as i32),
                    });
                }
                "W" if status.probe_voltage.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 1)?;
                    status.probe_voltage = Some(v[0]);
                }
                "L" if status.laser.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 5)?;
                    status.laser = Some(Laser {
                        mode: v[0] != 0.0,
                        on: v[1] != 0.0,
                        testing: v[2] != 0.0,
                        power: v[3],
                        scale: v[4],
                    });
                }
                "P" if status.progress.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 3)?;
                    status.progress = Some(Progress {
                        lines: v[0] as u64,
                        percent: v[1] as u32,
                        elapsed_secs: v[2] as u64,
                    });
                }
                "A" if status.atc_state.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 1)?;
                    status.atc_state = Some(v[0] as u32);
                }
                "O" if status.leveling_max_delta.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 1)?;
                    status.leveling_max_delta = Some(v[0]);
                }
                "H" if status.halt_reason.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 1)?;
                    status.halt_reason = Some(HaltReason::from_code(v[0] as u32));
                }
                "C" if status.machine.is_none() => {
                    let v = floats(key, value)?;
                    need(key, &v, 4)?;
                    status.machine = Some(MachineInfo {
                        model: v[0] as u32,
                        func_setting: v[1] as u32,
                        inch_mode: v[2] != 0.0,
                        absolute_mode: v[3] != 0.0,
                    });
                }
                _ => {
                    status.extra.insert(key.to_owned(), value.to_owned());
                }
            }
        }

        if !has_mpos || !has_wpos {
            return Err(err("missing MPos or WPos"));
        }
        if status.machine.is_some_and(|m| m.inch_mode) {
            status.convert_inch_to_mm();
        }
        Ok(status)
    }

    fn convert_inch_to_mm(&mut self) {
        const MM_PER_INCH: f64 = 25.4;
        for p in [&mut self.machine_pos, &mut self.work_pos] {
            for v in &mut p.0[..3] {
                *v *= MM_PER_INCH;
            }
        }
        if let Some(f) = &mut self.feed {
            f.current *= MM_PER_INCH;
            f.requested *= MM_PER_INCH;
        }
    }
}

/// Diagnose report (`{S:..|L:..|...}`), reply to the `diagnose` command. Kept as a raw map for
/// now; field meanings are documented in `docs/research/carvera-reference.md`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Diagnose(pub BTreeMap<String, Vec<i64>>);

impl Diagnose {
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let inner = line
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .ok_or_else(|| err("not enclosed in {}"))?;
        let mut map = BTreeMap::new();
        for part in inner.split('|').filter(|p| !p.is_empty()) {
            let (key, value) = part
                .split_once(':')
                .ok_or_else(|| err(format!("field {part:?} has no ':'")))?;
            let values = value
                .split(',')
                .map(|v| {
                    v.trim()
                        .parse::<i64>()
                        .map_err(|_| err(format!("bad number {v:?} in {key}")))
                })
                .collect::<Result<_, _>>()?;
            map.insert(key.to_owned(), values);
        }
        Ok(Self(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape follows Kernel::get_query_string for a Z1 (manual tool change, so T has 3 values).
    const Z1_IDLE: &str = "<Idle|MPos:-200.0000,-150.0000,-5.0000,0.0000,0.0000\
        |WPos:10.0000,20.0000,30.0000,0.0000,0.0000|F:0.0,3000.0,100.0\
        |S:0.0,0.0,100.0,0,25.3,31.0,0,0,0,0|T:1,-12.345,1|W:4.10\
        |L:0, 0, 0, 0.0,100.0|C:3,0,0,1>\n";

    #[test]
    fn parses_z1_idle() {
        let s = Status::parse(Z1_IDLE).unwrap();
        assert_eq!(s.state, MachineState::Idle);
        assert_eq!(s.machine_pos.x(), -200.0);
        assert_eq!(s.work_pos.z(), 30.0);
        assert_eq!(s.feed.unwrap().requested, 3000.0);
        let sp = s.spindle.unwrap();
        assert_eq!(sp.spindle_temp, Some(25.3));
        assert_eq!(sp.power_temp, Some(31.0));
        assert_eq!(sp.modes.len(), 4);
        assert_eq!(
            s.tool,
            Some(Tool {
                number: 1,
                offset: -12.345,
                target: Some(1)
            })
        );
        assert_eq!(s.probe_voltage, Some(4.10));
        assert!(!s.laser.unwrap().mode);
        let m = s.machine.unwrap();
        assert_eq!(m.model_name(), "Z1");
        assert!(m.absolute_mode);
        assert!(s.halt_reason.is_none());
        assert!(s.extra.is_empty());
    }

    #[test]
    fn parses_halted_and_running_fields() {
        let s = Status::parse(
            "<Alarm|MPos:0,0,0|WPos:0,0,0|P:120,45,300|O:0.125|H:21|E:1,0,0,1024,4096>",
        )
        .unwrap();
        assert_eq!(s.state, MachineState::Alarm);
        assert_eq!(
            s.progress,
            Some(Progress {
                lines: 120,
                percent: 45,
                elapsed_secs: 300
            })
        );
        assert_eq!(s.leveling_max_delta, Some(0.125));
        assert_eq!(s.halt_reason, Some(HaltReason::HardLimit));
        assert_eq!(HaltReason::HardLimit.recovery(), HaltRecovery::Reset);
        for code in 0..50 {
            assert_eq!(HaltReason::from_code(code).code(), code);
        }
        assert_eq!(
            s.extra.get("E").map(String::as_str),
            Some("1,0,0,1024,4096")
        );
    }

    #[test]
    fn converts_inch_mode_to_mm() {
        let s = Status::parse("<Idle|MPos:1,2,3,90|WPos:1,0,0|F:0,10,100|C:3,0,1,1>").unwrap();
        let expected = [25.4, 50.8, 76.2, 90.0, 0.0];
        for (got, want) in s.machine_pos.0.iter().zip(expected) {
            assert!((got - want).abs() < 1e-9, "{got} != {want}");
        }
        assert_eq!(s.feed.unwrap().requested, 254.0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Status::parse("ok").is_err());
        assert!(Status::parse("<Idle|F:0,0,0>").is_err());
        assert!(Status::parse("<Idle|MPos:a,b,c|WPos:0,0,0>").is_err());
    }

    /// Captured from a real Z1 (firmware `version = 1.1.2.0.1.13`), idle after homing.
    const Z1_CAPTURED: &str = "<Idle|MPos:-1.0000,-1.0000,-1.0000,0.0000,0.0000\
        |WPos:66.4050,123.9570,45.3106,0.0000,0.0000|F:0.0,2000.0,100.0\
        |S:0.0,10000.0,100.0,0,21.7,21.9,0,0,0,0|T:3,-18.211,-1|L:0, 0, 0, 0.0,100.0\
        |C:3,1,0,1|E:0,0,0,25,7613|OTA:0,0>\n";

    #[test]
    fn parses_captured_z1_status() {
        let s = Status::parse(Z1_CAPTURED).unwrap();
        assert_eq!(s.state, MachineState::Idle);
        assert_eq!(s.work_pos.y(), 123.957);
        assert_eq!(
            s.tool,
            Some(Tool {
                number: 3,
                offset: -18.211,
                target: Some(-1)
            })
        );
        assert_eq!(s.probe_voltage, None);
        assert_eq!(s.machine.unwrap().func_setting, 1);
        assert_eq!(s.extra.keys().collect::<Vec<_>>(), ["E", "OTA"]);
    }

    #[test]
    fn parses_diagnose() {
        let d =
            Diagnose::parse("{S:0,5000|L:0,0|F:1,0|V:0,1|G:0|E:0,0,0,0,0,0|RSSI:-55}\n").unwrap();
        assert_eq!(d.0["S"], vec![0, 5000]);
        assert_eq!(d.0["RSSI"], vec![-55]);
    }
}
