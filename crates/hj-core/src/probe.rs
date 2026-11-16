//! Probing routines with the 3D probe, run as a task alongside the core actor.
//!
//! Every move is a `G38.x` probe move, so the probe stops on contact wherever it goes: searches
//! (`G38.3`, "toward, no error") check the contact flag themselves, releases are `G38.5`, and
//! positioning moves are `G38.3` too, which turns an unexpected contact into an error instead of a
//! broken stylus. On the Z1, `G38.x` distances are always relative, they block until the move is
//! done, and report `[PRB:x,y,z:flag]` in machine coordinates before `ok` (see
//! `docs/research/probing.md`). Positions are of the tool tip: the ball's bottom in Z, its centre
//! in X/Y.
//!
//! Touches are two-pass, as in the user's own routines: a fast touch, back off until the probe
//! releases, 1 mm clearance, then a slow touch, which is the one measured.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeSettings {
    pub ball_diameter: f64,
    /// Searching touch, mm/min.
    pub fast_feed: f64,
    /// Measuring touch, mm/min.
    pub slow_feed: f64,
    /// Positioning moves (also probe moves, so they stop on contact), mm/min.
    pub travel_feed: f64,
    /// Height above a touched top for moves over the part, mm.
    pub clearance: f64,
    /// How far down to search for a top surface, mm.
    pub z_search: f64,
    /// 3D probe: how far the ball's bottom is below the probe body, mm. The body can't feel
    /// anything, so surface maps keep it this far (less a margin) above what's around.
    pub stylus_length: f64,
    /// 3D probe: the body's diameter, mm, for what counts as around.
    pub body_diameter: f64,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        Self {
            ball_diameter: 2.0,
            fast_feed: 100.0,
            slow_feed: 20.0,
            travel_feed: 600.0,
            clearance: 3.0,
            z_search: 20.0,
            // Makera's 3D probe (the only one there is for the Z1).
            stylus_length: 13.0,
            body_diameter: 17.0,
        }
    }
}

/// Which side of the part an edge is on. Front is −Y, left is −X.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Side {
    Left,
    Right,
    Front,
    Back,
}

impl Side {
    /// The axis the edge is found along, and which way is outward.
    fn axis(self) -> (usize, f64) {
        match self {
            Self::Left => (0, -1.0),
            Self::Right => (0, 1.0),
            Self::Front => (1, -1.0),
            Self::Back => (1, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Corner {
    FrontLeft,
    FrontRight,
    BackLeft,
    BackRight,
}

impl Corner {
    fn sides(self) -> (Side, Side) {
        match self {
            Self::FrontLeft => (Side::Left, Side::Front),
            Self::FrontRight => (Side::Right, Side::Front),
            Self::BackLeft => (Side::Left, Side::Back),
            Self::BackRight => (Side::Right, Side::Back),
        }
    }
}

/// Which way a bore or boss is measured across.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Across {
    /// Both, for a round feature's centre and diameter.
    #[default]
    Xy,
    /// One axis only, for a slot's or web's centreline and width.
    X,
    Y,
}

impl Across {
    /// The passes to make. Both is X, Y at the X centre, then X again at the Y centre, so the
    /// last X pass crosses the full diameter.
    fn passes(self) -> &'static [usize] {
        match self {
            Self::Xy => &[0, 1, 0],
            Self::X => &[0],
            Self::Y => &[1],
        }
    }
}

/// What to probe, and from where. Distances are in mm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Routine {
    /// Start above the surface; touch down.
    Surface,
    /// Start beside the part, outside the edge, at the height to touch it; touches in, searching
    /// up to `search`.
    Edge { side: Side, search: f64 },
    /// Start above the part, within `reach` of both edges at the corner. Finds the top, then for
    /// each edge steps outside, drops `depth` below the top and touches back in.
    Corner {
        corner: Corner,
        depth: f64,
        reach: f64,
    },
    /// Start inside the bore (or slot), below its top, near the centre. `diameter` is the width
    /// when measuring across one axis.
    Bore {
        diameter: f64,
        #[serde(default)]
        across: Across,
    },
    /// Start above the boss (or web), near its centre.
    Boss {
        diameter: f64,
        depth: f64,
        #[serde(default)]
        across: Across,
    },
}

impl Routine {
    pub fn label(&self) -> String {
        match self {
            Self::Surface => "Surface".into(),
            Self::Edge { side, .. } => format!("{side:?} edge"),
            Self::Corner { corner, .. } => match corner {
                Corner::FrontLeft => "Front left corner".into(),
                Corner::FrontRight => "Front right corner".into(),
                Corner::BackLeft => "Back left corner".into(),
                Corner::BackRight => "Back right corner".into(),
            },
            Self::Bore { across, .. } => match across {
                Across::Xy => "Bore centre".into(),
                axis => format!("Bore centreline in {axis:?}"),
            },
            Self::Boss { across, .. } => match across {
                Across::Xy => "Boss centre".into(),
                axis => format!("Boss centreline in {axis:?}"),
            },
        }
    }
}

/// What a routine found, in machine coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeResult {
    pub label: String,
    /// The feature's position: an edge's X or Y, a corner's X and Y, a centre.
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// The top surface.
    pub z: Option<f64>,
    pub diameter: Option<f64>,
    /// Across one axis: the bore's or boss's width along it.
    pub width: Option<f64>,
}

