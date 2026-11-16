//! Surface maps: touching down on a grid of points, for a height map of a part to export as
//! points or a mesh.
//!
//! The scan starts with the probe above the area, at a height above everything in it (the
//! "top"); it never goes higher, and searches down to `depth` below it. Between points it only
//! lifts `clearance` above the last contact and moves across with a guarded move: if the surface
//! rises into the way, the probe touches it sideways, backs off, lifts another `clearance` and
//! carries on. That saves most of the vertical travel on parts that are roughly flat. The depth
//! probe can't feel sideways contact, so with it every move across is at the top.
//!
//! Points go row by row from the front, alternating direction, so moves are one step long.
//! Contacts are of the tool tip (the ball's bottom), like every other routine; [`surface`]
//! corrects them for the ball touching a slope off its bottom.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::{Io, ProbeSettings, Result, Update};

/// What to map. Machine coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MapSpec {
    /// Front-left corner of the grid, X/Y.
    pub min: [f64; 2],
    /// Back-right corner.
    pub max: [f64; 2],
    /// Points along X and Y, at least 2 each.
    pub columns: u32,
    pub rows: u32,
    /// How far below the start height to search, mm.
    pub depth: f64,
    /// A searching touch, then a slow measuring one at each point, instead of one touch.
    pub precise: bool,
}

/// A surface map, measured or being measured.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SurfaceMap {
    pub spec: MapSpec,
    /// The machine Z the scan started at: moves never go above it.
    pub top: f64,
    /// Tool tip Z per grid point, row by row from the front (−Y), each from left (−X). `None`
    /// where nothing was found within the depth, or not probed yet.
    pub z: Vec<Option<f64>>,
    /// How many points are done, in probing order ([`order`]).
    pub done: u32,
    /// Grid points without contact because they were out of the stylus' reach: deeper below
    /// the surface around than the probe body allows (see [`scan`]).
    pub unreached: Vec<u32>,
    /// Time spent probing, s, for an estimate of the rest.
    pub secs: f64,
}

impl SurfaceMap {
    pub fn new(spec: MapSpec, top: f64) -> Self {
        let n = (spec.columns * spec.rows) as usize;
        Self {
            spec,
            top,
            z: vec![None; n],
            done: 0,
            unreached: vec![],
            secs: 0.0,
        }
    }

    pub fn total(&self) -> u32 {
        self.spec.columns * self.spec.rows
    }
}

impl MapSpec {
    /// The machine X/Y of grid point `index`.
    pub fn at(&self, index: usize) -> [f64; 2] {
        let (c, r) = (index % self.columns as usize, index / self.columns as usize);
        let lerp = |a: f64, b: f64, i: usize, n: u32| a + (b - a) * i as f64 / (n - 1) as f64;
        [
            lerp(self.min[0], self.max[0], c, self.columns),
            lerp(self.min[1], self.max[1], r, self.rows),
        ]
    }

    pub fn check(&self) -> std::result::Result<(), String> {
        if self.columns < 2 || self.rows < 2 {
            return Err("a map needs at least 2 × 2 points".into());
        }
        if self.columns * self.rows > 100_000 {
            return Err("that's over 100,000 points; use a coarser grid".into());
        }
        let finite = self.min.iter().chain(&self.max).all(|v| v.is_finite());
        if !finite || self.max[0] <= self.min[0] || self.max[1] <= self.min[1] {
            return Err("the map area is empty".into());
        }
        if !(self.depth.is_finite() && self.depth > 0.0) {
            return Err("the search depth must be positive".into());
        }
        Ok(())
    }
}

/// Grid indices in probing order: rows from the front, alternating direction.
pub fn order(spec: &MapSpec) -> impl Iterator<Item = usize> + '_ {
    let (cols, rows) = (spec.columns as usize, spec.rows as usize);
    (0..rows).flat_map(move |r| {
        (0..cols).map(move |i| r * cols + if r % 2 == 0 { i } else { cols - 1 - i })
    })
}

