//! G-code analysis for previews and pre-flight checks: interprets a program roughly the way the Z1
//! runs it and produces the toolpath in work coordinates, its bounds, a time estimate and
//! diagnostics. It doesn't need to match the firmware exactly, only well enough to show what the
//! program will do and to catch mistakes before it runs.
//!
//! Positions the program can't know (before the first absolute move on an axis, after `G53`, `G28`,
//! probing or a tool change) are tracked as unknown, and no path is drawn from them.

mod lex;
pub mod reference;
pub mod transform;

use serde::Serialize;
use ts_rs::TS;

use crate::lex::{Line, Word};

/// Vertex kinds in [`Analysis::kinds`]: how the head got to the vertex from the previous one.
pub const JUMP: u8 = 0;
pub const RAPID: u8 = 1;
pub const FEED: u8 = 2;

/// For the time estimate, mm/min: X, Y, Z max rates as measured on the Z1.
const RAPID_RATE: [f64; 3] = [1200.0, 1200.0, 600.0];
/// Max deviation of arc chords from the true arc, mm.
const ARC_TOLERANCE: f64 = 0.005;
const MAX_ARC_SEGMENTS: f64 = 10_000.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Analysis {
    /// Vertices, flattened `x, y, z` in work coordinates (mm).
    pub points: Vec<f32>,
    /// One per vertex: [`JUMP`], [`RAPID`] or [`FEED`].
    pub kinds: Vec<u8>,
    /// One per vertex: the 1-based source line of the move ending there.
    pub lines: Vec<u32>,
    /// Everywhere the head goes, rapids included.
    pub bounds: Option<Bounds>,
    /// Where it cuts (feed moves only).
    pub cut_bounds: Option<Bounds>,
    pub diagnostics: Vec<Diagnostic>,
    /// Tools used by `M6`, in order of first use.
    pub tools: Vec<i32>,
    pub line_count: u32,
    /// Rough: rapids at the axis max rates, no acceleration.
    pub estimated_secs: f64,
    /// The program moves the A axis, which the preview doesn't show.
    pub uses_a: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    fn point(p: [f64; 3]) -> Self {
        Self { min: p, max: p }
    }

    fn add(this: &mut Option<Self>, p: [f64; 3]) {
        match this {
            Some(b) => {
                for (i, v) in p.into_iter().enumerate() {
                    b.min[i] = b.min[i].min(v);
                    b.max[i] = b.max[i].max(v);
                }
            }
            None => *this = Some(Self::point(p)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Diagnostic {
    /// 1-based.
    pub line: u32,
    pub severity: Severity,
    pub message: String,
}

pub fn analyze(text: &str) -> Analysis {
    let mut m = Interpreter::default();
    for (i, line) in text.lines().enumerate() {
        m.line_no = i as u32 + 1;
        match lex::line(line) {
            Ok(Line::Words(words)) if !words.is_empty() => {
                if let Err(e) = m.block(&words) {
                    m.diag(Severity::Error, e);
                }
            }
            Ok(Line::Words(_)) => {}
            Ok(Line::Command) => m.diag(
                Severity::Info,
                "console command, not G-code: sent to the machine as is".into(),
            ),
            Err(e) => m.diag(Severity::Error, e),
        }
    }
    m.out.line_count = text.lines().count() as u32;
    m.out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Motion {
    Rapid,
    Linear,
    Cw,
    Ccw,
    /// G81, G82, G83, G73.
    Drill,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Plane {
    Xy,
    Zx,
    Yz,
}

impl Plane {
    /// Axis indices (first, second, normal) and the offset letters for the first two.
    fn axes(self) -> ([usize; 3], [char; 2]) {
        match self {
            Self::Xy => ([0, 1, 2], ['I', 'J']),
            Self::Zx => ([2, 0, 1], ['K', 'I']),
            Self::Yz => ([1, 2, 0], ['J', 'K']),
        }
    }
}

struct Interpreter {
    line_no: u32,
    /// mm per program unit.
    units: f64,
    absolute: bool,
    arc_absolute: bool,
    plane: Plane,
    motion: Option<Motion>,
    /// mm/min.
    feed: f64,
    /// Work coordinates, X Y Z A.
    pos: [f64; 4],
    known: [bool; 4],
    /// G92 offsets: work = program + offset.
    offset: [f64; 4],
    tool: Option<i32>,
    /// Canned cycle: R and Z levels (work coordinates), and the Z when the cycle started.
    cycle_r: Option<f64>,
    cycle_z: Option<f64>,
    cycle_initial: Option<f64>,
    retract_to_r: bool,
    /// Where the last vertex was put, to know when a jump is needed.
    pen: Option<[f64; 3]>,
    warned: Vec<&'static str>,
    out: Analysis,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self {
            line_no: 0,
            units: 1.0,
            absolute: true,
            arc_absolute: false,
            plane: Plane::Xy,
            motion: None,
            feed: 0.0,
            pos: [0.0; 4],
            known: [false; 4],
            offset: [0.0; 4],
            tool: None,
            cycle_r: None,
            cycle_z: None,
            cycle_initial: None,
            retract_to_r: false,
            pen: None,
            warned: vec![],
            out: Analysis::default(),
        }
    }
}

/// G and M numbers as integers (G38.2 → 382) so they can be matched exactly.
fn code(v: f64) -> i32 {
    (v * 10.0).round() as i32
}

struct Block {
    g: Vec<i32>,
    m: Vec<i32>,
    params: [Option<f64>; 26],
}

impl Block {
    fn get(&self, letter: char) -> Option<f64> {
        self.params[(letter as u8 - b'A') as usize]
    }
}

impl Interpreter {
    fn diag(&mut self, severity: Severity, message: String) {
        self.out.diagnostics.push(Diagnostic {
            line: self.line_no,
            severity,
            message,
        });
    }

    /// Like [`Self::diag`], but only the first time for each `key`.
    fn diag_once(&mut self, key: &'static str, severity: Severity, message: &str) {
        if !self.warned.contains(&key) {
            self.warned.push(key);
            self.diag(severity, message.into());
        }
    }

    fn block(&mut self, words: &[Word]) -> Result<(), String> {
        let mut b = Block {
            g: vec![],
            m: vec![],
            params: [None; 26],
        };
        for w in words {
            match w.letter {
                'G' => b.g.push(code(w.value)),
                'M' => b.m.push(code(w.value)),
                'N' | 'O' => {}
                l @ ('X' | 'Y' | 'Z' | 'A' | 'B' | 'C' | 'I' | 'J' | 'K' | 'R' | 'F' | 'S'
                | 'T' | 'P' | 'Q' | 'L' | 'H' | 'D' | 'E') => {
                    let slot = &mut b.params[(l as u8 - b'A') as usize];
                    if slot.is_some() {
                        return Err(format!("{l} appears twice"));
                    }
                    *slot = Some(w.value);
                }
                l => self.diag(Severity::Warning, format!("unknown word {l}")),
            }
        }
        if b.get('B').is_some() || b.get('C').is_some() {
            self.diag(
                Severity::Warning,
                "the Z1 has no B or C axis; ignored".into(),
            );
        }

        if let Some(f) = b.get('F') {
            self.feed = f * self.units;
        }
        if let Some(t) = b.get('T') {
            self.tool = Some(t as i32);
        }
        for &m in &b.m {
            if !reference::known('M', m) {
                self.diag(
                    Severity::Warning,
                    format!(
                        "unknown code M{}: the Z1 may reject or ignore it",
                        m as f64 / 10.0
                    ),
                );
            }
        }
        if b.m.contains(&60) {
            match self.tool {
                Some(t) => {
                    if !self.out.tools.contains(&t) {
                        self.out.tools.push(t);
                    }
                }
                None => return Err("M6 without a tool number (T)".into()),
            }
            // The tool change measures the tool and returns over the start point at clearance
            // height.
            self.known[2] = false;
        }

        // Non-motion G codes, then motion.
        let mut motion = None;
        let mut g53 = false;
        let mut axes_used = false;
        for &g in &b.g {
            match g {
                0 | 10 | 20 | 30 => {
                    if motion.is_some() {
                        return Err("two motion commands on one line".into());
                    }
                    motion = Some(match g {
                        0 => Motion::Rapid,
                        10 => Motion::Linear,
                        20 => Motion::Cw,
                        _ => Motion::Ccw,
                    });
                }
                810 | 820 | 830 | 730 => {
                    if motion.is_some() {
                        return Err("two motion commands on one line".into());
                    }
                    if self.motion != Some(Motion::Drill) {
                        self.cycle_initial = self.known[2].then_some(self.pos[2]);
                    }
                    motion = Some(Motion::Drill);
                }
                800 => {
                    self.motion = None;
                    self.cycle_initial = None;
                }
                40 => {
                    let secs = b.get('P').or(b.get('S')).unwrap_or(0.0);
                    self.out.estimated_secs += secs.max(0.0);
                }
                170 => self.plane = Plane::Xy,
                180 => self.plane = Plane::Zx,
                190 => self.plane = Plane::Yz,
                200 => {
                    self.units = 25.4;
                    self.diag_once(
                        "inch",
                        Severity::Warning,
                        "inch units (G20): the preview converts to mm",
                    );
                }
                210 => self.units = 1.0,
                900 => self.absolute = true,
                910 => self.absolute = false,
                901 => self.arc_absolute = true,
                911 => self.arc_absolute = false,
                980 => self.retract_to_r = false,
                990 => self.retract_to_r = true,
                530 => g53 = true,
                540 | 400 | 490 | 610 | 611 | 640 | 940 => {}
                550 | 560 | 570 | 580 | 590 | 591 | 592 | 593 => self.diag_once(
                    "wcs",
                    Severity::Info,
                    "selects another work coordinate system; the preview and fit check use the current work offset",
                ),
                430 | 431 => self.diag_once(
                    "g43",
                    Severity::Info,
                    "the Z1 measures tool lengths at tool change; G43 isn't needed",
                ),
                930 => self.diag_once(
                    "g93",
                    Severity::Warning,
                    "inverse time feed (G93) isn't supported by the preview",
                ),
                100 => {
                    axes_used = true;
                    self.diag(
                        Severity::Warning,
                        "changes work offsets mid-program; the preview assumes they don't change"
                            .into(),
                    );
                }
                920 => {
                    axes_used = true;
                    self.g92(&b);
                }
                921 => self.offset = [0.0; 4],
                924 => {
                    axes_used = true;
                    if ['X', 'Y', 'Z'].iter().any(|&l| b.get(l).is_some()) {
                        self.diag(
                            Severity::Warning,
                            "redefines machine coordinates (G92.4); the preview can't follow"
                                .into(),
                        );
                    }
                }
                280 | 300 => {
                    axes_used = true;
                    self.known = [false; 4];
                    self.diag(
                        Severity::Info,
                        "moves to a stored position; the preview continues from the next absolute move"
                            .into(),
                    );
                }
                281 | 301 => {}
                382..=385 => {
                    axes_used = true;
                    // Probing stops at contact. On the Z1 the distances are relative.
                    for (i, l) in ['X', 'Y', 'Z', 'A'].iter().enumerate() {
                        if b.get(*l).is_some() {
                            self.known[i] = false;
                        }
                    }
                }
                g if reference::known('G', g) => {}
                g => self.diag(
                    Severity::Warning,
                    format!(
                        "unknown code G{}: the Z1 may reject or ignore it",
                        g as f64 / 10.0
                    ),
                ),
            }
        }

        if let Some(m) = motion {
            self.motion = Some(m);
            if m != Motion::Drill {
                self.cycle_initial = None;
            }
        }
        let has_axes = ['X', 'Y', 'Z', 'A'].iter().any(|&l| b.get(l).is_some());
        // A bare `G0`/`G1` only sets the mode.
        if !has_axes || axes_used {
            return Ok(());
        }
        let Some(motion) = self.motion else {
            return Err("axis words without a motion mode (G0, G1, ...)".into());
        };

        if g53 {
            if !matches!(motion, Motion::Rapid | Motion::Linear) {
                return Err("G53 needs G0 or G1".into());
            }
            // Machine coordinates: we don't know where that is in work coordinates.
            for (i, l) in ['X', 'Y', 'Z', 'A'].iter().enumerate() {
                if b.get(*l).is_some() {
                    self.known[i] = false;
                }
            }
            self.pen = None;
            return Ok(());
        }

        match motion {
            Motion::Rapid | Motion::Linear => {
                let (target, known) = self.target(&b);
                let kind = if motion == Motion::Rapid { RAPID } else { FEED };
                self.straight(target, known, kind);
            }
            Motion::Cw | Motion::Ccw => self.arc(&b, motion == Motion::Cw)?,
            Motion::Drill => self.drill(&b)?,
        }
        Ok(())
    }

    /// The end of a move from the axis words, in work coordinates, and which axes are known after.
    fn target(&self, b: &Block) -> ([f64; 4], [bool; 4]) {
        let mut target = self.pos;
        let mut known = self.known;
        for (i, l) in ['X', 'Y', 'Z', 'A'].iter().enumerate() {
            let Some(v) = b.get(*l) else { continue };
            let v = if i < 3 { v * self.units } else { v };
            if self.absolute {
                target[i] = v + self.offset[i];
                known[i] = true;
            } else {
                target[i] += v;
            }
        }
        (target, known)
    }

    fn g92(&mut self, b: &Block) {
        for (i, l) in ['X', 'Y', 'Z', 'A'].iter().enumerate() {
            let Some(v) = b.get(*l) else { continue };
            let v = if i < 3 { v * self.units } else { v };
            if self.known[i] {
                self.offset[i] = self.pos[i] - v;
            } else {
                self.diag(
                    Severity::Warning,
                    "G92 at an unknown position; the preview can't follow".into(),
                );
            }
        }
    }

    fn xyz(p: [f64; 4]) -> [f64; 3] {
        [p[0], p[1], p[2]]
    }

    fn vertex(&mut self, p: [f64; 3], kind: u8) {
        self.out.points.extend(p.map(|v| v as f32));
        self.out.kinds.push(kind);
        self.out.lines.push(self.line_no);
        Bounds::add(&mut self.out.bounds, p);
        self.pen = Some(p);
    }

    /// Draw a segment from the current position, adding a jump to it first if needed.
    fn segment(&mut self, to: [f64; 3], kind: u8) {
        let from = Self::xyz(self.pos);
        if self.pen != Some(from) {
            self.vertex(from, JUMP);
        }
        if kind == FEED {
            Bounds::add(&mut self.out.cut_bounds, from);
            Bounds::add(&mut self.out.cut_bounds, to);
        }
        self.vertex(to, kind);
    }

    fn feed_time(&mut self, dist: f64) {
        if self.feed > 0.0 {
            self.out.estimated_secs += dist / self.feed * 60.0;
        } else {
            self.diag_once("feed", Severity::Error, "feed move without a feed rate (F)");
        }
    }

    fn straight(&mut self, target: [f64; 4], known: [bool; 4], kind: u8) {
        if target[3] != self.pos[3] && known[3] {
            self.out.uses_a = true;
            self.diag_once(
                "a",
                Severity::Info,
                "moves the A axis, which the preview doesn't show",
            );
        }
        let drawable = self.known[..3].iter().all(|&k| k) && known[..3].iter().all(|&k| k);
        if drawable {
            let d: Vec<f64> = (0..3).map(|i| target[i] - self.pos[i]).collect();
            let dist = d.iter().map(|v| v * v).sum::<f64>().sqrt();
            if kind == RAPID {
                self.out.estimated_secs += (0..3)
                    .map(|i| d[i].abs() / RAPID_RATE[i] * 60.0)
                    .fold(0.0, f64::max);
            } else {
                self.feed_time(dist);
            }
            if dist > 1e-9 {
                self.segment(Self::xyz(target), kind);
            }
        } else if kind == FEED && self.feed <= 0.0 {
            self.feed_time(0.0);
        }
        self.pos = target;
        self.known = known;
    }

    fn arc(&mut self, b: &Block, cw: bool) -> Result<(), String> {
        let ([u, v, w], [lu, lv]) = self.plane.axes();
        if b.get('R').is_none() && b.get(lu).is_none() && b.get(lv).is_none() {
            return Err(format!("arc needs R or {lu}/{lv}"));
        }
        let (target, known) = self.target(b);
        if !(self.known[u] && self.known[v] && self.known[w]) || !known[..3].iter().all(|&k| k) {
            return Err("arc from an unknown position".into());
        }
        let (su, sv) = (self.pos[u], self.pos[v]);
        let (eu, ev) = (target[u], target[v]);

        let (cu, cv) = if let Some(r) = b.get('R') {
            let r = r * self.units;
            let (x, y) = (eu - su, ev - sv);
            let chord = x.hypot(y);
            if chord < 1e-9 {
                return Err("R-form arc with the same start and end".into());
            }
            let h2 = 4.0 * r * r - chord * chord;
            if h2 < -1e-6 * (1.0 + r * r) {
                return Err(format!(
                    "R{} is too small for an arc of length {chord:.3}",
                    r / self.units
                ));
            }
            let mut h = -h2.max(0.0).sqrt() / chord;
            if !cw {
                h = -h;
            }
            if r < 0.0 {
                h = -h;
            }
            (su + (x - y * h) / 2.0, sv + (y + x * h) / 2.0)
        } else {
            let ou = b.get(lu).unwrap_or(0.0) * self.units;
            let ov = b.get(lv).unwrap_or(0.0) * self.units;
            if self.arc_absolute {
                (ou + self.offset[u], ov + self.offset[v])
            } else {
                (su + ou, sv + ov)
            }
        };

        let r0 = (su - cu).hypot(sv - cv);
        let r1 = (eu - cu).hypot(ev - cv);
        if (r0 - r1).abs() > 0.005 + 0.001 * r0 {
            self.diag(
                Severity::Warning,
                format!("arc end is off the circle by {:.3} mm", (r0 - r1).abs()),
            );
        }
        let a0 = (sv - cv).atan2(su - cu);
        let a1 = (ev - cv).atan2(eu - cu);
        let tau = std::f64::consts::TAU;
        let full = (su - eu).abs() < 1e-9 && (sv - ev).abs() < 1e-9;
        let mut sweep = a1 - a0;
        if cw {
            if sweep >= 0.0 || full {
                sweep -= tau;
            }
        } else if sweep <= 0.0 || full {
            sweep += tau;
        }

        let dw = target[w] - self.pos[w];
        let step = if r0 > ARC_TOLERANCE {
            2.0 * (1.0 - ARC_TOLERANCE / r0).acos()
        } else {
            tau
        };
        let n = (sweep.abs() / step).ceil().clamp(1.0, MAX_ARC_SEGMENTS) as usize;
        self.feed_time((r0 * sweep).hypot(dw));
        let start = self.pos;
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let mut p = target;
            if k < n {
                let a = a0 + sweep * t;
                p[u] = cu + r0 * a.cos();
                p[v] = cv + r0 * a.sin();
                p[w] = start[w] + dw * t;
            }
            self.segment(Self::xyz(p), FEED);
            // `segment` draws from `self.pos`, so step it along the arc.
            self.pos = p;
        }
        self.pos = target;
        self.known = known;
        Ok(())
    }

    /// Canned drilling cycles (absolute mode only): to the hole at the current height, rapid to R,
    /// feed to Z, retract to R (G99) or the starting height (G98).
    fn drill(&mut self, b: &Block) -> Result<(), String> {
        if !self.absolute {
            return Err("canned cycles in incremental mode (G91) aren't supported".into());
        }
        if let Some(r) = b.get('R') {
            self.cycle_r = Some(r * self.units + self.offset[2]);
        }
        if let Some(z) = b.get('Z') {
            self.cycle_z = Some(z * self.units + self.offset[2]);
        }
        let (Some(r), Some(z)) = (self.cycle_r, self.cycle_z) else {
            return Err("drilling cycle needs R and Z".into());
        };
        if !self.known[2] {
            return Err("drilling cycle from an unknown height".into());
        }
        let initial = self.cycle_initial.unwrap_or(self.pos[2]);
        let mut xy = self.pos;
        let mut known = self.known;
        for (i, l) in ['X', 'Y'].iter().enumerate() {
            if let Some(v) = b.get(*l) {
                xy[i] = v * self.units + self.offset[i];
                known[i] = true;
            }
        }
        if self.pos[2] < r {
            let mut up = self.pos;
            up[2] = r;
            self.straight(up, self.known, RAPID);
        }
        xy[2] = self.pos[2];
        self.straight(xy, known, RAPID);
        let mut p = self.pos;
        p[2] = r;
        self.straight(p, self.known, RAPID);
        p[2] = z;
        self.straight(p, self.known, FEED);
        p[2] = if self.retract_to_r { r } else { initial.max(r) };
        self.straight(p, self.known, RAPID);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verts(a: &Analysis) -> Vec<([f32; 3], u8, u32)> {
        a.points
            .chunks(3)
            .zip(&a.kinds)
            .zip(&a.lines)
            .map(|((p, k), l)| ([p[0], p[1], p[2]], *k, *l))
            .collect()
    }

    fn errors(a: &Analysis) -> Vec<(u32, &str)> {
        a.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.line, d.message.as_str()))
            .collect()
    }

    #[test]
    fn draws_lines_from_the_first_known_position() {
        let a = analyze("G21 G90\nG0 Z5\nG0 X0 Y0\nG1 Z-1 F100\nG1 X10\nG91 G1 Y5\n");
        assert_eq!(
            verts(&a),
            vec![
                ([0.0, 0.0, 5.0], JUMP, 4),
                ([0.0, 0.0, -1.0], FEED, 4),
                ([10.0, 0.0, -1.0], FEED, 5),
                ([10.0, 5.0, -1.0], FEED, 6),
            ]
        );
        let b = a.bounds.unwrap();
        assert_eq!((b.min, b.max), ([0.0, 0.0, -1.0], [10.0, 5.0, 5.0]));
        assert_eq!(a.cut_bounds.unwrap().max, [10.0, 5.0, 5.0]);
        assert!(errors(&a).is_empty());
        // 6 + 10 + 5 mm at 100 mm/min.
        assert!((a.estimated_secs - 21.0 * 0.6).abs() < 1e-9);
    }

    #[test]
    fn arcs_ijk_and_r() {
        // Quarter circle CCW from (10,0) to (0,10) around the origin, two ways.
        for prog in [
            "G0 X10 Y0 Z0\nG3 X0 Y10 I-10 J0 F100\n",
            "G0 X10 Y0 Z0\nG3 X0 Y10 R10 F100\n",
        ] {
            let a = analyze(prog);
            assert!(errors(&a).is_empty(), "{prog}: {:?}", a.diagnostics);
            assert!(a.diagnostics.is_empty(), "{prog}: {:?}", a.diagnostics);
            let v = verts(&a);
            assert!(v.len() > 10);
            for (p, _, _) in &v[1..] {
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                assert!((r - 10.0).abs() < 1e-3, "{prog}: radius {r}");
                assert!(p[0] >= -1e-4 && p[1] >= -1e-4, "{prog}: left the quadrant");
            }
            assert_eq!(v.last().unwrap().0, [0.0, 10.0, 0.0]);
        }
    }

    #[test]
    fn full_circle_and_helix() {
        let a = analyze("G0 X5 Y0 Z0\nG2 X5 Y0 Z-2 I-5 J0 F100\n");
        let b = a.cut_bounds.unwrap();
        assert!((b.min[0] + 5.0).abs() < 0.01 && (b.max[1] - 5.0).abs() < 0.01);
        assert!((b.min[2] + 2.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_positions_break_the_path() {
        let a = analyze("G0 X0 Y0 Z5\nG53 G0 Z-1\nG0 X10\nG0 Z5\nG1 X20 F100\n");
        let v = verts(&a);
        assert_eq!(
            v,
            vec![([10.0, 0.0, 5.0], JUMP, 5), ([20.0, 0.0, 5.0], FEED, 5)]
        );
    }

    #[test]
    fn g92_and_inches() {
        let a = analyze("G0 X10 Y0 Z0\nG92 X0\nG1 X1 F10\nG20\nG1 X2\n");
        let v = verts(&a);
        assert_eq!(v[1].0, [11.0, 0.0, 0.0]);
        assert!((v[2].0[0] - 60.8).abs() < 1e-4);
        assert!(a.diagnostics.iter().any(|d| d.message.contains("inch")));
    }

    #[test]
    fn drilling_cycle() {
        let a = analyze("G0 X0 Y0 Z10\nG98 G81 X5 Y5 R2 Z-3 F50\nX10\nG80\n");
        assert!(errors(&a).is_empty(), "{:?}", a.diagnostics);
        let z: Vec<f32> = verts(&a).iter().map(|v| v.0[2]).collect();
        // Each hole: XY at 10, down to R, feed to -3, back to 10.
        assert_eq!(z, vec![10.0, 10.0, 2.0, -3.0, 10.0, 10.0, 2.0, -3.0, 10.0]);
    }

    #[test]
    fn tools_and_diagnostics() {
        let a = analyze("T2 M6\nG0 X0 Y0 Z1\nM6 T1\nM6\nG1 X1\nG17 G99.9\nG1 X\nG2 X1 Y1\n$H\n");
        assert_eq!(a.tools, vec![2, 1]);
        let e = errors(&a);
        assert_eq!(e[0], (5, "feed move without a feed rate (F)"));
        assert!(e.contains(&(7, "X needs a number")));
        assert!(e.iter().any(|(l, m)| *l == 8 && m.contains("arc needs R")));
        assert!(
            a.diagnostics
                .iter()
                .any(|d| d.line == 6 && d.severity == Severity::Warning)
        );
        assert!(
            a.diagnostics
                .iter()
                .any(|d| d.line == 9 && d.severity == Severity::Info)
        );
        assert_eq!(a.line_count, 9);
    }

    #[test]
    fn unknown_m_codes_warn() {
        let a = analyze("M220 S50\nM1234\nM801 S40\n");
        let lines: Vec<u32> = a.diagnostics.iter().map(|d| d.line).collect();
        assert_eq!(lines, [2], "{:?}", a.diagnostics);
    }
}