/// Progress from a running routine to the core.
#[derive(Debug)]
pub(crate) enum Update {
    /// Send this line to the machine (logged like any other).
    Send(String),
    Step(String),
    /// A measured contact (tool tip, machine coordinates), for the bed view.
    Contact([f64; 3]),
    /// Before each move: the way out if it goes wrong (a unit vector), for [`back_off`].
    BackOff([f64; 3]),
    Done(std::result::Result<ProbeResult, String>),
    /// [`back_off`] finished.
    Recovered(std::result::Result<(), String>),
    /// A surface map point: grid index, tip Z if found, seconds it took.
    MapPoint {
        index: usize,
        z: Option<f64>,
        /// No contact because the point was out of the stylus' reach.
        limited: bool,
        secs: f64,
    },
    /// A surface map scan stopped: whether it finished (rather than paused), or why it failed.
    MapDone(std::result::Result<bool, String>),
}

/// The routine's side of the machine connection: send lines through the core, read the console
/// lines the core forwards while the routine runs.
pub(crate) struct Io {
    pub updates: mpsc::UnboundedSender<Update>,
    pub lines: mpsc::UnboundedReceiver<String>,
    /// Where the tool tip is, tracked from the `[PRB:…]` replies.
    pub here: [f64; 3],
    pub settings: ProbeSettings,
}

pub(crate) type Result<T> = std::result::Result<T, String>;

const AXES: [char; 3] = ['X', 'Y', 'Z'];

impl Io {
    pub(crate) fn step(&self, text: &str) {
        let _ = self.updates.send(Update::Step(text.into()));
    }