/// Probe the points of `map` from `map.done` on, until done or `pause` is set. Returns whether
/// it finished (rather than paused). `lateral` says it's the 3D probe, which feels sideways
/// contact (allowing the low moves between points) and has a known stylus length (limiting how
/// deep each point may go: see [`Reach`]).
pub(crate) async fn scan(
    io: &mut Io,
    map: &SurfaceMap,
    lateral: bool,
    pause: &AtomicBool,
) -> Result<bool> {
    let spec = &map.spec;
    let top = map.top;
    let clearance = io.settings.clearance;
    let mut reach = Reach::new(map, &io.settings, lateral);
    // Wherever it was left, start from the top.
    rise(io, top).await?;
    let mut last: Option<(usize, f64)> = None;
    let total = map.total();
    for (k, index) in order(spec).enumerate().skip(map.done as usize) {
        if pause.load(Ordering::Relaxed) {
            rise(io, top).await?;
            return Ok(false);
        }
        let started = Instant::now();
        io.step(&format!("Point {} of {total}", k + 1));
        let [x, y] = spec.at(index);
        let floor = reach.floor(index);
        // Low across from the last contact, but with the body clear of what's around both ends.
        let height = match last {
            Some((from, z)) if lateral => {
                (z + clearance).max(floor).max(reach.floor(from)).min(top)
            }
            _ => top,
        };
        if height > io.here[2] {
            rise(io, height).await?;
        } else if height < io.here[2] - 1e-3 {
            // Down at the point just measured: nothing below but that surface.
            io.travel([0.0, 0.0, height - io.here[2]]).await?;
        }
        cross(io, [x, y], top, clearance).await?;

        let depth = io.here[2] - floor;
        let mut z = None;
        if depth > 0.0
            && io
                .probe_move("G38.3", [0.0, 0.0, -depth], io.settings.fast_feed)
                .await?
        {
            if spec.precise {
                io.release(2, -1.0).await?;
                io.travel([0.0, 0.0, 1.0]).await?;
                if !io
                    .probe_move("G38.3", [0.0, 0.0, -3.0], io.settings.slow_feed)
                    .await?
                {
                    return Err("lost contact on the slow touch; is the part moving?".into());
                }
            }
            z = Some(io.here[2]);
            io.release(2, -1.0).await?;
        }
        // Nothing found, and it stopped short of the depth asked for: out of the stylus' reach.
        let limited = z.is_none() && floor > top - spec.depth + 1e-6;
        reach.measured(index, z);
        last = z.map(|z| (index, z));
        let secs = started.elapsed().as_secs_f64();
        let _ = io.updates.send(Update::MapPoint {
            index,
            z,
            limited,
            secs,
        });
    }
    rise(io, top).await?;
    Ok(true)
}

/// Margin between the probe body and what's below it, mm.
const BODY_MARGIN: f64 = 2.0;

/// How deep each point may go. Down to the map's depth below its top, but with the 3D probe also
/// no further than the stylus length (less [`BODY_MARGIN`]) below the highest point measured
/// around it, so the body (which feels nothing) stays clear of it: a deep, narrow hole is left
/// out of reach rather than driving the body into its rim. Around is within the body's radius
/// plus one grid step, so the nearest points count even on a coarse grid. Points not measured
/// yet can't count; a wall taller than the stylus right beside the path is the user's to avoid.
struct Reach<'a> {
    spec: &'a MapSpec,
    z: Vec<Option<f64>>,
    floor: f64,
    /// The stylus length less the margin, and how far around counts; `None` without a stylus.
    stylus: Option<(f64, f64)>,
}

impl<'a> Reach<'a> {
    fn new(map: &'a SurfaceMap, settings: &ProbeSettings, stylus: bool) -> Self {
        let spec = &map.spec;
        let step_x = (spec.max[0] - spec.min[0]) / (spec.columns - 1) as f64;
        let step_y = (spec.max[1] - spec.min[1]) / (spec.rows - 1) as f64;
        Self {
            spec,
            z: map.z.clone(),
            floor: map.top - spec.depth,
            stylus: stylus.then(|| {
                (
                    settings.stylus_length - BODY_MARGIN,
                    settings.body_diameter / 2.0 + step_x.max(step_y),
                )
            }),
        }
    }

    fn measured(&mut self, index: usize, z: Option<f64>) {
        self.z[index] = z;
    }

    /// The lowest the tip may go at grid point `index`.
    fn floor(&self, index: usize) -> f64 {
        let Some((length, around)) = self.stylus else {
            return self.floor;
        };
        let spec = self.spec;
        let (cols, rows) = (spec.columns as usize, spec.rows as usize);
        let [x, y] = spec.at(index);
        // Only the grid window that can be within reach.
        let span = |lo: f64, hi: f64, n: usize| {
            ((around / ((hi - lo) / (n - 1) as f64)).ceil() as usize).min(n)
        };
        let (dc, dr) = (
            span(spec.min[0], spec.max[0], cols),
            span(spec.min[1], spec.max[1], rows),
        );
        let (c, r) = (index % cols, index / cols);
        let mut highest = f64::NEG_INFINITY;
        for rr in r.saturating_sub(dr)..(r + dr + 1).min(rows) {
            for cc in c.saturating_sub(dc)..(c + dc + 1).min(cols) {
                let i = rr * cols + cc;
                let [px, py] = spec.at(i);
                if let Some(z) = self.z[i]
                    && (px - x).hypot(py - y) <= around + 1e-9
                {
                    highest = highest.max(z);
                }
            }
        }
        self.floor.max(highest - length)
    }
}

