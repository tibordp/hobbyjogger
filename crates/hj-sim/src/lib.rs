//! A simulated Makera Z1 speaking the framed protocol over TCP.
//!
//! It reproduces the behaviour measured on a real machine (see `docs/research/z1-protocol.md`),
//! not the full firmware: status reports in the Z1 format, relative `$J` jogs with speed as a
//! fraction of the axis max rate and no reply, queued moves, homing, halt and `$X`, soft limits at
//! the homed position, `G10 L20` work offsets, `G53`/`G90`/`G91` moves, spindle on/off, and the
//! `version`/`model`/`ftype`/`diagnose` queries, a `config.txt`, overrides, the switchable
//! outputs and laser mode (from the firmware source, not measured), an in-memory SD card (`ls -s`,
//! `rm`, uploads, downloads) and job playback (`play`, `suspend`, `resume`, `abort`) with the `P:`
//! progress field.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hj_makera::command::unescape_arg;
use hj_makera::frame::{Decoder, Frame, FrameType};
use hj_makera::transfer::md5_hex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};

const TICK: Duration = Duration::from_millis(20);
/// Max rates in mm/s (deg/s for A, B), as measured: X/Y 1200 mm/min, Z 600, A ~1800 °/min.
const MAX_RATE: [f64; 5] = [20.0, 20.0, 10.0, 30.0, 30.0];
/// Soft limits on X, Y, Z (machine coordinates). Max is the homed position.
/// As configured on the Z1 (`soft_endstop.*_min`).
const LIMIT_MIN: [f64; 3] = [-207.0, -206.0, -102.0];
const LIMIT_MAX: f64 = -1.0;
const HOME: [f64; 3] = [-1.0, -1.0, -1.0];
const SPINDLE_RAMP_RPM_S: f64 = 1500.0;
/// Block size for downloads. Smaller than the real machine's 8 KiB so tests span several blocks.
const DOWNLOAD_BLOCK: usize = 512;
/// Lines of a playing job kept in the planner queue.
const JOB_LOOKAHEAD: usize = 3;
/// Timestamp of the seeded files; uploads get 1980-01-01 like on a machine whose clock isn't set.
const SEED_DATE: &str = "20261027120000";
const UPLOAD_DATE: &str = "19800101000000";

/// The SD card's `config.txt`, a few keys of it.
const CONFIG: &str = "# Hobby Jogger simulator\n\
soft_endstop.enable  true\n\
soft_endstop.x_min   -207.0   # mm\n\
soft_endstop.y_min   -206.0\n\
soft_endstop.z_min   -102.0\n\
spindle.max_rpm      13000\n\
laser_module_test_power 0.01\n\
switch.light.input_on_command M821\n";

const SQUARE: &str = "(sim: 20 mm square, 0.5 mm deep)\nG21 G90\nM3 S10000\nG0 Z5\nG0 X0 Y0\nG1 Z-0.5 F300\nG1 X20 F600\nG1 Y20\nG1 X0\nG1 Y0\nG0 Z5\nM5\n";
const CIRCLE: &str = "(sim: 10 mm circle)\nG21 G90\nG0 Z5\nG0 X10 Y0\nG1 Z-0.2 F300\nG2 X10 Y0 I-10 J0 F600\nG0 Z5\n";

/// Manual tool change (`M6 T<n>`), as on the Z1: move to the change position, wait in the `Tool`
/// state for `M490.2`, measure the new tool on the sensor, then `Done ATC`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Atc {
    Approach(i32),
    Waiting(i32),
    Measuring(i32),
}

impl Atc {
    fn target(self) -> i32 {
        match self {
            Self::Approach(t) | Self::Waiting(t) | Self::Measuring(t) => t,
        }
    }
}

/// Machine positions of the tool change routine (from a captured `M6`).
const ATC_CLEARANCE_Z: f64 = -3.0;
const ATC_CHANGE_XY: [f64; 2] = [-9.6, -13.3];
const ATC_SENSOR_XY: [f64; 2] = [-9.4, -13.3];
const ATC_SENSOR_Z: f64 = -82.154;

/// A part on the bed for probing, in machine coordinates of the tool tip: a 50 × 40 mm block,
/// top at Z-60, with a Ø16 mm bore 10 mm deep and a Ø12 mm boss 5 mm tall on top.
pub const PART_MIN: [f64; 2] = [-150.0, -150.0];
pub const PART_MAX: [f64; 2] = [-100.0, -110.0];
pub const PART_TOP: f64 = -60.0;
pub const BORE_CENTER: [f64; 2] = [-138.0, -130.0];
pub const BORE_DIAMETER: f64 = 16.0;
pub const BORE_DEPTH: f64 = 10.0;
pub const BOSS_CENTER: [f64; 2] = [-112.0, -130.0];
pub const BOSS_DIAMETER: f64 = 12.0;
pub const BOSS_HEIGHT: f64 = 5.0;
/// A third part, for edge angles: a 20 mm square block turned 5° counter-clockwise, top at Z-60.
pub const TILTED_CENTER: [f64; 2] = [-60.0, -175.0];
pub const TILTED_SIZE: f64 = 20.0;
pub const TILTED_ANGLE: f64 = 5.0;

/// How far from where the probe let go its contact chatters, mm.
const CHATTER: f64 = 0.1;

/// The simulated probe's ball radius.
pub const BALL_RADIUS: f64 = 1.0;

/// A second part, for surface mapping: a plate whose top slopes up toward +X, with a spherical
/// dome rising out of it and a deep hole in one corner. Both have exact ball contact, so a map of it can be checked against the
/// true surface.
pub const PLATE_MIN: [f64; 2] = [-90.0, -150.0];
pub const PLATE_MAX: [f64; 2] = [-30.0, -110.0];
/// The plate's top: `PLATE_TOP + PLATE_SLOPE * (x - PLATE_MIN[0])`.
pub const PLATE_TOP: f64 = -72.0;
pub const PLATE_SLOPE: f64 = 0.05;
pub const DOME_CENTER: [f64; 3] = [-60.0, -130.0, -80.0];
pub const DOME_RADIUS: f64 = 15.0;
/// A narrow, deep hole in the plate: deeper than the probe's stylus is long.
pub const HOLE_CENTER: [f64; 2] = [-38.0, -145.0];
pub const HOLE_DIAMETER: f64 = 6.0;
pub const HOLE_DEPTH: f64 = 20.0;

/// The plate's top at `x`, ignoring the dome.
pub fn plate_top(x: f64) -> f64 {
    PLATE_TOP + PLATE_SLOPE * (x - PLATE_MIN[0])
}

