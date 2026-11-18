//! Contours: tracing a part's outline (or a pocket's) at one height with the 3D probe, for a closed
//! 2D outline to export.
//!
//! It traces at the height the probe was jogged to. Outside, it first moves from where it is
//! toward the middle of a box drawn around the part until it touches; inside, from within the
//! pocket toward its front wall (−Y). From there it walks round with the material on its left
//! (so an outside contour goes counter-clockwise seen from above, an inside one clockwise), step by
//! step: from the last touch it stands off from the wall, steps along it by `spacing`, and probes
//! back toward it. The stand-off covers how far the wall can come back toward the probe over a
//! step, where it bends toward it no tighter than `radius` (s² / 2(R − r), the ball's centre
//! running a ball radius further out, plus a margin). Where the wall bends away (an outside
//! corner), the probe finds nothing; it tries again with half the step, and again, until it's
//! within about a ball radius of the corner, then swings back toward the last touch, which finds
//! the face round it.
//! Where it bends toward it tighter than `radius`, the step along it touches the wall: that's a
//! touch too. It's done when it's back by the first touch with a full turn behind it.
//!
//! Every move is a probe move, as in `probe`. Touches are single, at the searching feed. The
//! points are where the ball's centre was at each touch (X/Y; Z is where it traced); [`wall`]
//! moves them onto the wall.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::{Io, LET_GO, Result, Update};

/// What to trace. Machine coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContourSpec {
    /// A pocket's (or bore's) wall from inside it, rather than a part's outline from outside.
    pub inside: bool,
    /// Outside: the box around the part, front-left and back-right corners, X/Y. The trace mustn't
    /// leave it. Unused inside.
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// Between touches along the wall, mm.
    pub spacing: f64,
    /// The tightest the wall bends toward the probe (inside corners of a part, the corners of a
    /// pocket), mm.
    pub radius: f64,
    /// Inside: how far to look for the front wall, mm.
    pub search: f64,
}

impl ContourSpec {
    pub fn check(&self) -> std::result::Result<(), String> {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if !positive(self.spacing) || !positive(self.radius) {
            return Err("the spacing and radius must be positive".into());
        }
        if self.inside {
            if !positive(self.search) {
                return Err("the search distance must be positive".into());
            }
        } else {
            let finite = self.min.iter().chain(&self.max).all(|v| v.is_finite());
            if !finite || self.max[0] <= self.min[0] || self.max[1] <= self.min[1] {
                return Err("draw a box around the part".into());
            }
        }
        Ok(())
    }
}

/// A contour, traced or being traced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Contour {
    pub spec: ContourSpec,
    /// The tool tip's machine Z it traced at.
    pub z: f64,
    /// Where the ball's centre was at each touch, machine X/Y, in order round the wall.
    pub points: Vec<[f64; 2]>,
    /// Traced all the way round.
    pub closed: bool,
}

/// Room to spare between the ball and the wall when standing off, mm.
const MARGIN: f64 = 0.5;
/// How far a touch backs off before the next move, mm (it stops as soon as the probe lets go).
const RELEASE: f64 = 2.0;
/// Contours longer than this many points are taken to have gone wrong.
pub const MAX_POINTS: usize = 5000;

type V = [f64; 2];
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: V, k: f64) -> V {
    [a[0] * k, a[1] * k]
}
fn len(a: V) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: V) -> V {
    mul(a, 1.0 / len(a).max(1e-12))
}
/// 90° counter-clockwise: from the direction of travel, toward the material.
fn left(t: V) -> V {
    [-t[1], t[0]]
}
/// 90° clockwise: from toward the material, the direction of travel.
fn right(m: V) -> V {
    [m[1], -m[0]]
}
fn xyz(v: V) -> [f64; 3] {
    [v[0], v[1], 0.0]
}

/// The stand-off from the wall before each step along it: how far a wall bending toward the probe
/// no tighter than `radius` comes back over a step of `spacing`, for the ball's centre (a ball
/// radius further out, so bending tighter), plus [`MARGIN`].
pub fn stand_off(spacing: f64, radius: f64, ball_radius: f64) -> f64 {
    let bend = (radius - ball_radius).max(0.1);
    spacing * spacing / (2.0 * bend) + MARGIN
}

impl Io {
    fn xy(&self) -> V {
        [self.here[0], self.here[1]]
    }

    /// A probe move by `d` (X/Y): where the ball's centre was if it touched, after which it has
    /// backed off the way it came.
    async fn feel(&mut self, d: V, feed: f64) -> Result<Option<V>> {
        if len(d) < 1e-4 || !self.guarded(xyz(d), feed).await? {
            return Ok(None);
        }
        let at = self.xy();
        let back = mul(unit(d), -RELEASE);
        if !self
            .let_go(xyz(back), self.settings.fast_feed, LET_GO)
            .await?
        {
            return Err("the probe didn't release; check it".into());
        }
        Ok(Some(at))
    }
}