    /// Send a probe move and wait for its `[PRB:…]` and `ok`. Returns whether it triggered.
    pub(crate) async fn probe_move(
        &mut self,
        code: &str,
        delta: [f64; 3],
        feed: f64,
    ) -> Result<bool> {
        let words: Vec<String> = (0..3)
            .filter(|&i| delta[i].abs() >= 1e-4)
            .map(|i| format!("{}{:.4}", AXES[i], delta[i]))
            .collect();
        if words.is_empty() {
            return Ok(false);
        }
        let line = format!("{code} {} F{feed:.0}", words.join(" "));
        let len = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
        let _ = self.updates.send(Update::BackOff(way_out(code, delta)));
        let timeout = Duration::from_secs_f64(len / feed * 60.0 * 1.5 + 15.0);
        self.updates
            .send(Update::Send(line.clone()))
            .map_err(|_| "stopped".to_string())?;

        let mut report = None;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let line = tokio::time::timeout_at(deadline, self.lines.recv())
                .await
                .map_err(|_| format!("no reply to {code} within {timeout:?}"))?
                .ok_or("disconnected")?;
            let lower = line.to_ascii_lowercase();
            if let Some(prb) = parse_prb(&line) {
                report = Some(prb);
            } else if line == "ok" {
                break;
            } else if lower.starts_with("error") || lower.starts_with("alarm") || line == "!!" {
                return Err(format!("the machine reported: {line}"));
            }
        }
        let (pos, hit) = report.ok_or_else(|| format!("no probe report for {code}"))?;
        self.here = pos;
        Ok(hit)
    }

    /// Move by `delta`, stopping (with an error) if the probe touches anything on the way.
    /// Send a plain command and wait for its `ok`.
    pub(crate) async fn command(&mut self, line: &str) -> Result<()> {
        self.updates
            .send(Update::Send(line.into()))
            .map_err(|_| "stopped".to_string())?;
        loop {
            let reply = tokio::time::timeout(Duration::from_secs(10), self.lines.recv())
                .await
                .map_err(|_| format!("no reply to {line}"))?
                .ok_or("disconnected")?;
            let lower = reply.to_ascii_lowercase();
            if reply == "ok" {
                return Ok(());
            } else if lower.starts_with("error") || lower.starts_with("alarm") || reply == "!!" {
                return Err(format!("the machine reported: {reply}"));
            }
        }
    }

    /// Move by `delta`, stopping if the probe touches anything on the way. Then it backs out to
    /// where the move started, which was clear, and reports the contact as an error.
    pub(crate) async fn travel(&mut self, delta: [f64; 3]) -> Result<()> {
        let feed = self.settings.travel_feed;
        let from = self.here;
        if !self.probe_move("G38.3", delta, feed).await? {
            return Ok(());
        }
        let hit = self.here;
        // Triggered while lifting: not something in the way but the probe still pressed (a
        // switch that hasn't let go, say). Going back down would only press it harder.
        if delta[2] > 0.0 {
            return Err(format!(
                "the probe still read triggered while lifting, at Z{:.3}. Recover lifts it until \
                 it releases.",
                hit[2]
            ));
        }
        let what = format!(
            "the probe touched something on the way, at X{:.3} Y{:.3} Z{:.3}",
            hit[0], hit[1], hit[2]
        );
        let back = |here: [f64; 3]| [from[0] - here[0], from[1] - here[1], from[2] - here[2]];
        // Release first (stops as soon as it does), then the rest of the way, still guarded.
        let released = self
            .probe_move("G38.5", back(hit), self.settings.fast_feed)
            .await
            .unwrap_or(false);
        if !released {
            return Err(format!("{what}, and it didn't release when backing off"));
        }
        if self.probe_move("G38.3", back(self.here), feed).await? {
            return Err(format!("{what}, and again while backing off"));
        }
        Err(format!("{what} (it backed off to where that move started)"))
    }

    async fn travel_to(&mut self, target: [f64; 3]) -> Result<()> {
        let here = self.here;
        // Up first, sideways at height, then down: never drag the stylus across the part.
        if target[2] > here[2] {
            self.travel([0.0, 0.0, target[2] - here[2]]).await?;
        }
        let here = self.here;
        self.travel([target[0] - here[0], target[1] - here[1], 0.0])
            .await?;
        let here = self.here;
        self.travel([0.0, 0.0, target[2] - here[2]]).await
    }

    /// Two-pass touch along `axis` in direction `dir` (±1), searching up to `reach`. Returns the
    /// measured contact (the tool tip's coordinate on `axis`) and leaves the probe 1 mm back.
    async fn touch(&mut self, axis: usize, dir: f64, reach: f64) -> Result<f64> {
        let s = self.settings.clone();
        let along = |d: f64| {
            let mut v = [0.0; 3];
            v[axis] = d;
            v
        };
        if !self
            .probe_move("G38.3", along(dir * reach), s.fast_feed)
            .await?
        {
            return Err(format!(
                "no contact within {reach:.1} mm moving {}{}",
                if dir > 0.0 { "+" } else { "−" },
                AXES[axis]
            ));
        }
        self.release(axis, dir).await?;
        self.travel(along(-dir)).await?;
        if !self
            .probe_move("G38.3", along(dir * 3.0), s.slow_feed)
            .await?
        {
            return Err("lost contact on the slow touch; is the part moving?".into());
        }
        let contact = self.here[axis];
        let _ = self.updates.send(Update::Contact(self.here));
        self.release(axis, dir).await?;
        self.travel(along(-dir)).await?;
        Ok(contact)
    }

    pub(crate) async fn release(&mut self, axis: usize, dir: f64) -> Result<()> {
        let mut back = [0.0; 3];
        back[axis] = -dir * 2.0;
        if !self
            .probe_move("G38.5", back, self.settings.fast_feed / 2.0)
            .await?
        {
            return Err("the probe didn't release; check it".into());
        }
        Ok(())
    }

    /// Touch the top below the current position; returns its Z and lifts to clearance above it.
    async fn top(&mut self) -> Result<f64> {
        self.step("Touching the top");
        let z = self.touch(2, -1.0, self.settings.z_search).await?;
        let lift = z + self.settings.clearance - self.here[2];
        self.travel([0.0, 0.0, lift]).await?;
        Ok(z)
    }

    /// Find an edge from above the part: out by `reach`, down to `depth` below `top`, touch back
    /// in, then back up and over the start. Returns the edge coordinate.
    async fn edge(&mut self, side: Side, top: f64, depth: f64, reach: f64) -> Result<f64> {
        let (axis, out) = side.axis();
        let r = self.settings.ball_diameter / 2.0;
        let start = self.here;
        self.step(&format!(
            "Finding the {} edge",
            format!("{side:?}").to_lowercase()
        ));
        // Far enough out that the ball clears the edge on the way down, with 2 mm to spare.
        let mut outside = start;
        outside[axis] += out * (reach + r + 2.0);
        self.travel_to(outside).await?;
        self.travel([0.0, 0.0, top - depth - self.here[2]])
            .await
            .map_err(|e| {
                format!(
                    "coming down outside the edge, {e}. Is the edge more than {reach} mm from \
                     where the probe started? Increase Reach or start closer to it."
                )
            })?;
        let contact = self.touch(axis, -out, reach + r + 7.0).await?;
        self.travel_to(start).await?;
        // The ball centre stops a radius short of the surface.
        Ok(contact - out * r)
    }
}