/// Whether the probe ball, with its bottom (the tool tip) at `tip`, touches a part. The block is
/// grown by the ball radius and tested against the ball centre, which is exact on faces and close
/// enough at edges.
fn touches_part(tip: [f64; 3]) -> bool {
    let r = BALL_RADIUS;
    let c = [tip[0], tip[1], tip[2] + r];
    let dist = |a: [f64; 2]| (c[0] - a[0]).hypot(c[1] - a[1]);
    let in_block = (PART_MIN[0] - r..=PART_MAX[0] + r).contains(&c[0])
        && (PART_MIN[1] - r..=PART_MAX[1] + r).contains(&c[1])
        && c[2] <= PART_TOP + r;
    let in_bore = dist(BORE_CENTER) < BORE_DIAMETER / 2.0 - r && c[2] > PART_TOP - BORE_DEPTH + r;
    let in_boss =
        dist(BOSS_CENTER) <= BOSS_DIAMETER / 2.0 + r && c[2] <= PART_TOP + BOSS_HEIGHT + r;
    // The plate's sloped top as a plane, and the dome as a whole sphere (its lower part is
    // inside the plate anyway).
    let in_plate = (PLATE_MIN[0] - r..=PLATE_MAX[0] + r).contains(&c[0])
        && (PLATE_MIN[1] - r..=PLATE_MAX[1] + r).contains(&c[1])
        && (c[2] - plate_top(c[0])) / PLATE_SLOPE.hypot(1.0) <= r;
    let d = DOME_CENTER;
    let in_hole =
        dist(HOLE_CENTER) < HOLE_DIAMETER / 2.0 - r && c[2] > plate_top(c[0]) - HOLE_DEPTH + r;
    let in_dome = ((c[0] - d[0]).powi(2) + (c[1] - d[1]).powi(2) + (c[2] - d[2]).powi(2)).sqrt()
        <= DOME_RADIUS + r;
    // The tilted block, in its own (unturned) frame.
    let (sin, cos) = TILTED_ANGLE.to_radians().sin_cos();
    let (dx, dy) = (c[0] - TILTED_CENTER[0], c[1] - TILTED_CENTER[1]);
    let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
    let half = TILTED_SIZE / 2.0 + r;
    let in_tilted = u.abs() <= half && v.abs() <= half && c[2] <= PART_TOP + r;
    (in_block && !in_bore) || in_boss || (in_plate && !in_hole) || in_dome || in_tilted
}

/// A `G38.x` waiting for its move to finish before reporting.
#[derive(Clone, Copy, Debug)]
enum ProbeReport {
    Hit([f64; 3]),
    Miss([f64; 3]),
    /// G38.2/G38.4 without the expected contact change.
    Fail,
}

/// Tool length offsets the simulated sensor measures.
fn tool_length(tool: i32) -> f64 {
    match tool {
        9999 => -0.023,
        8888 => -7.0,
        t if t < 0 => 0.0,
        t => -10.0 - f64::from(t) * 1.5,
    }
}

struct SdFile {
    data: Vec<u8>,
    date: &'static str,
}

#[derive(Default)]
struct Job {
    lines: Vec<String>,
    next: usize,
    bytes: usize,
    total: usize,
    elapsed: f64,
    suspended: bool,
    /// "Suspended" was reported once the queue drained.
    paused: bool,
    aborting: bool,
}

#[derive(Clone, Copy)]
struct Move {
    target: [f64; 5],
    /// mm/s along the path.
    rate: f64,
}

struct Machine {
    mpos: [f64; 5],
    wcs: [f64; 5],
    tool: i32,
    tlo: f64,
    moves: VecDeque<Move>,
    homing: bool,
    halt: Option<u32>,
    hold: bool,
    absolute: bool,
    feed: f64,
    current_feed: f64,
    spindle_on: bool,
    spindle_target: f64,
    spindle_rpm: f64,
    files: BTreeMap<String, SdFile>,
    job: Option<Job>,
    atc: Option<Atc>,
    /// Where the head was when the tool change started; it returns there afterwards.
    atc_return: [f64; 2],
    probe_report: Option<ProbeReport>,
    /// Where the probe last let go: within [`CHATTER`] of it, it flickers, and a probe move
    /// toward anything stops at once (the firmware doesn't debounce it).
    released_at: Option<[f64; 3]>,
    /// M119s answered, for leaving the probe out of some.
    m119s: u32,
    /// Feed and spindle overrides, %.
    feed_override: f64,
    spindle_override: f64,
    laser_mode: bool,
    laser_testing: bool,
    /// %.
    laser_scale: f64,
    light: bool,
    /// Power (%) of the PWM outputs, when on.
    power_fan: Option<f64>,
    spindle_fan: Option<f64>,
    ext_out: Option<f64>,
    /// Unsolicited output (job messages), sent to every client.
    outbox: Vec<Frame>,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            mpos: [-1.0, -1.0, -1.0, 0.0, 0.0],
            wcs: [-67.405, -124.957, -46.311, 0.0, 0.0],
            tool: 3,
            tlo: -18.211,
            moves: VecDeque::new(),
            homing: false,
            halt: None,
            hold: false,
            absolute: true,
            feed: 2000.0,
            current_feed: 0.0,
            spindle_on: false,
            spindle_target: 10000.0,
            spindle_rpm: 0.0,
            files: [
                ("/sd/gcodes/sim-square.nc", SQUARE),
                ("/sd/gcodes/Examples/sim circle.nc", CIRCLE),
                ("/sd/config.txt", CONFIG),
            ]
            .into_iter()
            .map(|(path, text)| {
                (
                    path.to_owned(),
                    SdFile {
                        data: text.as_bytes().to_vec(),
                        date: SEED_DATE,
                    },
                )
            })
            .collect(),
            job: None,
            atc: None,
            atc_return: [0.0; 2],
            probe_report: None,
            released_at: None,
            m119s: 0,
            feed_override: 100.0,
            spindle_override: 100.0,
            laser_mode: false,
            laser_testing: false,
            laser_scale: 100.0,
            light: true,
            power_fan: Some(34.0),
            spindle_fan: None,
            ext_out: None,
            outbox: vec![],
        }
    }
}

type Shared = Arc<Mutex<Machine>>;

impl Machine {
    fn moving(&self) -> bool {
        !self.moves.is_empty()
    }

