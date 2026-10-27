//! A simulated Makera Z1 speaking the framed protocol over TCP.
//!
//! It reproduces the behaviour measured on a real machine (see `docs/research/z1-protocol.md`),
//! not the full firmware: status reports in the Z1 format, relative `$J` jogs with speed as a
//! fraction of the axis max rate and no reply, queued moves, homing, halt and `$X`, soft limits at
//! the homed position, `G10 L20` work offsets, `G53`/`G90`/`G91` moves, spindle on/off, and the
//! `version`/`model`/`ftype`/`diagnose` queries.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hj_makera::frame::{Decoder, Frame, FrameType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_millis(20);
/// Max rates in mm/s (deg/s for A, B), as measured: X/Y 1200 mm/min, Z 600, A ~1800 °/min.
const MAX_RATE: [f64; 5] = [20.0, 20.0, 10.0, 30.0, 30.0];
/// Soft limits on X, Y, Z (machine coordinates). Max is the homed position.
/// As configured on the Z1 (`soft_endstop.*_min`).
const LIMIT_MIN: [f64; 3] = [-207.0, -206.0, -102.0];
const LIMIT_MAX: f64 = -1.0;
const HOME: [f64; 3] = [-1.0, -1.0, -1.0];
const SPINDLE_RAMP_RPM_S: f64 = 1500.0;

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
            "<{}|MPos:{}|WPos:{}|F:{:.1},{:.1},100.0|S:{:.1},{:.1},100.0,0,25.0,25.5,0,0,0,0|T:{},{:.3},-1|L:0, 0, 0, 0.0,100.0|C:3,1,0,{}",
            self.state(),
            fmt(&self.mpos),
            fmt(&wpos),
            self.current_feed,
            self.feed,
            self.spindle_rpm,
            self.spindle_target,
            self.tool,
            self.tlo,
            u8::from(self.absolute),
        );
        if let Some(h) = self.halt {
            s += &format!("|H:{h}");
        }
        s += "|E:0,0,0,25,7613|OTA:0,0>\n";
        s
    }

    fn tick(&mut self, dt: f64) {
        let ramp = SPINDLE_RAMP_RPM_S * dt;
        let want = if self.spindle_on {
            self.spindle_target
        } else {
            0.0
        };
        self.spindle_rpm += (want - self.spindle_rpm).clamp(-ramp, ramp);

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

    fn halt_with(&mut self, reason: u32) {
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
                let wcs = self.wcs;
                let mpos = self.mpos;
                *self = Self {
                    mpos,
                    wcs,
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
        for w in words {
            let (letter, value) = w.split_at(1);
            let value: f64 = value.parse().unwrap_or(f64::NAN);
            match letter.to_ascii_uppercase().as_str() {
                "G" => match code(value) {
                    530 => g53 = true,
                    924 => motion = Some(924),
                    900 => self.absolute = true,
                    910 => self.absolute = false,
                    c @ (0 | 10 | 100) => motion = Some(c),
                    _ => {}
                },
                "M" => m = Some(code(value)),
                "S" => s_word = Some(value),
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

        // On G92.4, S is the shrink flag, not a spindle speed.
        if let Some(s) = s_word
            && motion != Some(924)
        {
            self.spindle_target = s;
        }
        match m {
            Some(30) => self.spindle_on = true,
            Some(50) => self.spindle_on = false,
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

    let ticker = machine.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            ticker.lock().unwrap().tick(TICK.as_secs_f64());
        }
    });

    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            tracing::debug!("simulator: client {peer}");
            let machine = machine.clone();
            tokio::spawn(async move {
                if let Err(e) = serve(stream, machine).await {
                    tracing::debug!("simulator: client {peer} ended: {e}");
                }
            });
        }
    });
    Ok(Simulator { addr })
}

async fn serve(stream: TcpStream, machine: Shared) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_all(&frame.encode()).await.is_err() {
                return;
            }
        }
    });

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
                        vec![Frame::new(
                            FrameType::DIAGNOSE,
                            format!(
                                "{{S:{},{:.0},0,0,25,25|L:0,0|V:1,34|F:0,0|G:1,0,0,0,0|T:0|C:1|E:0,0,0,0,0,1,0,0|P:0,0|I:0|RSSI:-40}}\n",
                                u8::from(m.spindle_on),
                                m.spindle_target
                            )
                            .into_bytes(),
                        )]
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
}