/// The way out if a probe move goes wrong (a unit vector), for [`back_off`]: back the way it
/// came for a move toward something (`G38.2`/`G38.3`), onwards for one away from it
/// (`G38.4`/`G38.5`). Never down, though: a probe that triggers going up is still pressed from
/// below, and backing off downward drives it into the part. Then it's straight up.
fn way_out(code: &str, delta: [f64; 3]) -> [f64; 3] {
    let len = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
    let away = if code == "G38.4" || code == "G38.5" {
        1.0
    } else {
        -1.0
    };
    let dir = delta.map(|d| away * d / len);
    if dir[2] < -1e-9 { [0.0, 0.0, 1.0] } else { dir }
}

/// `[PRB:x,y,z:flag]`
fn parse_prb(line: &str) -> Option<([f64; 3], bool)> {
    let inner = line.strip_prefix("[PRB:")?.strip_suffix(']')?;
    let (coords, flag) = inner.rsplit_once(':')?;
    let v: Vec<f64> = coords
        .split(',')
        .map(|c| c.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, z] = v[..] else { return None };
    Some(([x, y, z], flag.trim() == "1"))
}

/// How far [`back_off`] goes looking for the probe to release, mm.
pub const BACK_OFF: f64 = 5.0;

/// Recover after a routine went wrong: clear the halt if `unlock`, then move along `dir` until the
/// probe releases (a probe move, so it stops as soon as it does; fine if it wasn't in contact).
pub(crate) async fn back_off(unlock: bool, dir: [f64; 3], mut io: Io) {
    let feed = io.settings.fast_feed / 2.0;
    let mut result = Ok(());
    if unlock {
        result = io.command("$X").await;
    }
    if result.is_ok() {
        result = io
            .probe_move("G38.5", dir.map(|d| d * BACK_OFF), feed)
            .await
            .map(|_| ());
    }
    let _ = io.updates.send(Update::Recovered(result));
}

/// Run `routine` from the current position (`io.here`) and report the result. With `power`, the
/// probe is switched on (`M494.1`) for it and off again after.
pub(crate) async fn run(routine: Routine, mut io: Io, power: bool) {
    let mut result = Ok(());
    if power {
        result = io.command("M494.1").await;
    }
    let mut result = match result {
        Ok(()) => execute(&routine, &mut io).await,
        Err(e) => Err(e),
    };
    if power {
        // Best effort: after a halt the machine refuses it, and the routine's error says why.
        if let (Err(e), Ok(_)) = (io.command("M494.2").await, &result) {
            result = Err(e);
        }
    }
    let _ = io.updates.send(Update::Done(result));
}

