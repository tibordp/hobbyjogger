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
//! A map is a grid of `columns` × `rows` points, and with `levels` of refinement, a finer
//! lattice: the grid's spacing halved `levels` times, of which only some points are probed. The
//! grid comes first; then each cell's centre, and where that's further than `tolerance` from the
//! average of the cell's corners, or the corners and centre don't all touch or all miss (an edge
//! of the part), the midpoints of its sides, and the same again on its four quarters, down to the
//! lattice. What to probe next depends only on what's been measured ([`plan`]), so a paused map
//! carries on by planning again. Each batch goes row by row, alternating direction.
//!
//! Contacts are of the tool tip (the ball's bottom), like every other routine; [`surface`]
//! corrects them for the ball touching a slope off its bottom.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::{Io, LET_GO, ProbeSettings, Result, Update};

/// What to map. Machine coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MapSpec {
    /// Front-left corner of the grid, X/Y.
    pub min: [f64; 2],
    /// Back-right corner.
    pub max: [f64; 2],
    /// Grid points along X and Y, at least 2 each.
    pub columns: u32,
    pub rows: u32,
    /// How far below the start height to search, mm.
    pub depth: f64,
    /// A searching touch, then a slow measuring one at each point, instead of one touch.
    pub precise: bool,
    /// How many times the grid's spacing may be halved where the surface needs it; 0 for a plain
    /// grid.
    #[serde(default)]
    pub levels: u32,
    /// Refinement: how far a cell's centre may be from the average of its corners, mm.
    #[serde(default)]
    pub tolerance: f64,
}

/// A surface map, measured or being measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SurfaceMap {
    pub spec: MapSpec,
    /// The machine Z the scan started at: moves never go above it.
    pub top: f64,
    /// Tool tip Z per lattice point ([`MapSpec::lattice`]), row by row from the front (−Y), each
    /// from left (−X). `None` where nothing was found within the depth, or not probed.
    pub z: Vec<Option<f64>>,
    /// The lattice points probed, in the order they were.
    pub probed: Vec<u32>,
    /// Points planned and not probed yet: the rest of the grid, or of the batch of refinement
    /// in progress (more may follow it).
    pub pending: u32,
    /// Lattice points without contact because they were out of the stylus' reach: deeper below
    /// the surface around than the probe body allows (see [`scan`]).
    pub unreached: Vec<u32>,
    /// Nothing more to probe.
    pub complete: bool,
    /// Time spent probing, s, for an estimate of the rest.
    pub secs: f64,
}

impl SurfaceMap {
    pub fn new(spec: MapSpec, top: f64) -> Self {
        let (cols, rows) = spec.lattice();
        Self {
            pending: spec.columns * spec.rows,
            spec,
            top,
            z: vec![None; cols * rows],
            probed: vec![],
            unreached: vec![],
            complete: false,
            secs: 0.0,
        }
    }
}

impl MapSpec {
    /// Lattice points per grid step.
    pub fn scale(&self) -> usize {
        1 << self.levels
    }

    /// Columns and rows of the lattice.
    pub fn lattice(&self) -> (usize, usize) {
        let s = self.scale();
        (
            (self.columns as usize - 1) * s + 1,
            (self.rows as usize - 1) * s + 1,
        )
    }

    /// The lattice's spacing along X and Y, mm.
    pub fn step(&self) -> [f64; 2] {
        let (cols, rows) = self.lattice();
        [
            (self.max[0] - self.min[0]) / (cols - 1) as f64,
            (self.max[1] - self.min[1]) / (rows - 1) as f64,
        ]
    }

    /// The machine X/Y of lattice point `index`.
    pub fn at(&self, index: usize) -> [f64; 2] {
        let (cols, _) = self.lattice();
        let [dx, dy] = self.step();
        [
            self.min[0] + dx * (index % cols) as f64,
            self.min[1] + dy * (index / cols) as f64,
        ]
    }