/// Trace `spec` from where the probe is, sending each touch as it comes. Returns when it's closed.
/// `so_far`: touches from a trace that stopped partway, to carry on from (empty to start).
pub(crate) async fn trace(io: &mut Io, spec: &ContourSpec, so_far: &[V]) -> Result<()> {
    let r = io.settings.ball_diameter / 2.0;
    let s = spec.spacing;
    // How finely a corner is closed in on before going round it.
    let corner_step = r.max(0.5).min(s);
    let fast = io.settings.fast_feed;
    let travel = io.settings.travel_feed;

    let mut points: Vec<V> = so_far.to_vec();
    // Toward the material from the last touch, how far round it's turned, and its direction.
    let (mut turned, mut heading) = (0.0, None::<V>);
    let mut m = if points.len() >= 2 {
        // Carrying on: all that from the touches so far.
        for w in points.windows(2) {
            let h = unit(sub(w[1], w[0]));
            if let Some(prev) = heading {
                turned += turn(prev, h);
            }
            heading = Some(h);
        }
        left(heading.unwrap())
    } else {
        // The first touch, and which way the material is from it.
        io.step("Finding the wall");
        let start = io.xy();
        let (dir, reach) = if spec.inside {
            ([0.0, -1.0], spec.search)
        } else {
            let centre = mul(add(spec.min, spec.max), 0.5);
            (unit(sub(centre, start)), len(sub(centre, start)))
        };
        let first = io.feel(mul(dir, reach), fast).await?.ok_or_else(|| {
            if spec.inside {
                format!("no wall within {:.1} mm in front (−Y)", spec.search)
            } else {
                "nothing found toward the middle of the box".to_string()
            }
        })?;
        let _ = io.updates.send(Update::ContourPoint(first));
        points = vec![first];
        dir
    };
    let first = points[0];
    loop {
        if points.len() > MAX_POINTS {
            return Err(format!(
                "still going after {MAX_POINTS} points; is the outline closed?"
            ));
        }
        let p = *points.last().unwrap();
        io.step(&format!("Point {}", points.len() + 1));
        let t = right(m);

        // Stand off, step along, and feel for the wall; where it isn't there (it bent away, round
        // an outside corner), again with half the step, closing in on the corner.
        let mut step = s;
        let off = sub(p, mul(m, stand_off(s, spec.radius, r)));
        if io.feel(sub(off, io.xy()), travel).await?.is_some() {
            return Err(format!(
                "no room to stand off from the wall at X{:.3} Y{:.3}; a smaller spacing needs less",
                p[0], p[1]
            ));
        }
        let next = loop {
            // Along it: a touch here means the wall bent toward the probe tighter than `radius`.
            if let Some(q) = io.feel(mul(t, step), travel).await? {
                break Some(q);
            }
            let reach = stand_off(s, spec.radius, r) + 2.0 * step;
            if let Some(q) = io.feel(mul(m, reach), fast).await? {
                break Some(q);
            }
            if step <= corner_step {
                break None;
            }
            // Back the way it came (round the corner, so not straight), for a shorter step.
            io.feel(mul(m, -reach), travel).await?;
            io.feel(mul(t, -step), travel).await?;
            step /= 2.0;
        };
        let mut next = next;
        if next.is_none() {
            // Just past the corner: back toward the last touch finds the face round it. Aimed a
            // little into the material from it: when the last touch is the corner itself, aiming at
            // it would only graze it.
            let aim = add(p, mul(m, r.max(0.5)));
            let back = sub(aim, io.xy());
            next = io
                .feel(mul(back, 1.0 + 1.0 / len(back).max(1e-9)), fast)
                .await?;
        }
        let next = next.ok_or_else(|| format!("lost the wall after X{:.3} Y{:.3}", p[0], p[1]))?;
        if !spec.inside && !inside_box(spec, next) {
            return Err(format!(
                "the outline left the box at X{:.3} Y{:.3}; draw it around the whole part",
                next[0], next[1]
            ));
        }

        let step = sub(next, p);
        if len(step) < 1e-3 {
            return Err(format!(
                "touched the same place twice at X{:.3} Y{:.3}",
                p[0], p[1]
            ));
        }
        let h = unit(step);
        if let Some(prev) = heading {
            turned += turn(prev, h);
        }
        heading = Some(h);
        m = left(h);
        points.push(next);
        let _ = io.updates.send(Update::ContourPoint(next));

        // Back by the first touch, all the way round.
        if points.len() > 3 && len(sub(next, first)) < 0.75 * s && turned.abs() > 5.5 {
            return Ok(());
        }
    }
}

/// The signed angle from direction `a` to `b`, counter-clockwise positive.
fn turn(a: V, b: V) -> f64 {
    (a[0] * b[1] - a[1] * b[0]).atan2(a[0] * b[0] + a[1] * b[1])
}