    fn state(&self) -> &'static str {
        if self.halt.is_some() {
            "Alarm"
        } else if self.homing {
            "Home"
        } else if self.hold {
            "Hold"
        } else if let Some(Atc::Waiting(_)) = self.atc {
            "Tool"
        } else if let Some(job) = &self.job {
            match (job.suspended, self.moving()) {
                (false, _) => "Run",
                (true, true) => "Wait",
                (true, false) => "Pause",
            }
        } else if self.moving() || self.spindle_on {
            "Run"
        } else {
            "Idle"
        }
    }

    fn status(&self) -> String {
        let fmt = |v: &[f64]| {
            v.iter()
                .map(|x| format!("{x:.4}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let wpos: Vec<f64> = (0..5).map(|i| self.mpos[i] - self.wcs[i]).collect();
        let mut s = format!(
            "<{}|MPos:{}|WPos:{}|F:{:.1},{:.1},{:.1}|S:{:.1},{:.1},{:.1},0,25.0,25.5,0,0,0,0|T:{},{:.3},{}|L:{}, {}, {}, 0.0,{:.1}|C:3,1,0,{}",
            self.state(),
            fmt(&self.mpos),
            fmt(&wpos),
            self.current_feed,
            self.feed,
            self.feed_override,
            self.spindle_rpm,
            self.spindle_target,
            self.spindle_override,
            self.tool,
            self.tlo,
            self.atc.map_or(-1, Atc::target),
            u8::from(self.laser_mode),
            u8::from(self.laser_testing),
            u8::from(self.laser_testing),
            self.laser_scale,
            u8::from(self.absolute),
        );
        if let Some(h) = self.halt {
            s += &format!("|H:{h}");
        }
        if let Some(job) = &self.job {
            s += &format!(
                "|P:{},{},{}",
                job.next,
                (job.bytes * 100).checked_div(job.total).unwrap_or(100),
                job.elapsed as u64
            );
        }
        s += "|E:0,0,0,25,7613|OTA:0,0>\n";
        s
    }

    fn diagnose(&self) -> String {
        let pwm = |p: Option<f64>| format!("{},{:.0}", u8::from(p.is_some()), p.unwrap_or(0.0));
        format!(
            "{{S:{},{:.0},0,0,25,25|L:{},0|V:{}|F:{}|G:{},0,0,{}|T:0|C:1|E:0,0,0,0,0,1,0,0|P:0,0|I:0|RSSI:-40}}\n",
            u8::from(self.spindle_on),
            self.spindle_target,
            u8::from(self.laser_testing),
            pwm(self.power_fan),
            pwm(self.spindle_fan),
            u8::from(self.light),
            pwm(self.ext_out),
        )
    }

    fn tick(&mut self, dt: f64) {
        let ramp = SPINDLE_RAMP_RPM_S * dt;
        let want = if self.spindle_on {
            self.spindle_target
        } else {
            0.0
        };
        self.spindle_rpm += (want - self.spindle_rpm).clamp(-ramp, ramp);
        self.tool_change();
        self.report_probe();
        self.play(dt);

        let Some(m) = self.moves.front().copied() else {
            self.current_feed = 0.0;
            if self.homing {
                self.homing = false;
            }
            return;
        };
        let delta: Vec<f64> = (0..5).map(|i| m.target[i] - self.mpos[i]).collect();
        let dist = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
        let step = m.rate * dt;
        if dist <= step || dist < 1e-9 {
            self.mpos = m.target;
            self.moves.pop_front();
        } else {
            for (p, d) in self.mpos.iter_mut().zip(&delta) {
                *p += d / dist * step;
            }
        }
        self.current_feed = m.rate * 60.0;
    }

    /// The end position of everything queued.
    fn planned(&self) -> [f64; 5] {
        self.moves.back().map_or(self.mpos, |m| m.target)
    }

    /// Queue a move, or halt if it violates a soft limit (checked when queued, like the firmware).
    fn queue(&mut self, target: [f64; 5], rate: f64, out: &mut Vec<String>) {
        for i in 0..3 {
            if target[i] > LIMIT_MAX + 1e-9 || target[i] < LIMIT_MIN[i] {
                out.push(format!(
                    "error:Soft Endstop {} was exceeded - reset or $X or M999 required\n",
                    ['X', 'Y', 'Z'][i]
                ));
                self.halt_with(10);
                return;
            }
        }
        self.moves.push_back(Move { target, rate });
    }

    /// `G38.x` answers once its move is done, like the firmware: `[PRB:…]` then `ok`.
    fn report_probe(&mut self) {
        if self.moving() {
            return;
        }
        let Some(report) = self.probe_report.take() else {
            return;
        };
        let prb = |p: [f64; 3], hit: bool| {
            format!(
                "[PRB:{:.3},{:.3},{:.3}:{}]\r\nok\r\n",
                p[0],
                p[1],
                p[2],
                u8::from(hit)
            )
        };
        match report {
            ProbeReport::Hit(p) => self.outbox.push(info(prb(p, true))),
            ProbeReport::Miss(p) => self.outbox.push(info(prb(p, false))),
            ProbeReport::Fail => {
                self.halt_with(3);
                self.outbox.push(info("ALARM: Probe fail\r\n".into()));
            }
        }
    }

    /// `G38.2`–`G38.5` with a relative `delta` (as on the Z1): move until the probe touches (2, 3)
    /// or releases (4, 5) the simulated part.
    fn probe_move(&mut self, code: i64, delta: [Option<f64>; 5], out: &mut Vec<String>) {
        let start = self.planned();
        let toward = code == 382 || code == 383;
        // As the firmware: toward needs the probe free and away needs it triggered, or it's a
        // halt (with a message that says "triggered" either way).
        if touches_part([start[0], start[1], start[2]]) == toward {
            out.push("Error:ZProbe triggered before move, aborting command.\n".into());
            self.halt_with(3);
            return;
        }
        let d: Vec<f64> = (0..3).map(|i| delta[i].unwrap_or(0.0)).collect();
        let len = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        let at = |t: f64| {
            [
                start[0] + d[0] * t,
                start[1] + d[1] * t,
                start[2] + d[2] * t,
            ]
        };
        let steps = (len / 0.0005).ceil().max(1.0) as usize;
        let hit = |t: f64| touches_part(at(t)) == toward;
        // The first step that changes the contact, then bisect to the boundary.
        let stop = (0..=steps)
            .find(|&k| hit(k as f64 / steps as f64))
            .map(|k| {
                let (mut lo, mut hi) = (
                    (k.max(1) - 1) as f64 / steps as f64,
                    k as f64 / steps as f64,
                );
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if hit(mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                hi
            });
        // A flicker right where it let go stops a move toward at once.
        let chatter = toward
            && self.released_at.is_some_and(|r| {
                (0..3)
                    .map(|i| (r[i] - start[i]).powi(2))
                    .sum::<f64>()
                    .sqrt()
                    < CHATTER
            });
        let stop = if chatter { Some(0.0) } else { stop };
        let end = at(stop.unwrap_or(1.0));
        if !toward && stop.is_some() {
            self.released_at = Some(end);
        }
        let mut target = start;
        target[..3].copy_from_slice(&end);
        self.queue(target, (self.feed / 60.0).max(0.1), out);
        self.probe_report = Some(match (stop, code) {
            (Some(_), _) => ProbeReport::Hit(end),
            (None, 383 | 385) => ProbeReport::Miss(end),
            (None, _) => ProbeReport::Fail,
        });
    }

    /// Advance a tool change once the moves of its current step are done.
    fn tool_change(&mut self) {
        if self.moving() {
            return;
        }
        match self.atc {
            Some(Atc::Approach(t)) => self.atc = Some(Atc::Waiting(t)),
            Some(Atc::Measuring(t)) => {
                self.tool = t;
                self.tlo = tool_length(t);
                self.atc = None;
                self.outbox.push(info("Done ATC\r\n".into()));
                let mut p = self.mpos;
                p[2] = ATC_CLEARANCE_Z;
                self.moves.push_back(Move {
                    target: p,
                    rate: MAX_RATE[2],
                });
                p[..2].copy_from_slice(&self.atc_return);
                self.moves.push_back(Move {
                    target: p,
                    rate: MAX_RATE[0],
                });
            }
            _ => {}
        }
    }

    /// Feed job lines into the planner queue, and finish suspend/abort once the queue drains.
    fn play(&mut self, dt: f64) {
        // A tool change in the program holds the job until it's done.
        if self.atc.is_some() {
            return;
        }
        let Some(job) = &mut self.job else { return };
        if job.aborting {
            if self.moves.is_empty() {
                self.job = None;
                self.spindle_on = false;
                self.outbox
                    .push(info("ok\r\nAborted playing or paused file.\r\n".into()));
            }
            return;
        }
        if job.suspended {
            if !job.paused && self.moves.is_empty() {
                job.paused = true;
                self.outbox
                    .push(info("Suspended, resume to continue playing\r\n".into()));
            }
            return;
        }
        job.elapsed += dt;
        while self.moves.len() < JOB_LOOKAHEAD && self.halt.is_none() {
            let Some(job) = &mut self.job else { return };
            let Some(line) = job.lines.get(job.next).cloned() else {
                if self.moves.is_empty() {
                    self.job = None;
                }
                return;
            };
            job.next += 1;
            job.bytes += line.len() + 1;
            let code = strip_comments(&line);
            if code.is_empty() {
                continue;
            }
            for out in self.command(&code) {
                if out.starts_with("error") {
                    self.outbox.push(info(out));
                }
            }
        }
    }

    fn start_tool_change(&mut self, target: i32, out: &mut Vec<String>) {
        out.push(format!("Please change the tool to: T{target}\n"));
        self.atc_return = [self.mpos[0], self.mpos[1]];
        let mut p = self.planned();
        p[2] = ATC_CLEARANCE_Z;
        self.queue(p, MAX_RATE[2], out);
        p[..2].copy_from_slice(&ATC_CHANGE_XY);
        self.queue(p, MAX_RATE[0], out);
        self.atc = Some(Atc::Approach(target));
    }

    fn confirm_tool_change(&mut self, out: &mut Vec<String>) {
        let Some(Atc::Waiting(t)) = self.atc else {
            out.push("error:not waiting for a tool change\n".into());
            return;
        };
        // The tool is unknown until measured.
        self.tool = -1;
        let mut p = self.mpos;
        p[..2].copy_from_slice(&ATC_SENSOR_XY);
        self.queue(p, MAX_RATE[0], out);
        p[2] = ATC_SENSOR_Z - (tool_length(t) - tool_length(3));
        self.queue(p, 500.0 / 60.0, out);
        p[2] = -20.0;
        self.queue(p, MAX_RATE[2], out);
        self.atc = Some(Atc::Measuring(t));
    }

    /// `M491`: measure the tool that's in on the sensor again, no change.
    fn remeasure_tool(&mut self, out: &mut Vec<String>) {
        if self.tool < 0 {
            out.push("error:no tool to measure\n".into());
            return;
        }
        self.atc_return = [self.mpos[0], self.mpos[1]];
        let mut p = self.planned();
        p[2] = ATC_CLEARANCE_Z;
        self.queue(p, MAX_RATE[2], out);
        p[..2].copy_from_slice(&ATC_SENSOR_XY);
        self.queue(p, MAX_RATE[0], out);
        p[2] = ATC_SENSOR_Z - (tool_length(self.tool) - tool_length(3));
        self.queue(p, 500.0 / 60.0, out);
        p[2] = -20.0;
        self.queue(p, MAX_RATE[2], out);
        self.atc = Some(Atc::Measuring(self.tool));
    }

    fn halt_with(&mut self, reason: u32) {
        self.atc = None;
        self.probe_report = None;
        if self.job.take().is_some() {
            self.outbox.push(info("Aborted by halt\r\n".into()));
        }
        self.moves.clear();
        self.homing = false;
        self.hold = false;
        self.spindle_on = false;
        self.halt = Some(reason);
        self.current_feed = 0.0;
    }

    fn command(&mut self, line: &str) -> Vec<String> {
        let mut out = vec![];
        let line = line.trim();
        let words: Vec<&str> = line.split_whitespace().collect();
        let first = words.first().copied().unwrap_or_default();

        match first {
            "?" => {}
            "version" => out.push("version = 1.1.3.0.1.13\n".into()),
            "model" => out.push(format!("model = Z1, 3, 1, 0, {}\n", self.state())),
            "ftype" => out.push("ftype = nc\r\n".into()),
            "config-get" => {
                let key = words.last().copied().unwrap_or_default();
                let value = match key {
                    "soft_endstop.x_min" => Some(LIMIT_MIN[0]),
                    "soft_endstop.y_min" => Some(LIMIT_MIN[1]),
                    "soft_endstop.z_min" => Some(LIMIT_MIN[2]),
                    _ => None,
                };
                out.push(match value {
                    Some(v) => format!("sd: {key} is set to {v:.1}\n"),
                    None => format!("sd: {key} is not in config\n"),
                });
            }
            "reset" => {
                // The real controller reboots; the WiFi/USB side stays up, so the link survives.
                *self = Self {
                    mpos: self.mpos,
                    wcs: self.wcs,
                    files: std::mem::take(&mut self.files),
                    ..Self::default()
                };
                return vec!["Smoothie out. Peace. Rebooting in 5 seconds...\r\n".into()];
            }
            "$X" | "M999" => {
                self.halt = None;
                out.push("[Caution: Unlocked]\nok\n".into());
            }
            _ if self.halt.is_some() => out.push("!!\n".into()),
            "$J" => {
                let mut delta = [0.0; 5];
                let mut scale = 1.0;
                for w in &words[1..] {
                    let (axis, value) = w.split_at(1);
                    let Ok(v) = value.parse::<f64>() else {
                        out.push(format!("error:bad axis {axis}\n"));
                        return out;
                    };
                    match axis.to_ascii_uppercase().as_str() {
                        "F" => scale = v,
                        a => match "XYZAB".find(a) {
                            Some(i) => delta[i] = v,
                            None => {
                                out.push(format!("error:bad axis {a}\n"));
                                return out;
                            }
                        },
                    }
                }
                let rate = (0..5)
                    .filter(|&i| delta[i] != 0.0)
                    .map(|i| MAX_RATE[i])
                    .fold(f64::INFINITY, f64::min);
                if rate.is_infinite() {
                    out.push("error:no delta jog specified\n".into());
                    return out;
                }
                let mut target = self.planned();
                for i in 0..5 {
                    target[i] += delta[i];
                }
                // $J gets no reply on the real machine.
                self.queue(target, rate * scale, &mut out);
            }
            "$H" => {
                let mut target = self.planned();
                target[..3].copy_from_slice(&HOME);
                self.homing = true;
                self.queue(target, MAX_RATE[0], &mut out);
                out.push("ok\n".into());
            }
            _ => self.gcode(&words, &mut out),
        }
        out
    }

    fn gcode(&mut self, words: &[&str], out: &mut Vec<String>) {
        let mut g53 = false;
        let mut motion = None;
        let mut axes: [Option<f64>; 5] = [None; 5];
        let mut l = None;
        let mut p = None;
        let mut m = None;
        // G and M words as integer codes (G38.2 → 382) so they can be matched exactly.
        let code = |v: f64| (v * 10.0).round() as i64;
        let mut s_word = None;
        let mut t_word = None;
        for w in words {
            let (letter, value) = w.split_at(1);
            let value: f64 = value.parse().unwrap_or(f64::NAN);
            match letter.to_ascii_uppercase().as_str() {
                "G" => match code(value) {
                    530 => g53 = true,
                    924 => motion = Some(924),
                    900 => self.absolute = true,
                    910 => self.absolute = false,
                    // Arcs are simulated as straight moves to their end point.
                    c @ (0 | 10 | 100 | 382..=385) => motion = Some(c),
                    20 | 30 => motion = Some(10),
                    _ => {}
                },
                "M" => m = Some(code(value)),
                "S" => s_word = Some(value),
                "T" => t_word = Some(value),
                "F" => self.feed = value,
                "L" => l = Some(value),
                "P" => p = Some(value),
                a => {
                    if let Some(i) = "XYZAB".find(a) {
                        axes[i] = Some(value);
                    }
                }
            }
        }

        // On G92.4, S is the shrink flag, and on these M codes a percentage.
        let s_is_rpm = !matches!(m, Some(2200 | 2230 | 3250 | 8010 | 8110 | 8510));
        if let Some(s) = s_word
            && motion != Some(924)
            && s_is_rpm
        {
            self.spindle_target = s;
        }
        match m {
            Some(30) => self.spindle_on = true,
            Some(50) => self.spindle_on = false,
            Some(60) => {
                let target = t_word.map_or(self.tool, |t| t as i32);
                self.start_tool_change(target, out);
            }
            Some(4902) => self.confirm_tool_change(out),
            Some(4910) => self.remeasure_tool(out),
            // The probe's state, as the firmware appends it to the endstops'.
            // Every third time it's left out, as a Z1 was seen to do.
            Some(1190) => {
                self.m119s += 1;
                let p = self.planned();
                if self.m119s.is_multiple_of(3) {
                    out.push("X_max:0 Y_max:0 Z_max:0 A_min:1 pins- (XL)P0.24:0\n".into());
                } else {
                    out.push(format!(
                        "X_max:0 Y_max:0 Z_max:0 Probe: {}\n",
                        u8::from(touches_part([p[0], p[1], p[2]]))
                    ));
                }
            }
            Some(2200) => self.feed_override = s_word.unwrap_or(100.0).clamp(10.0, 1000.0),
            Some(2230) => self.spindle_override = s_word.unwrap_or(100.0).clamp(50.0, 200.0),
            Some(3250) => self.laser_scale = s_word.unwrap_or(100.0),
            Some(3210) if !self.laser_mode => {
                self.laser_mode = true;
                if self.tool != 8888 {
                    self.start_tool_change(8888, out);
                }
            }
            Some(3220) => {
                self.laser_mode = false;
                self.laser_testing = false;
            }
            Some(3230) => self.laser_testing = true,
            Some(3240) => self.laser_testing = false,
            Some(8010) => self.power_fan = Some(s_word.unwrap_or(100.0)),
            Some(8020) => self.power_fan = None,
            Some(8110) => self.spindle_fan = Some(s_word.unwrap_or(100.0)),
            Some(8120) => self.spindle_fan = None,
            Some(8210) => self.light = true,
            Some(8220) => self.light = false,
            Some(8510) => self.ext_out = Some(s_word.unwrap_or(100.0)),
            Some(8520) => self.ext_out = None,
            // M493.2 T<n>: set the current tool without measuring.
            Some(4932) => {
                if let Some(t) = t_word {
                    self.tool = t as i32;
                    self.tlo = tool_length(self.tool);
                }
            }
            _ => {}
        }

        match motion {
            Some(924) => {
                // G92.4: reset machine positions without moving. With S on A, "shrink": drop
                // whole turns so the work A is within ±360°.
                let shrink = words
                    .iter()
                    .any(|w| w.to_ascii_uppercase().starts_with('S'));
                for (i, value) in axes.iter().enumerate().take(4) {
                    let Some(value) = value else { continue };
                    if i == 3 && shrink {
                        let wa = self.mpos[3] - self.wcs[3];
                        if wa.abs() > 360.0 {
                            self.mpos[3] -= wa - wa % 360.0;
                        }
                    } else {
                        self.mpos[i] = *value;
                    }
                }
            }
            Some(100) => {
                if l == Some(20.0) && p == Some(0.0) {
                    for (i, value) in axes.iter().enumerate() {
                        if let Some(value) = value {
                            self.wcs[i] = self.mpos[i] - value;
                        }
                    }
                }
            }
            Some(c @ 382..=385) => {
                // Answered with `[PRB:…]` and `ok` once the move is done (`report_probe`).
                self.probe_move(c, axes, out);
                return;
            }
            Some(v) => {
                let mut target = self.planned();
                for (i, value) in axes.iter().enumerate() {
                    if let Some(value) = value {
                        target[i] = if g53 {
                            *value
                        } else if self.absolute {
                            value + self.wcs[i]
                        } else {
                            target[i] + value
                        };
                    }
                }
                let rate = if v == 0 {
                    MAX_RATE[0]
                } else {
                    self.feed / 60.0
                };
                self.queue(target, rate, out);
                if self.halt.is_some() {
                    return;
                }
            }
            None => {}
        }
        out.push("ok\n".into());
    }

    /// SD card and job commands, which reply in frames other than plain console text.
    fn file_command(&mut self, line: &str) -> Option<Vec<Frame>> {
        let mut words = line.split_whitespace();
        let cmd = words.next()?;
        let args: Vec<String> = words.map(unescape_arg).collect();
        let arg = args.last().cloned().unwrap_or_default();
        Some(match cmd {
            "ls" => {
                let dir = format!("{}/", arg.trim_end_matches('/'));
                let mut entries: BTreeMap<String, String> = BTreeMap::new();
                for (path, file) in &self.files {
                    let Some(rest) = path.strip_prefix(&dir) else {
                        continue;
                    };
                    let escaped = |n: &str| hj_makera::command::escape_arg(n);
                    match rest.split_once('/') {
                        Some((sub, _)) => {
                            entries.insert(sub.into(), format!("{}/ 0 {SEED_DATE}", escaped(sub)));
                        }
                        None => {
                            entries.insert(
                                rest.into(),
                                format!("{}  {} {}", escaped(rest), file.data.len(), file.date),
                            );
                        }
                    }
                }
                let lines: Vec<String> = entries.into_values().collect();
                let mut frames: Vec<Frame> = lines
                    .chunks(8)
                    .map(|c| Frame::new(FrameType::LOAD_INFO, format!("{}\r\n", c.join("\r\n"))))
                    .collect();
                frames.push(Frame::new(
                    FrameType::LOAD_FINISH,
                    *b"Load directory finished.\r\n",
                ));
                frames
            }
            "rm" => match self.files.remove(&arg) {
                Some(_) => vec![Frame::new(FrameType::LOAD_FINISH, *b"ok\r\n")],
                None => vec![Frame::new(
                    FrameType::LOAD_ERROR,
                    format!("Could not delete {arg} \r\n"),
                )],
            },
            "play" => {
                if self.job.is_some() || self.moving() || self.halt.is_some() {
                    return Some(vec![info("error:busy\r\n".into())]);
                }
                let Some(file) = self.files.get(&arg) else {
                    return Some(vec![info(format!("error:Could not open {arg}\r\n"))]);
                };
                let text = String::from_utf8_lossy(&file.data);
                self.job = Some(Job {
                    lines: text.lines().map(str::to_owned).collect(),
                    total: file.data.len(),
                    ..Job::default()
                });
                vec![info(format!("  File size {}\r\n", file.data.len()))]
            }
            "suspend" => match &mut self.job {
                Some(job) if !job.suspended && !job.aborting => {
                    job.suspended = true;
                    vec![info(
                        "Suspending , waiting for queue to empty...\r\n".into(),
                    )]
                }
                _ => vec![info("error:not playing\r\n".into())],
            },
            "resume" => match &mut self.job {
                Some(job) if job.suspended && !job.aborting => {
                    job.suspended = false;
                    job.paused = false;
                    vec![info(
                        "Resuming playing...\r\nPlaying file resumed\r\n".into(),
                    )]
                }
                _ => vec![info("error:not suspended\r\n".into())],
            },
            "abort" => match &mut self.job {
                Some(job) => {
                    job.aborting = true;
                    vec![]
                }
                None => vec![info("ok\r\n".into())],
            },
            _ => return None,
        })
    }

    fn realtime(&mut self, byte: u8) -> Vec<String> {
        match byte {
            0x18 => {
                self.halt_with(1);
                vec!["ALARM: Abort during cycle\r\n".into()]
            }
            b'!' => {
                self.hold = true;
                vec![]
            }
            b'~' => {
                self.hold = false;
                vec![]
            }
            _ => vec![],
        }
    }
}

/// A running simulator. Dropping it doesn't stop the background tasks; they end with the runtime.
pub struct Simulator {
    pub addr: SocketAddr,
}

/// Start a simulator on `addr` (use port 0 for an ephemeral port) on the current runtime.
pub async fn spawn(addr: SocketAddr) -> std::io::Result<Simulator> {
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let machine: Shared = Arc::default();

    let (broadcast, _) = broadcast::channel(256);
    let ticker = machine.clone();
    let outbox = broadcast.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            let frames = {
                let mut m = ticker.lock().unwrap();
                m.tick(TICK.as_secs_f64());
                std::mem::take(&mut m.outbox)
            };
            for f in frames {
                let _ = outbox.send(f);
            }
        }
    });

    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            tracing::debug!("simulator: client {peer}");
            let machine = machine.clone();
            let events = broadcast.subscribe();
            tokio::spawn(async move {
                if let Err(e) = serve(stream, machine, events).await {
                    tracing::debug!("simulator: client {peer} ended: {e}");
                }
            });
        }
    });
    Ok(Simulator { addr })
}