    pub fn check(&self) -> std::result::Result<(), String> {
        if self.columns < 2 || self.rows < 2 {
            return Err("a map needs at least 2 × 2 points".into());
        }
        if self.levels > 6 {
            return Err("refine at most 6 times".into());
        }
        let (cols, rows) = self.lattice();
        if self.columns * self.rows > 100_000 || cols * rows > 1_000_000 {
            return Err("that's too many points; use a coarser grid or fewer levels".into());
        }
        let finite = self.min.iter().chain(&self.max).all(|v| v.is_finite());
        if !finite || self.max[0] <= self.min[0] || self.max[1] <= self.min[1] {
            return Err("the map area is empty".into());
        }
        if !(self.depth.is_finite() && self.depth > 0.0) {
            return Err("the search depth must be positive".into());
        }
        if self.levels > 0 && !(self.tolerance.is_finite() && self.tolerance > 0.0) {
            return Err("the refinement tolerance must be positive".into());
        }
        Ok(())
    }
}

/// The lattice points to probe next, given what's been measured, in the order to probe them:
/// the grid's first, then refinement as in the module docs. Empty when the map is complete.
pub fn plan(spec: &MapSpec, z: &[Option<f64>], probed: &[bool]) -> Vec<usize> {
    let (cols, rows) = spec.lattice();
    let s = spec.scale();
    let grid: Vec<usize> = (0..rows)
        .step_by(s)
        .flat_map(|r| (0..cols).step_by(s).map(move |c| r * cols + c))
        .filter(|&i| !probed[i])
        .collect();
    if !grid.is_empty() {
        // Rows of the grid alternate, as they would have from the start.
        return snake(grid, cols, |r| (r / s) % 2 == 1);
    }
    let mut need = vec![];
    for r0 in (0..rows - 1).step_by(s) {
        for c0 in (0..cols - 1).step_by(s) {
            refine(spec, z, probed, c0, r0, s, &mut need);
        }
    }
    need.sort_unstable();
    need.dedup();
    let rank: std::collections::BTreeMap<usize, usize> = {
        let mut rows: Vec<usize> = need.iter().map(|i| i / cols).collect();
        rows.dedup();
        rows.into_iter().enumerate().map(|(k, r)| (r, k)).collect()
    };
    snake(need, cols, |r| rank[&r] % 2 == 1)
}

/// `points` row by row from the front, each row left to right unless `reversed`.
fn snake(mut points: Vec<usize>, cols: usize, reversed: impl Fn(usize) -> bool) -> Vec<usize> {
    points.sort_unstable_by_key(|&i| {
        let (c, r) = (i % cols, i / cols);
        (r, if reversed(r) { cols - c } else { c })
    });
    points
}

/// What the cell at lattice (`c0`, `r0`), `size` points across, still needs: its centre, then if
/// it's to be split, its sides' midpoints, then its quarters' needs.
fn refine(
    spec: &MapSpec,
    z: &[Option<f64>],
    probed: &[bool],
    c0: usize,
    r0: usize,
    size: usize,
    need: &mut Vec<usize>,
) {
    if size < 2 {
        return;
    }
    let (cols, _) = spec.lattice();
    let at = |c: usize, r: usize| r * cols + c;
    let h = size / 2;
    let centre = at(c0 + h, r0 + h);
    if !probed[centre] {
        need.push(centre);
        return;
    }
    let corners = [
        at(c0, r0),
        at(c0 + size, r0),
        at(c0 + size, r0 + size),
        at(c0, r0 + size),
    ]
    .map(|i| z[i]);
    let all = corners.iter().chain([&z[centre]]);
    let hits = all.clone().filter(|v| v.is_some()).count();
    let split = match hits {
        0 => false,
        5 => {
            let mean = corners.iter().flatten().sum::<f64>() / 4.0;
            (z[centre].unwrap() - mean).abs() > spec.tolerance
        }
        // Some touch and some don't: an edge of the part runs through.
        _ => true,
    };
    if !split {
        return;
    }
    let sides = [
        at(c0 + h, r0),
        at(c0 + size, r0 + h),
        at(c0 + h, r0 + size),
        at(c0, r0 + h),
    ];
    let missing: Vec<usize> = sides.into_iter().filter(|&i| !probed[i]).collect();
    if !missing.is_empty() {
        need.extend(missing);
        return;
    }
    for (c, r) in [(c0, r0), (c0 + h, r0), (c0, r0 + h), (c0 + h, r0 + h)] {
        refine(spec, z, probed, c, r, h, need);
    }
}