/// Straight up to `z`, guarded.
async fn rise(io: &mut Io, z: f64) -> Result<()> {
    if z > io.here[2] + 1e-4 {
        io.travel([0.0, 0.0, z - io.here[2]]).await?;
    }
    Ok(())
}

/// Across to `xy` at the current height. If the surface rises into the way, back off until the
/// probe lets go, lift by `clearance` (no higher than `top`) and go on.
async fn cross(io: &mut Io, xy: [f64; 2], top: f64, clearance: f64) -> Result<()> {
    let feed = io.settings.travel_feed;
    loop {
        let d = [xy[0] - io.here[0], xy[1] - io.here[1], 0.0];
        if !io.probe_move("G38.3", d, feed).await? {
            return Ok(());
        }
        let len = d[0].hypot(d[1]);
        let back = [-d[0] / len * 2.0, -d[1] / len * 2.0, 0.0];
        if !io.probe_move("G38.5", back, io.settings.fast_feed).await? {
            return Err("the probe didn't release after touching on the way".into());
        }
        if io.here[2] >= top - 1e-4 {
            return Err(format!(
                "something is in the way at X{:.3} Y{:.3}, even at the start height; start higher",
                io.here[0], io.here[1]
            ));
        }
        rise(io, (io.here[2] + clearance).min(top)).await?;
    }
}

/// Run a scan as the routine task, with the probe switched on around it if `power`.
pub(crate) async fn run(
    map: SurfaceMap,
    mut io: Io,
    lateral: bool,
    power: bool,
    pause: Arc<AtomicBool>,
) {
    let mut result = Ok(true);
    if power {
        result = io.command("M494.1").await.map(|_| true);
    }
    if result.is_ok() {
        result = scan(&mut io, &map, lateral, &pause).await;
    }
    if power && let (Err(e), Ok(_)) = (io.command("M494.2").await, &result) {
        result = Err(e);
    }
    let _ = io.updates.send(Update::MapDone(result));
}

// ---- results ----

/// The measured surface at each grid point, machine coordinates; `None` where there's no point.
/// With `compensate`, corrected for the ball touching a slope off its bottom: the ball centres
/// lie on the surface grown by the radius, so each is moved back by the radius along that
/// surface's normal, estimated from the neighbouring points. On the flat that's the tool tip.
pub fn surface(map: &SurfaceMap, ball_radius: f64, compensate: bool) -> Vec<Option<[f64; 3]>> {
    let spec = &map.spec;
    let (cols, rows) = (spec.columns as usize, spec.rows as usize);
    let z = |c: usize, r: usize| map.z[r * cols + c];
    let dx = (spec.max[0] - spec.min[0]) / (cols - 1) as f64;
    let dy = (spec.max[1] - spec.min[1]) / (rows - 1) as f64;
    // Slope along one axis from whichever neighbours there are.
    let slope =
        |before: Option<f64>, here: f64, after: Option<f64>, step: f64| match (before, after) {
            (Some(a), Some(b)) => (b - a) / (2.0 * step),
            (Some(a), None) => (here - a) / step,
            (None, Some(b)) => (b - here) / step,
            (None, None) => 0.0,
        };
    let mut out = Vec::with_capacity(map.z.len());
    for r in 0..rows {
        for c in 0..cols {
            let Some(h) = z(c, r) else {
                out.push(None);
                continue;
            };
            let [x, y] = spec.at(r * cols + c);
            if !compensate {
                out.push(Some([x, y, h]));
                continue;
            }
            let gx = slope(
                c.checked_sub(1).and_then(|c| z(c, r)),
                h,
                (c + 1 < cols).then(|| z(c + 1, r)).flatten(),
                dx,
            );
            let gy = slope(
                r.checked_sub(1).and_then(|r| z(c, r)),
                h,
                (r + 1 < rows).then(|| z(c, r + 1)).flatten(),
                dy,
            );
            let len = (gx * gx + gy * gy + 1.0).sqrt();
            let n = [-gx / len, -gy / len, 1.0 / len];
            // Ball centre, then back along the normal.
            let centre = [x, y, h + ball_radius];
            out.push(Some([
                centre[0] - ball_radius * n[0],
                centre[1] - ball_radius * n[1],
                centre[2] - ball_radius * n[2],
            ]));
        }
    }
    out
}