/// An upload in progress on one connection. The machine pulls blocks one at a time.
struct Upload {
    path: String,
    md5: Option<String>,
    blocks: u32,
    data: Vec<u8>,
}

impl Upload {
    fn frame(&mut self, frame: &Frame, machine: &Shared) -> Vec<Frame> {
        let request = |seq: u32| Frame::new(FrameType::FILE_DATA, seq.to_be_bytes());
        match frame.kind {
            FrameType::FILE_MD5 => {
                self.md5 = Some(String::from_utf8_lossy(&frame.payload).trim().to_owned());
                vec![Frame::new(FrameType::FILE_VIEW, [])]
            }
            FrameType::FILE_VIEW if frame.payload.len() >= 6 => {
                let p = &frame.payload;
                self.blocks = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                if self.blocks == 0 {
                    return self.finish(machine);
                }
                vec![request(1)]
            }
            FrameType::FILE_DATA if frame.payload.len() >= 4 => {
                let p = &frame.payload;
                let seq = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                self.data.extend_from_slice(&p[4..]);
                if seq < self.blocks {
                    vec![request(seq + 1)]
                } else {
                    self.finish(machine)
                }
            }
            _ => vec![],
        }
    }

    fn finish(&mut self, machine: &Shared) -> Vec<Frame> {
        if self.md5.as_deref() != Some(md5_hex(&self.data).as_str()) {
            return vec![Frame::new(FrameType::FILE_CANCEL, *b"md5 mismatch\r\n")];
        }
        machine.lock().unwrap().files.insert(
            self.path.clone(),
            SdFile {
                data: std::mem::take(&mut self.data),
                date: UPLOAD_DATE,
            },
        );
        vec![Frame::new(FrameType::FILE_END, *b"ok\r\n")]
    }
}