/// The cells a probed lattice makes, as (column, row, size): a cell is split in four where its
/// sides' midpoints were probed. For meshing; calls `f` with each cell that isn't split.
pub fn leaves(spec: &MapSpec, probed: &[bool], mut f: impl FnMut(usize, usize, usize)) {
    let (cols, rows) = spec.lattice();
    let s = spec.scale();
    fn walk(
        cols: usize,
        probed: &[bool],
        c0: usize,
        r0: usize,
        size: usize,
        f: &mut dyn FnMut(usize, usize, usize),
    ) {
        let h = size / 2;
        let at = |c: usize, r: usize| r * cols + c;
        let split = size >= 2
            && [
                at(c0 + h, r0),
                at(c0 + size, r0 + h),
                at(c0 + h, r0 + size),
                at(c0, r0 + h),
            ]
            .iter()
            .all(|&i| probed[i]);
        if !split {
            return f(c0, r0, size);
        }
        for (c, r) in [(c0, r0), (c0 + h, r0), (c0, r0 + h), (c0 + h, r0 + h)] {
            walk(cols, probed, c, r, h, f);
        }
    }
    for r0 in (0..rows - 1).step_by(s) {
        for c0 in (0..cols - 1).step_by(s) {
            walk(cols, probed, c0, r0, s, &mut f);
        }
    }
}

/// The probed points around a cell's edge, counter-clockwise from its front-left corner.
pub fn rim(spec: &MapSpec, probed: &[bool], c0: usize, r0: usize, size: usize) -> Vec<usize> {
    let (cols, _) = spec.lattice();
    let at = |c: usize, r: usize| r * cols + c;
    let mut out = vec![];
    out.extend((0..size).map(|k| at(c0 + k, r0)));
    out.extend((0..size).map(|k| at(c0 + size, r0 + k)));
    out.extend((0..size).map(|k| at(c0 + size - k, r0 + size)));
    out.extend((0..size).map(|k| at(c0, r0 + size - k)));
    out.retain(|&i| probed[i]);
    out
}

/// Probe what [`plan`] asks for, batch after batch, from what `map` has, until there's nothing
/// left or `pause` is set. Returns whether it finished (rather than paused). `lateral` says it's
/// the 3D probe, which feels sideways contact (allowing the low moves between points) and has a
/// known stylus length (limiting how deep each point may go: see [`Reach`]).
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
    let mut probed = vec![false; map.z.len()];
    for &i in &map.probed {
        probed[i as usize] = true;
    }
    let mut count = map.probed.len();
    // Wherever it was left, start from the top.
    rise(io, top).await?;
    let mut last: Option<(usize, f64)> = None;
    loop {
        let batch = plan(spec, &reach.z, &probed);
        if batch.is_empty() {
            break;
        }
        let _ = io.updates.send(Update::MapPending(batch.len() as u32));
        for (k, &index) in batch.iter().enumerate() {
            if pause.load(Ordering::Relaxed) {
                rise(io, top).await?;
                return Ok(false);
            }
            let started = Instant::now();
            count += 1;
            io.step(&format!(
                "Point {count}, {} more planned",
                batch.len() - k - 1
            ));
            let [x, y] = spec.at(index);
            let floor = reach.floor(index);
            // Low across from the last contact, but with the body clear of what's around both
            // ends.
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
                    io.travel([0.0, 0.0, 1.0 - LET_GO]).await?;
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
            // Nothing found, and it stopped short of the depth asked for: out of the stylus'
            // reach.
            let limited = z.is_none() && floor > top - spec.depth + 1e-6;
            reach.z[index] = z;
            probed[index] = true;
            last = z.map(|z| (index, z));
            let secs = started.elapsed().as_secs_f64();
            let _ = io.updates.send(Update::MapPoint {
                index,
                z,
                limited,
                secs,
            });
        }
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
        let grid_step = spec.step().map(|d| d * spec.scale() as f64);
        Self {
            spec,
            z: map.z.clone(),
            floor: map.top - spec.depth,
            stylus: stylus.then(|| {
                (
                    settings.stylus_length - BODY_MARGIN,
                    settings.body_diameter / 2.0 + grid_step[0].max(grid_step[1]),
                )
            }),
        }
    }

    /// The lowest the tip may go at lattice point `index`.
    fn floor(&self, index: usize) -> f64 {
        let Some((length, around)) = self.stylus else {
            return self.floor;
        };
        let highest = within(self.spec, index, around)
            .filter_map(|i| self.z[i])
            .fold(f64::NEG_INFINITY, f64::max);
        self.floor.max(highest - length)
    }
}