fn inside_box(spec: &ContourSpec, p: V) -> bool {
    (spec.min[0]..=spec.max[0]).contains(&p[0]) && (spec.min[1]..=spec.max[1]).contains(&p[1])
}

/// Run a trace as the routine task.
pub(crate) async fn run(spec: ContourSpec, so_far: Vec<V>, mut io: Io) {
    let result = trace(&mut io, &spec, &so_far).await;
    let _ = io.updates.send(Update::ContourDone(result));
}

// ---- results ----

/// The wall at each point, X/Y: with `compensate`, each centre moved a ball radius toward the
/// material, square to the line through its neighbours (the material is on the left of the
/// direction of travel, inside or out). Otherwise the centres as they are.
pub fn wall(points: &[[f64; 2]], ball_radius: f64, compensate: bool) -> Vec<[f64; 2]> {
    let n = points.len();
    if !compensate || n < 3 {
        return points.to_vec();
    }
    (0..n)
        .map(|i| {
            let (a, b) = (points[(i + n - 1) % n], points[(i + 1) % n]);
            add(points[i], mul(left(unit(sub(b, a))), ball_radius))
        })
        .collect()
}

/// `points` (X/Y, work coordinates) at height `z`: CSV/TSV, or a closed polyline in DXF.
pub fn export(points: &[[f64; 2]], z: f64, format: crate::map::ExportFormat) -> Vec<u8> {
    use crate::map::ExportFormat;
    match format {
        ExportFormat::Csv | ExportFormat::Tsv => {
            let sep = if format == ExportFormat::Csv {
                ','
            } else {
                '\t'
            };
            let mut out = format!("x{sep}y{sep}z\n");
            for [x, y] in points {
                out.push_str(&format!("{x:.4}{sep}{y:.4}{sep}{z:.4}\n"));
            }
            out.into_bytes()
        }
        ExportFormat::Dxf => dxf(points, z),
        ExportFormat::Stl => Vec::new(),
    }
}

/// A DXF (R12, entities only: what most CAD and CAM opens) with one closed polyline at `z`.
fn dxf(points: &[[f64; 2]], z: f64) -> Vec<u8> {
    let mut out = String::new();
    let mut pair = |code: u32, value: &str| {
        out.push_str(&format!("{code}\n{value}\n"));
    };
    pair(0, "SECTION");
    pair(2, "ENTITIES");
    pair(0, "POLYLINE");
    pair(8, "0");
    pair(66, "1");
    pair(10, "0.0");
    pair(20, "0.0");
    pair(30, &format!("{z:.4}"));
    // Closed.
    pair(70, "1");
    for [x, y] in points {
        pair(0, "VERTEX");
        pair(8, "0");
        pair(10, &format!("{x:.4}"));
        pair(20, &format!("{y:.4}"));
        pair(30, &format!("{z:.4}"));
    }
    pair(0, "SEQEND");
    pair(8, "0");
    pair(0, "ENDSEC");
    pair(0, "EOF");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stands_off_enough_for_the_bend() {
        // A straight wall needs only the margin; a tight bend more.
        assert!((stand_off(1.0, 1e9, 1.0) - MARGIN).abs() < 1e-6);
        assert!((stand_off(4.0, 9.0, 1.0) - (16.0 / 16.0 + MARGIN)).abs() < 1e-9);
    }

    #[test]
    fn compensates_onto_the_wall() {
        // Ball centres round circles, with the material on the left of the way round.
        let n = 72;
        let ring = |rad: f64, ccw: bool| -> Vec<[f64; 2]> {
            (0..n)
                .map(|i| {
                    let a =
                        std::f64::consts::TAU * i as f64 / n as f64 * if ccw { 1.0 } else { -1.0 };
                    [rad * a.cos(), rad * a.sin()]
                })
                .collect()
        };
        // Outside a Ø18 boss: centres at radius 10, counter-clockwise; the wall at 9.
        for p in wall(&ring(10.0, true), 1.0, true) {
            assert!((len(p) - 9.0).abs() < 0.01, "{p:?}");
        }
        // Inside a Ø20 bore: centres at radius 9, clockwise; the wall at 10.
        for p in wall(&ring(9.0, false), 1.0, true) {
            assert!((len(p) - 10.0).abs() < 0.01, "{p:?}");
        }
    }

    #[test]
    fn writes_a_closed_dxf_polyline() {
        let dxf = String::from_utf8(export(
            &[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]],
            -2.0,
            crate::map::ExportFormat::Dxf,
        ))
        .unwrap();
        assert!(dxf.contains("POLYLINE\n8\n0\n66\n1\n"));
        assert!(dxf.contains("70\n1\n"));
        assert_eq!(dxf.matches("VERTEX").count(), 3);
        assert!(dxf.contains("10\n10.0000\n20\n5.0000\n30\n-2.0000\n"));
        assert!(dxf.ends_with("0\nEOF\n"));
    }
}