async fn serve(
    stream: TcpStream,
    machine: Shared,
    mut events: broadcast::Receiver<Frame>,
) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let forward = tx.clone();
    tokio::spawn(async move {
        while let Ok(frame) = events.recv().await {
            if forward.send(frame).is_err() {
                return;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_all(&frame.encode()).await.is_err() {
                return;
            }
        }
    });
    let mut upload: Option<Upload> = None;
    // A download in progress: the path and the file's contents.
    let mut download: Option<(String, Vec<u8>)> = None;

    let mut decoder = Decoder::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        decoder.push(&buf[..n]);
        while let Some(item) = decoder.next() {
            let Ok(frame) = item else { continue };
            if frame.kind == FrameType::FILE_START
                && let Some(path) = String::from_utf8_lossy(&frame.payload)
                    .trim()
                    .strip_prefix("download ")
                    .map(unescape_arg)
            {
                let data = machine
                    .lock()
                    .unwrap()
                    .files
                    .get(&path)
                    .map(|f| f.data.clone());
                let reply = match data {
                    Some(data) => {
                        let md5 = md5_hex(&data);
                        download = Some((path, data));
                        Frame::new(FrameType::FILE_MD5, md5.into_bytes())
                    }
                    None => Frame::new(
                        FrameType::FILE_CANCEL,
                        format!("Error: failed to open file [{path}]!\r\n"),
                    ),
                };
                let _ = tx.send(reply);
                continue;
            }
            if let Some((path, data)) = &download
                && (FrameType::FILE_MD5.0..=FrameType::FILE_RETRY.0).contains(&frame.kind.0)
            {
                let blocks = data.len().div_ceil(DOWNLOAD_BLOCK) as u32;
                let replies = match frame.kind {
                    FrameType::FILE_VIEW => {
                        let mut p = blocks.to_be_bytes().to_vec();
                        p.extend_from_slice(&(DOWNLOAD_BLOCK as u16).to_be_bytes());
                        vec![Frame::new(FrameType::FILE_VIEW, p)]
                    }
                    FrameType::FILE_DATA if frame.payload.len() >= 4 => {
                        let p = &frame.payload;
                        let seq = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                        let start =
                            (seq.saturating_sub(1) as usize * DOWNLOAD_BLOCK).min(data.len());
                        let end = (start + DOWNLOAD_BLOCK).min(data.len());
                        let mut p = seq.to_be_bytes().to_vec();
                        p.extend_from_slice(&data[start..end]);
                        vec![Frame::new(FrameType::FILE_DATA, p)]
                    }
                    FrameType::FILE_END => vec![
                        Frame::new(FrameType::FILE_END, *b"ok\r\n"),
                        info(format!("Info: download success: {path}.\r\n")),
                    ],
                    FrameType::FILE_CANCEL => {
                        vec![Frame::new(
                            FrameType::FILE_CANCEL,
                            *b"Info: canceled by remote!\r\n",
                        )]
                    }
                    _ => vec![],
                };
                if matches!(frame.kind, FrameType::FILE_END | FrameType::FILE_CANCEL) {
                    download = None;
                }
                for r in replies {
                    let _ = tx.send(r);
                }
                continue;
            }
            if frame.kind == FrameType::FILE_START {
                let cmd = String::from_utf8_lossy(&frame.payload);
                let path = cmd.trim().strip_prefix("upload ").map(unescape_arg);
                upload = path.map(|path| Upload {
                    path,
                    md5: None,
                    blocks: 0,
                    data: vec![],
                });
                continue;
            }
            if (FrameType::FILE_MD5.0..=FrameType::FILE_RETRY.0).contains(&frame.kind.0) {
                let replies = match &mut upload {
                    Some(up) => up.frame(&frame, &machine),
                    None => vec![],
                };
                if replies
                    .iter()
                    .any(|r| matches!(r.kind, FrameType::FILE_END | FrameType::FILE_CANCEL))
                {
                    upload = None;
                }
                for r in replies {
                    let _ = tx.send(r);
                }
                continue;
            }
            let mut m = machine.lock().unwrap();
            let replies: Vec<Frame> = match frame.kind {
                FrameType::CTRL_SINGLE if frame.payload == b"?" => {
                    vec![Frame::new(FrameType::STATUS, m.status().into_bytes())]
                }
                FrameType::CTRL_SINGLE => frame
                    .payload
                    .first()
                    .map(|b| m.realtime(*b))
                    .unwrap_or_default()
                    .into_iter()
                    .map(info)
                    .collect(),
                FrameType::CTRL_MULTI => {
                    let line = String::from_utf8_lossy(&frame.payload).into_owned();
                    if line.trim() == "diagnose" {
                        vec![Frame::new(FrameType::DIAGNOSE, m.diagnose().into_bytes())]
                    } else if let Some(frames) = m.file_command(&line) {
                        frames
                    } else {
                        m.command(&line).into_iter().map(info).collect()
                    }
                }
                _ => vec![],
            };
            drop(m);
            for r in replies {
                let _ = tx.send(r);
            }
        }
    }
}