/// Lattice points within `radius` (mm) of lattice point `index`, itself included.
fn within(spec: &MapSpec, index: usize, radius: f64) -> impl Iterator<Item = usize> + '_ {
    let (cols, rows) = spec.lattice();
    let [dx, dy] = spec.step();
    let (dc, dr) = (
        ((radius / dx).floor() as usize).min(cols),
        ((radius / dy).floor() as usize).min(rows),
    );
    let (c, r) = (index % cols, index / cols);
    (r.saturating_sub(dr)..(r + dr + 1).min(rows)).flat_map(move |rr| {
        (c.saturating_sub(dc)..(c + dc + 1).min(cols)).filter_map(move |cc| {
            let (ox, oy) = ((cc as f64 - c as f64) * dx, (rr as f64 - r as f64) * dy);
            (ox.hypot(oy) <= radius + 1e-9).then_some(rr * cols + cc)
        })
    })
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
        if !io.let_go(back, io.settings.fast_feed, LET_GO).await? {
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

/// The measured surface at each lattice point, machine coordinates; `None` where there's no
/// contact (or no point). With `compensate`, corrected for the ball touching a slope off its
/// bottom: the ball centres lie on the surface grown by the radius, so each is moved back by the
/// radius along that surface's normal ([`slope`]). On the flat that's the tool tip.
pub fn surface(map: &SurfaceMap, ball_radius: f64, compensate: bool) -> Vec<Option<[f64; 3]>> {
    let spec = &map.spec;
    (0..map.z.len())
        .map(|i| {
            let h = map.z[i]?;
            let [x, y] = spec.at(i);
            if !compensate {
                return Some([x, y, h]);
            }
            let [gx, gy] = slope(spec, &map.z, i);
            let len = (gx * gx + gy * gy + 1.0).sqrt();
            // Ball centre, then back along the normal (−gx, −gy, 1) / len.
            Some([
                x + ball_radius * gx / len,
                y + ball_radius * gy / len,
                h + ball_radius - ball_radius / len,
            ])
        })
        .collect()
}

/// The measured surface's slope (dz/dx, dz/dy) at lattice point `i`: the least-squares plane
/// through it and the nearest points with contact, within its own level's spacing (on a plain
/// grid, the four next to it: central differences), or further out where those don't fix a
/// plane. Mirrored by `surfacePoints` in the app.
pub fn slope(spec: &MapSpec, z: &[Option<f64>], i: usize) -> [f64; 2] {
    let (cols, _) = spec.lattice();
    let (c, r) = (i % cols, i / cols);
    let [dx, dy] = spec.step();
    let h = z[i].unwrap_or(0.0);
    // The level the point belongs to: the largest power of two (up to a grid step) dividing both.
    let mut p = 1;
    while p < spec.scale() && c % (p * 2) == 0 && r % (p * 2) == 0 {
        p *= 2;
    }
    let grid = spec.scale() as f64 * dx.max(dy);
    let mut radius = p as f64 * dx.max(dy) * 1.01;
    let mut sums = [0.0; 5];
    while radius <= grid * 2.0 {
        // Σdx², Σdx·dy, Σdy², Σdx·dz, Σdy·dz.
        sums = [0.0; 5];
        for j in within(spec, i, radius) {
            let Some(zj) = z[j].filter(|_| j != i) else {
                continue;
            };
            let ox = ((j % cols) as f64 - c as f64) * dx;
            let oy = ((j / cols) as f64 - r as f64) * dy;
            let oz = zj - h;
            sums = [
                sums[0] + ox * ox,
                sums[1] + ox * oy,
                sums[2] + oy * oy,
                sums[3] + ox * oz,
                sums[4] + oy * oz,
            ];
        }
        let det = sums[0] * sums[2] - sums[1] * sums[1];
        if det > 1e-9 * (sums[0] + sums[2]).powi(2) && sums[0] > 0.0 && sums[2] > 0.0 {
            return [
                (sums[3] * sums[2] - sums[4] * sums[1]) / det,
                (sums[4] * sums[0] - sums[3] * sums[1]) / det,
            ];
        }
        radius *= std::f64::consts::SQRT_2;
    }
    // Only along one line (or nothing): the slope along it.
    [
        if sums[0] > 0.0 {
            sums[3] / sums[0]
        } else {
            0.0
        },
        if sums[2] > 0.0 {
            sums[4] / sums[2]
        } else {
            0.0
        },
    ]
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
    /// A closed polyline (contours only).
    Dxf,
}

/// `points` (as from [`surface`], shifted to work coordinates) in `format`. `probed` says which
/// lattice points were probed, for the mesh.
pub fn export(
    spec: &MapSpec,
    probed: &[bool],
    points: &[Option<[f64; 3]>],
    format: ExportFormat,
) -> Vec<u8> {
    match format {
        ExportFormat::Csv => delimited(points, ','),
        ExportFormat::Tsv => delimited(points, '\t'),
        ExportFormat::Stl => stl(spec, probed, points),
        // Refused before it gets here: a map isn't a polyline.
        ExportFormat::Dxf => Vec::new(),
    }
}

fn delimited(points: &[Option<[f64; 3]>], sep: char) -> Vec<u8> {
    let mut out = format!("x{sep}y{sep}z\n");
    for [x, y, z] in points.iter().flatten() {
        out.push_str(&format!("{x:.4}{sep}{y:.4}{sep}{z:.4}\n"));
    }
    out.into_bytes()
}

/// Each cell ([`leaves`]) as a fan: from its centre where that has contact, round the points on
/// its edge (finer neighbours' included, so there are no gaps); otherwise across its edge points.
/// Points without contact are left out, and the triangles that would use them.
fn stl(spec: &MapSpec, probed: &[bool], points: &[Option<[f64; 3]>]) -> Vec<u8> {
    let (cols, _) = spec.lattice();
    let mut tris: Vec<[[f64; 3]; 3]> = vec![];
    leaves(spec, probed, |c0, r0, size| {
        let edge = rim(spec, probed, c0, r0, size);
        let centre = (size >= 2)
            .then(|| points[(r0 + size / 2) * cols + c0 + size / 2])
            .flatten();
        // Counter-clockwise from above, so the normals face up.
        if let Some(m) = centre {
            for k in 0..edge.len() {
                if let (Some(a), Some(b)) = (points[edge[k]], points[edge[(k + 1) % edge.len()]]) {
                    tris.push([m, a, b]);
                }
            }
        } else {
            let hits: Vec<[f64; 3]> = edge.iter().filter_map(|&i| points[i]).collect();
            for k in 1..hits.len().saturating_sub(1) {
                tris.push([hits[0], hits[k], hits[k + 1]]);
            }
        }
    });
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
            levels: 0,
            tolerance: 0.0,
        }
    }

    /// Plan and "probe" `height` until done; the lattice points probed.
    fn run(
        spec: &MapSpec,
        height: impl Fn(f64, f64) -> Option<f64>,
    ) -> (Vec<Option<f64>>, Vec<bool>, usize) {
        let (cols, rows) = spec.lattice();
        let mut z = vec![None; cols * rows];
        let mut probed = vec![false; cols * rows];
        let mut batches = 0;
        loop {
            let batch = plan(spec, &z, &probed);
            if batch.is_empty() {
                return (z, probed, batches);
            }
            batches += 1;
            for i in batch {
                assert!(!probed[i], "planned twice: {i}");
                let [x, y] = spec.at(i);
                z[i] = height(x, y);
                probed[i] = true;
            }
        }
    }

    #[test]
    fn snakes_through_the_grid() {
        let s = spec(3, 3);
        let none = vec![false; 9];
        assert_eq!(plan(&s, &[None; 9], &none), [0, 1, 2, 5, 4, 3, 6, 7, 8]);
        assert_eq!(s.at(5), [10.0, 10.0]);
        // Carrying on after three points keeps to the same order.
        let mut some = none.clone();
        for i in [0, 1, 2] {
            some[i] = true;
        }
        assert_eq!(plan(&s, &[None; 9], &some), [5, 4, 3, 6, 7, 8]);
    }

    #[test]
    fn refines_only_where_the_surface_bends() {
        let s = MapSpec {
            max: [40.0, 40.0],
            columns: 5,
            rows: 5,
            levels: 3,
            tolerance: 0.01,
            ..spec(5, 5)
        };
        // A tilted plane: bilinear everywhere, so just the grid and the centres.
        let (_, probed, batches) = run(&s, |x, y| Some(0.1 * x - 0.05 * y));
        assert_eq!(probed.iter().filter(|p| **p).count(), 25 + 16);
        assert_eq!(batches, 2);
        // A bump near one corner: finer there, down to the lattice, and nowhere else.
        let bump = |x: f64, y: f64| Some((-((x - 5.0).powi(2) + (y - 5.0).powi(2)) / 8.0).exp());
        let (_, probed, _) = run(&s, bump);
        let (cols, _) = s.lattice();
        let n = probed.iter().filter(|p| **p).count();
        assert!(n > 41 && n < cols * cols / 4, "{n} points");
        let near = |i: usize| s.at(i)[0] < 20.0 && s.at(i)[1] < 20.0;
        let fine = |i: usize| (i % cols) % 2 == 1 || (i / cols) % 2 == 1;
        assert!((0..probed.len()).any(|i| probed[i] && fine(i) && near(i)));
        assert!(!(0..probed.len()).any(|i| probed[i] && fine(i) && !near(i)));
        // An edge (a step down to nothing): refined along it.
        let (_, probed, _) = run(&s, |x, _| (x < 13.0).then_some(1.0));
        assert!((0..probed.len()).any(|i| probed[i] && fine(i) && (s.at(i)[0] - 13.0).abs() < 1.0));
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
        // And on a refined lattice, where neighbours are at different distances.
        let s = MapSpec {
            max: [40.0, 40.0],
            levels: 2,
            tolerance: 0.01,
            ..spec(3, 3)
        };
        let bump = |x: f64, y: f64| {
            Some((-((x - 10.0).powi(2) + (y - 10.0).powi(2)) / 50.0).exp() * 10.0 + 0.3 * x)
        };
        let (z, probed, _) = run(&s, bump);
        let (cols, _) = s.lattice();
        for i in (0..z.len()).filter(|&i| probed[i]) {
            let [x, y] = s.at(i);
            let g = slope(&s, &z, i);
            // Roughly the true slope: finite differences at the local spacing.
            let e = 0.01;
            let want = [
                (bump(x + e, y).unwrap() - bump(x - e, y).unwrap()) / (2.0 * e),
                (bump(x, y + e).unwrap() - bump(x, y - e).unwrap()) / (2.0 * e),
            ];
            assert!(
                (g[0] - want[0]).abs() < 1.0 && (g[1] - want[1]).abs() < 1.0,
                "at {} {}: {g:?} vs {want:?}",
                i % cols,
                i / cols
            );
        }
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
        let probed = vec![true; 4];
        let csv = String::from_utf8(export(&s, &probed, &points, ExportFormat::Csv)).unwrap();
        assert_eq!(csv.lines().count(), 4);
        assert!(csv.starts_with("x,y,z\n0.0000,0.0000,1.0000\n"));
        let stl = export(&s, &probed, &points, ExportFormat::Stl);
        // One triangle (three corners), and its normal points up.
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 1);
        assert_eq!(stl.len(), 84 + 50);
        assert!(f32::from_le_bytes(stl[92..96].try_into().unwrap()) > 0.0);
    }

    #[test]
    fn meshes_refined_cells_without_gaps() {
        // A bump in one cell of a 3 × 3 grid, refined twice.
        let s = MapSpec {
            max: [20.0, 20.0],
            levels: 2,
            tolerance: 0.01,
            ..spec(3, 3)
        };
        let (z, probed, _) = run(&s, |x, y| {
            Some(if x < 10.0 && y < 10.0 {
                ((x - 5.0).powi(2) + (y - 5.0).powi(2)) / 10.0
            } else {
                2.5 + 0.0 * x
            })
        });
        let map = SurfaceMap {
            z,
            probed: (0..probed.len())
                .filter(|&i| probed[i])
                .map(|i| i as u32)
                .collect(),
            ..SurfaceMap::new(s.clone(), 10.0)
        };
        let points = surface(&map, 1.0, false);
        let stl = export(&s, &probed, &points, ExportFormat::Stl);
        let n = u32::from_le_bytes(stl[80..84].try_into().unwrap()) as usize;
        // Every triangle's area adds up to the whole 20 × 20 area seen from above, with all
        // normals up: no gaps or overlaps.
        let mut area = 0.0;
        for t in 0..n {
            let f = |k: usize| {
                f32::from_le_bytes(
                    stl[84 + t * 50 + k * 4..84 + t * 50 + k * 4 + 4]
                        .try_into()
                        .unwrap(),
                ) as f64
            };
            let (a, b, c) = ([f(3), f(4)], [f(6), f(7)], [f(9), f(10)]);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross > 0.0, "triangle {t} faces down");
            area += cross / 2.0;
        }
        assert!((area - 400.0).abs() < 1e-3, "{area}");
    }
}