/// What to write a map or contour as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportFormat {
    Csv,
    Tsv,
    /// Binary STL: the grid as a triangle mesh (maps only).
    Stl,
}

/// `points` (as from [`surface`], shifted to work coordinates) in `format`.
pub fn export(spec: &MapSpec, points: &[Option<[f64; 3]>], format: ExportFormat) -> Vec<u8> {
    match format {
        ExportFormat::Csv => delimited(points, ','),
        ExportFormat::Tsv => delimited(points, '\t'),
        ExportFormat::Stl => stl(spec, points),
    }
}

fn delimited(points: &[Option<[f64; 3]>], sep: char) -> Vec<u8> {
    let mut out = format!("x{sep}y{sep}z\n");
    for [x, y, z] in points.iter().flatten() {
        out.push_str(&format!("{x:.4}{sep}{y:.4}{sep}{z:.4}\n"));
    }
    out.into_bytes()
}

/// Two triangles per grid cell with all four corners measured, one with three.
fn stl(spec: &MapSpec, points: &[Option<[f64; 3]>]) -> Vec<u8> {
    let cols = spec.columns as usize;
    let rows = spec.rows as usize;
    let mut tris: Vec<[[f64; 3]; 3]> = vec![];
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            // Counter-clockwise from above, so the normals face up.
            let [a, b, cc, d] =
                [(c, r), (c + 1, r), (c + 1, r + 1), (c, r + 1)].map(|(c, r)| points[r * cols + c]);
            match (a, b, cc, d) {
                (Some(a), Some(b), Some(c), Some(d)) => tris.extend([[a, b, c], [a, c, d]]),
                (Some(a), Some(b), Some(c), None) => tris.push([a, b, c]),
                (Some(a), Some(b), None, Some(d)) => tris.push([a, b, d]),
                (Some(a), None, Some(c), Some(d)) => tris.push([a, c, d]),
                (None, Some(b), Some(c), Some(d)) => tris.push([b, c, d]),
                _ => {}
            }
        }
    }
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = b"Hobby Jogger surface map, mm".to_vec();
    header.resize(80, 0);
    out.extend(header);
    out.extend((tris.len() as u32).to_le_bytes());
    for [a, b, c] in tris {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        for f in n.map(|v| v / len).into_iter().chain(a).chain(b).chain(c) {
            out.extend((f as f32).to_le_bytes());
        }
        out.extend(0u16.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(columns: u32, rows: u32) -> MapSpec {
        MapSpec {
            min: [0.0, 0.0],
            max: [10.0, 20.0],
            columns,
            rows,
            depth: 5.0,
            precise: false,
        }
    }

    #[test]
    fn snakes_through_the_grid() {
        let s = spec(3, 3);
        assert_eq!(order(&s).collect::<Vec<_>>(), [0, 1, 2, 5, 4, 3, 6, 7, 8]);
        assert_eq!(s.at(5), [10.0, 10.0]);
    }

    #[test]
    fn compensates_a_slope() {
        // A 45° slope rising along X, z = x: the tip reads r(√2 − 1) above the surface off its
        // centre; compensated, every point lands back on z = x.
        let r = 1.0;
        let s = spec(3, 2);
        let mut map = SurfaceMap::new(s.clone(), 10.0);
        for i in 0..6 {
            let [x, _] = s.at(i);
            map.z[i] = Some(x + r * (2f64.sqrt() - 1.0));
        }
        for p in surface(&map, r, true).into_iter().flatten() {
            assert!((p[2] - p[0]).abs() < 1e-9, "{p:?}");
        }
        // Raw: the tip, at the grid point.
        assert_eq!(
            surface(&map, r, false)[1],
            Some([5.0, 0.0, 5.0 + r * (2f64.sqrt() - 1.0)])
        );
    }

    #[test]
    fn writes_csv_and_stl() {
        let s = spec(2, 2);
        let points = vec![
            Some([0.0, 0.0, 1.0]),
            Some([10.0, 0.0, 1.0]),
            None,
            Some([10.0, 20.0, 2.0]),
        ];
        let csv = String::from_utf8(export(&s, &points, ExportFormat::Csv)).unwrap();
        assert_eq!(csv.lines().count(), 4);
        assert!(csv.starts_with("x,y,z\n0.0000,0.0000,1.0000\n"));
        let stl = export(&s, &points, ExportFormat::Stl);
        // One triangle (three corners), and its normal points up.
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 1);
        assert_eq!(stl.len(), 84 + 50);
        assert!(f32::from_le_bytes(stl[92..96].try_into().unwrap()) > 0.0);
    }
}