/// A job line without comments, as the firmware's player would pass it on.
fn strip_comments(line: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in line.chars() {
        match c {
            ';' if depth == 0 => break,
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn info(text: String) -> Frame {
    Frame::new(FrameType::NORMAL_INFO, text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hj_makera::status::Status;

    fn run(m: &mut Machine, secs: f64) {
        for _ in 0..(secs / 0.02) as usize {
            m.tick(0.02);
        }
    }

    #[test]
    fn status_parses() {
        let m = Machine::default();
        let s = Status::parse(&m.status()).unwrap();
        assert_eq!(s.machine_pos.x(), -1.0);
        assert_eq!(s.machine.unwrap().model_name(), "Z1");
    }

    #[test]
    fn jog_is_relative_scaled_and_silent() {
        let mut m = Machine::default();
        assert!(m.command("$J X-10 F0.1").is_empty());
        assert_eq!(m.state(), "Run");
        run(&mut m, 4.0); // 10 mm at 2 mm/s = 5 s
        assert!(m.mpos[0] > -11.0 && m.mpos[0] < -1.0);
        run(&mut m, 2.0);
        assert_eq!(m.mpos[0], -11.0);
        assert_eq!(m.state(), "Idle");
    }

    #[test]
    fn soft_limit_halts() {
        let mut m = Machine::default();
        let out = m.command("$J X0.5");
        assert!(out[0].starts_with("error:Soft Endstop X"));
        assert_eq!(m.state(), "Alarm");
        assert_eq!(m.command("G1 X1"), vec!["!!\n"]);
        m.command("$X");
        assert_eq!(m.state(), "Idle");
    }

    #[test]
    fn halt_stops_motion_and_keeps_position() {
        let mut m = Machine::default();
        m.command("$J X-20 F0.1");
        run(&mut m, 1.0);
        let x = m.mpos[0];
        assert_eq!(m.realtime(0x18), vec!["ALARM: Abort during cycle\r\n"]);
        run(&mut m, 1.0);
        assert_eq!(m.mpos[0], x);
        assert_eq!(m.halt, Some(1));
    }

    #[test]
    fn g92_4_shrinks_a() {
        let mut m = Machine::default();
        m.command("$J A725 F1");
        run(&mut m, 30.0);
        assert_eq!(m.mpos[3], 725.0);
        m.command("G92.4 A0 S0");
        assert_eq!(m.mpos[3], 5.0);
        m.command("G10 L20 P0 A0");
        let s = Status::parse(&m.status()).unwrap();
        assert_eq!(s.work_pos.a(), 0.0);
    }

    #[test]
    fn g10_sets_work_zero() {
        let mut m = Machine::default();
        m.command("G10 L20 P0 X0 Y0");
        let s = Status::parse(&m.status()).unwrap();
        assert_eq!(s.work_pos.x(), 0.0);
        assert_eq!(s.work_pos.y(), 0.0);
    }

    fn texts(frames: &[Frame]) -> Vec<(FrameType, String)> {
        frames
            .iter()
            .map(|f| (f.kind, String::from_utf8_lossy(&f.payload).into_owned()))
            .collect()
    }

    #[test]
    fn lists_and_deletes_files() {
        let mut m = Machine::default();
        let ls = texts(&m.file_command("ls -s /sd/gcodes").unwrap());
        assert_eq!(ls[0].0, FrameType::LOAD_INFO);
        assert_eq!(
            ls[0].1,
            format!(
                "Examples/ 0 {SEED_DATE}\r\nsim-square.nc  {} {SEED_DATE}\r\n",
                SQUARE.len()
            )
        );
        assert_eq!(ls[1].0, FrameType::LOAD_FINISH);
        let sub = texts(&m.file_command("ls -s /sd/gcodes/Examples").unwrap());
        assert!(sub[0].1.starts_with("sim\x01circle.nc  "));

        let rm = texts(
            &m.file_command("rm /sd/gcodes/Examples/sim\x01circle.nc")
                .unwrap(),
        );
        assert_eq!(rm, vec![(FrameType::LOAD_FINISH, "ok\r\n".into())]);
        let rm = texts(&m.file_command("rm /sd/gcodes/nope.nc").unwrap());
        assert_eq!(rm[0].0, FrameType::LOAD_ERROR);
    }

    #[test]
    fn plays_suspends_resumes_and_aborts() {
        let mut m = Machine::default();
        // Work zero at machine X-100 Y-100 Z-50.
        m.command("G10 L20 P0 X99 Y99 Z49");
        let out = texts(&m.file_command("play /sd/gcodes/sim-square.nc").unwrap());
        assert_eq!(out[0].1, format!("  File size {}\r\n", SQUARE.len()));
        run(&mut m, 2.0);
        assert_eq!(m.state(), "Run");
        assert!(Status::parse(&m.status()).unwrap().progress.is_some());

        m.file_command("suspend");
        run(&mut m, 20.0);
        assert_eq!(m.state(), "Pause");
        assert!(
            texts(&m.outbox)
                .iter()
                .any(|t| t.1.starts_with("Suspended"))
        );
        let paused_at = m.mpos;
        run(&mut m, 1.0);
        assert_eq!(m.mpos, paused_at);

        m.file_command("resume");
        run(&mut m, 1.0);
        assert_eq!(m.state(), "Run");
        m.file_command("abort");
        run(&mut m, 30.0);
        assert!(m.job.is_none());
        assert!(!m.spindle_on);
        assert!(
            texts(&m.outbox)
                .iter()
                .any(|t| t.1.contains("Aborted playing"))
        );
        assert_eq!(m.state(), "Idle");
    }

    #[test]
    fn job_runs_to_completion() {
        let mut m = Machine::default();
        // Work zero at machine X-100 Y-100 Z-50.
        m.command("G10 L20 P0 X99 Y99 Z49");
        m.file_command("play /sd/gcodes/sim-square.nc");
        run(&mut m, 60.0);
        assert!(m.job.is_none());
        let s = Status::parse(&m.status()).unwrap();
        assert!(s.progress.is_none());
        // Back at X0 Y0, Z5 in work coordinates.
        assert_eq!(
            (s.work_pos.x(), s.work_pos.y(), s.work_pos.z()),
            (0.0, 0.0, 5.0)
        );
    }

    #[test]
    fn remeasures_the_tool() {
        let mut m = Machine::default();
        m.command("M493.2 T9999");
        let out = m.command("M491");
        assert!(!out.iter().any(|l| l.starts_with("error")), "{out:?}");
        run(&mut m, 60.0);
        assert_eq!(m.state(), "Idle");
        assert_eq!(m.tool, 9999);
        assert!(
            m.outbox
                .iter()
                .any(|f| String::from_utf8_lossy(&f.payload).contains("Done ATC"))
        );
    }

    #[test]
    fn manual_tool_change() {
        let mut m = Machine::default();
        let out = m.command("M6 T2");
        assert_eq!(out[0], "Please change the tool to: T2\n");
        run(&mut m, 30.0);
        assert_eq!(m.state(), "Tool");
        let s = Status::parse(&m.status()).unwrap();
        assert_eq!(s.tool.unwrap().target, Some(2));

        m.command("M490.2");
        assert_eq!(m.tool, -1);
        run(&mut m, 60.0);
        assert_eq!(m.state(), "Idle");
        assert_eq!(m.tool, 2);
        assert_eq!(m.tlo, tool_length(2));
        assert!(texts(&m.outbox).iter().any(|t| t.1 == "Done ATC\r\n"));
        // Back over where it started.
        assert_eq!((m.mpos[0], m.mpos[1]), (-1.0, -1.0));
    }

    #[test]
    fn tool_change_holds_a_job() {
        let mut m = Machine::default();
        m.command("G10 L20 P0 X99 Y99 Z49");
        m.files.insert(
            "/sd/gcodes/tc.nc".into(),
            SdFile {
                data: b"G0 Z5\nM6 T1\nG0 X10\n".to_vec(),
                date: SEED_DATE,
            },
        );
        m.file_command("play /sd/gcodes/tc.nc");
        run(&mut m, 30.0);
        assert_eq!(m.state(), "Tool");
        let at = m.job.as_ref().unwrap().next;
        run(&mut m, 5.0);
        assert_eq!(m.job.as_ref().unwrap().next, at, "the job waits");
        m.command("M490.2");
        run(&mut m, 90.0);
        assert!(m.job.is_none());
        assert_eq!(m.tool, 1);
    }

    /// Run a probe move to completion and return what it reported.
    fn probe(m: &mut Machine, line: &str) -> String {
        m.outbox.clear();
        m.command(line);
        run(m, 60.0);
        texts(&m.outbox).into_iter().map(|t| t.1).collect()
    }

    fn move_to(m: &mut Machine, p: [f64; 3]) {
        m.command(&format!("G53 G0 X{} Y{} Z{}", p[0], p[1], p[2]));
        run(m, 60.0);
    }

    #[test]
    fn probes_the_part() {
        let mut m = Machine::default();
        // Top surface.
        move_to(&mut m, [-120.0, -120.0, -50.0]);
        assert_eq!(
            probe(&mut m, "G38.2 Z-20 F100"),
            "[PRB:-120.000,-120.000,-60.000:1]\r\nok\r\n"
        );
        // Release going up.
        assert!(probe(&mut m, "G38.4 Z2 F50").ends_with(":1]\r\nok\r\n"));
        assert!(m.mpos[2] > -60.0);

        // Left edge, 3 mm below the top: the ball centre stops a radius short.
        move_to(&mut m, [-160.0, -120.0, -63.0]);
        assert!(probe(&mut m, "G38.2 X20 F100").starts_with("[PRB:-151.000,"));

        // Inside the bore.
        move_to(&mut m, [BORE_CENTER[0], BORE_CENTER[1], -50.0]);
        move_to(&mut m, [BORE_CENTER[0], BORE_CENTER[1], -63.0]);
        assert!(probe(&mut m, "G38.2 X20 F100").starts_with("[PRB:-131.000,"));

        // A protected move that hits nothing reports 0; G38.2 without contact halts.
        move_to(&mut m, [-50.0, -50.0, -50.0]);
        assert!(probe(&mut m, "G38.3 X5 F500").ends_with(":0]\r\nok\r\n"));
        assert_eq!(probe(&mut m, "G38.2 X5 F500"), "ALARM: Probe fail\r\n");
        assert_eq!(m.state(), "Alarm");
    }
}