async fn execute(routine: &Routine, io: &mut Io) -> Result<ProbeResult> {
    let r = io.settings.ball_diameter / 2.0;
    let mut result = ProbeResult {
        label: routine.label(),
        ..ProbeResult::default()
    };
    match *routine {
        Routine::Surface => {
            result.z = Some(io.top().await?);
        }
        Routine::Edge { side, search } => {
            let (axis, out) = side.axis();
            io.step(&format!(
                "Touching the {} edge",
                format!("{side:?}").to_lowercase()
            ));
            let contact = io.touch(axis, -out, search).await?;
            // The ball centre stops a radius short of the surface.
            let edge = contact - out * r;
            if axis == 0 {
                result.x = Some(edge);
            } else {
                result.y = Some(edge);
            }
        }
        Routine::Corner {
            corner,
            depth,
            reach,
        } => {
            let (sx, sy) = corner.sides();
            let top = io.top().await?;
            result.x = Some(io.edge(sx, top, depth, reach).await?);
            result.y = Some(io.edge(sy, top, depth, reach).await?);
            result.z = Some(top);
        }
        Routine::Bore { diameter, across } => {
            let reach = diameter / 2.0 + 5.0;
            let mut center = io.here;
            let mut spans = [0.0; 2];
            for &axis in across.passes() {
                io.step(&format!("Touching the bore wall in {}", AXES[axis]));
                let plus = io.touch(axis, 1.0, reach).await?;
                io.travel_to(center).await?;
                let minus = io.touch(axis, -1.0, reach).await?;
                center[axis] = (plus + minus) / 2.0;
                spans[axis] = plus - minus + 2.0 * r;
                io.travel_to(center).await?;
            }
            set_across(&mut result, across, center, spans);
        }
        Routine::Boss {
            diameter,
            depth,
            across,
        } => {
            let top = io.top().await?;
            let mut center = io.here;
            let mut widths = [0.0; 2];
            let out = diameter / 2.0 + r + 3.0;
            for &axis in across.passes() {
                io.step(&format!("Touching the boss in {}", AXES[axis]));
                let mut contacts = [0.0; 2];
                for (k, dir) in [1.0, -1.0].into_iter().enumerate() {
                    let mut outside = center;
                    outside[axis] += dir * out;
                    io.travel_to(outside).await?;
                    io.travel([0.0, 0.0, top - depth - io.here[2]])
                        .await
                        .map_err(|e| {
                            format!(
                                "coming down beside the boss, {e}. Is it larger than \
                                 {diameter} mm, or the probe far off its centre?"
                            )
                        })?;
                    contacts[k] = io.touch(axis, -dir, out + 2.0).await?;
                    // Straight up beside the wall; the next move crosses over from there.
                    io.travel([0.0, 0.0, center[2] - io.here[2]]).await?;
                }
                center[axis] = (contacts[0] + contacts[1]) / 2.0;
                widths[axis] = contacts[0] - contacts[1] - 2.0 * r;
                io.travel_to(center).await?;
            }
            set_across(&mut result, across, center, widths);
            result.z = Some(top);
        }
    }
    io.step("Done");
    Ok(result)
}

/// The centre and diameter, or across one axis, its centreline and width.
fn set_across(result: &mut ProbeResult, across: Across, center: [f64; 3], spans: [f64; 2]) {
    match across {
        Across::Xy => {
            result.x = Some(center[0]);
            result.y = Some(center[1]);
            result.diameter = Some((spans[0] + spans[1]) / 2.0);
        }
        Across::X => {
            result.x = Some(center[0]);
            result.width = Some(spans[0]);
        }
        Across::Y => {
            result.y = Some(center[1]);
            result.width = Some(spans[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_away_and_never_down() {
        // Touching down: up. Touching an edge in +X: back in −X.
        assert_eq!(way_out("G38.3", [0.0, 0.0, -20.0]), [0.0, 0.0, 1.0]);
        assert_eq!(way_out("G38.3", [10.0, 0.0, 0.0]), [-1.0, 0.0, 0.0]);
        // Releasing upward: onwards, up.
        assert_eq!(way_out("G38.5", [0.0, 0.0, 2.0]), [0.0, 0.0, 1.0]);
        // A guarded lift that triggers: up, not back down into the part.
        assert_eq!(way_out("G38.3", [0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]);
        // Releasing sideways and down (it never does, but): up.
        assert_eq!(way_out("G38.5", [0.0, 0.0, -2.0]), [0.0, 0.0, 1.0]);
        // Moving up and sideways: straight up rather than back down at an angle.
        assert_eq!(way_out("G38.3", [3.0, 0.0, 4.0]), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn parses_probe_reports() {
        assert_eq!(
            parse_prb("[PRB:-9.400,-13.300,-82.154:1]"),
            Some(([-9.4, -13.3, -82.154], true))
        );
        assert_eq!(parse_prb("[PRB:0,0,0:0]"), Some(([0.0; 3], false)));
        assert_eq!(parse_prb("ok"), None);
    }
}
