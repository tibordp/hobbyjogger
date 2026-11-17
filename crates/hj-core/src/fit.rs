//! Fitting a drawing's outline to a traced contour: where the drawing's origin is on the bed, and
//! how it's turned (a rigid fit: no scaling or mirroring).
//!
//! It fits where the ball's centre was at each touch to the outline offset by the ball's radius
//! (outward round a part, into a hole), which is exactly where the centres lie: round an outside
//! corner, an arc. That needs no estimate of the wall's direction at each point, as compensating
//! the points would, which goes astray at corners and on sparse points.
//!
//! Iterative closest point, on the traced points: each is paired with the nearest point on the
//! outline, and the small rotation and offset that best close the gaps across the outline (point
//! to line: points may slide along a straight edge, which matters little and would otherwise make
//! it settle slowly) is solved for, and again, until it settles. It starts from 12 angles with the outlines' centres together, so a part at any angle
//! is found, first on a thinned-out outline and then, for the best few, on the whole of it.
//! Distinct results are kept: a symmetrical part (a rectangle, say) fits as well at more than one
//! angle, and which is meant is the user's call.

use serde::Serialize;
use ts_rs::TS;

type P = [f64; 2];

/// One way the drawing fits the trace.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Fit {
    /// How far the drawing is turned, degrees counter-clockwise: rotating a program drawn like it
    /// by this (about its origin) lines it up.
    pub angle: f64,
    /// Where the drawing's origin is: machine X/Y, for the work origin.
    pub origin: [f64; 2],
    /// How far the traced points are from the fitted outline: root mean square, and the most.
    pub rms: f64,
    pub max: f64,
}

/// Machine → drawing: rotate by `phi`, then add `t`.
#[derive(Clone, Copy, Debug)]
struct Rigid {
    phi: f64,
    t: P,
}

impl Rigid {
    fn apply(&self, p: P) -> P {
        let (s, c) = self.phi.sin_cos();
        [
            p[0] * c - p[1] * s + self.t[0],
            p[0] * s + p[1] * c + self.t[1],
        ]
    }
}

/// The nearest point to `p` on the closed polyline `poly`, and the normal of the segment it's on.
fn nearest_with_normal(poly: &[P], p: P) -> (P, P) {
    let mut best = (poly[0], [0.0, 0.0]);
    let mut best_d = f64::INFINITY;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let u = if l2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + u * dx, a[1] + u * dy];
        let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
        if d < best_d {
            best_d = d;
            let l = l2.sqrt().max(1e-12);
            best = (q, [-dy / l, dx / l]);
        }
    }
    best
}

/// The outline offset by `offset` (out of the region it encloses when positive, into it when
/// negative) near `p`: the nearest point on it, and the direction across it there. `ccw` says which
/// way round the outline goes, so which side is out.
fn on_offset(poly: &[P], ccw: bool, offset: f64, p: P) -> (P, P) {
    let (q, left) = nearest_with_normal(poly, p);
    if offset == 0.0 {
        return (q, left);
    }
    // Out of the region is to the right of a counter-clockwise outline's segments.
    let out = if ccw { [-left[0], -left[1]] } else { left };
    let side = if offset > 0.0 {
        out
    } else {
        [-out[0], -out[1]]
    };
    let (dx, dy) = (p[0] - q[0], p[1] - q[1]);
    let d = dx.hypot(dy);
    // On the ball's side, straight from the nearest point (round an outside corner, that's the
    // arc); otherwise square to the segment.
    let dir = if dx * side[0] + dy * side[1] > 0.0 && d > 1e-9 {
        [dx / d, dy / d]
    } else {
        side
    };
    let r = offset.abs();
    ([q[0] + r * dir[0], q[1] + r * dir[1]], dir)
}

/// The small rotation (about the points' centre) and offset that best close the gaps between each
/// `from` and the line through its `to` with normal `normal`, linearised; as a rigid motion.
fn solve_lines(from: &[P], to: &[(P, P)]) -> Rigid {
    let n = from.len() as f64;
    let c = from
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
    // Normal equations for (θ, tx, ty): residual (q − c_i)·n + θ·(q − c)×n + t·n.
    let mut m = [[0.0; 3]; 3];
    let mut v = [0.0; 3];
    for (q, (near, nrm)) in from.iter().zip(to) {
        let (rx, ry) = (q[0] - c[0], q[1] - c[1]);
        let row = [rx * nrm[1] - ry * nrm[0], nrm[0], nrm[1]];
        let res = (q[0] - near[0]) * nrm[0] + (q[1] - near[1]) * nrm[1];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            v[i] -= row[i] * res;
        }
    }
    let Some([theta, tx, ty]) = solve3(m, v) else {
        return Rigid {
            phi: 0.0,
            t: [0.0, 0.0],
        };
    };
    // Turn by θ about c, then move by t.
    let (s, co) = theta.sin_cos();
    Rigid {
        phi: theta,
        t: [
            c[0] - (c[0] * co - c[1] * s) + tx,
            c[1] - (c[0] * s + c[1] * co) + ty,
        ],
    }
}

/// `m x = v` by Cramer's rule, if `m` isn't (nearly) singular.
fn solve3(m: [[f64; 3]; 3], v: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    let scale = m
        .iter()
        .flatten()
        .map(|x| x.abs())
        .fold(0.0, f64::max)
        .powi(3);
    if d.abs() <= 1e-12 * scale.max(1e-300) {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut a = m;
        for i in 0..3 {
            a[i][k] = v[i];
        }
        *o = det(a) / d;
    }
    Some(out)
}

/// Refine `r` by iterating; returns it and the distances at the end.
fn icp(
    reference: &[P],
    offset: f64,
    measured: &[P],
    mut r: Rigid,
    rounds: usize,
) -> (Rigid, Vec<f64>) {
    let ccw = signed_area(reference) > 0.0;
    for _ in 0..rounds {
        let moved: Vec<P> = measured.iter().map(|&m| r.apply(m)).collect();
        let near: Vec<(P, P)> = moved
            .iter()
            .map(|&q| on_offset(reference, ccw, offset, q))
            .collect();
        let step = solve_lines(&moved, &near);
        // Compose: first r, then the step.
        let next = Rigid {
            phi: r.phi + step.phi,
            t: step.apply(r.t),
        };
        let settled = step.phi.abs() < 1e-9 && step.t[0].hypot(step.t[1]) < 1e-7;
        r = next;
        if settled {
            break;
        }
    }
    let d = measured.iter().map(|&m| {
        let q = r.apply(m);
        let (n, _) = on_offset(reference, ccw, offset, q);
        (q[0] - n[0]).hypot(q[1] - n[1])
    });
    (r, d.collect())
}

fn signed_area(poly: &[P]) -> f64 {
    (0..poly.len())
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// The centre of a closed polygon's area (or of its points, if it has none).
fn centroid(poly: &[P]) -> P {
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let w = p[0] * q[1] - q[0] * p[1];
        a += w;
        cx += (p[0] + q[0]) * w;
        cy += (p[1] + q[1]) * w;
    }
    if a.abs() < 1e-12 {
        let n = poly.len() as f64;
        return poly
            .iter()
            .fold([0.0, 0.0], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n]);
    }
    [cx / (3.0 * a), cy / (3.0 * a)]
}

/// Every `k`th point, for at most `most`.
fn thin(points: &[P], most: usize) -> Vec<P> {
    let k = points.len().div_ceil(most).max(1);
    points.iter().step_by(k).copied().collect()
}

/// The ways `reference` (a closed outline, drawing coordinates) fits `measured` (where the ball's
/// centre was at each touch, machine coordinates), best first: distinct ones only, at most four.
/// `offset` is the ball's radius: positive for a part traced from outside (the centres are outside
/// the outline), negative for a hole from inside, 0 for points already on the wall.
pub fn fit(reference: &[P], measured: &[P], offset: f64) -> Vec<Fit> {
    if reference.len() < 3 || measured.len() < 3 {
        return vec![];
    }
    let (rc, mc) = (centroid(reference), centroid(measured));
    let (coarse_ref, coarse_meas) = (thin(reference, 400), thin(measured, 100));
    let mut starts: Vec<(Rigid, f64)> = (0..12)
        .map(|k| {
            let phi = k as f64 * std::f64::consts::TAU / 12.0;
            let (s, c) = phi.sin_cos();
            let start = Rigid {
                phi,
                t: [
                    rc[0] - (mc[0] * c - mc[1] * s),
                    rc[1] - (mc[0] * s + mc[1] * c),
                ],
            };
            let (r, d) = icp(&coarse_ref, offset, &coarse_meas, start, 30);
            (r, rms(&d))
        })
        .collect();
    starts.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Several starts end up the same way; one of each, so a second way of fitting (a symmetrical
    // part's) isn't crowded out by the first.
    let mut distinct: Vec<Rigid> = vec![];
    for (r, _) in starts {
        let close = |d: &Rigid| {
            let da = (d.phi - r.phi).rem_euclid(std::f64::consts::TAU);
            da.min(std::f64::consts::TAU - da) < 2f64.to_radians()
                && (d.t[0] - r.t[0]).hypot(d.t[1] - r.t[1]) < 1.0
        };
        if !distinct.iter().any(close) {
            distinct.push(r);
        }
    }

    let mut fits: Vec<Fit> = vec![];
    for start in distinct.into_iter().take(4) {
        let (r, d) = icp(reference, offset, measured, start, 100);
        // Drawing → machine: the inverse.
        let angle = -r.phi;
        let (s, c) = angle.sin_cos();
        let origin = [-(r.t[0] * c - r.t[1] * s), -(r.t[0] * s + r.t[1] * c)];
        let f = Fit {
            angle: angle.to_degrees().rem_euclid(360.0),
            origin,
            rms: rms(&d),
            max: d.iter().copied().fold(0.0, f64::max),
        };
        let same = |g: &Fit| {
            let da = (g.angle - f.angle).rem_euclid(360.0);
            da.min(360.0 - da) < 1.0
                && (g.origin[0] - f.origin[0]).hypot(g.origin[1] - f.origin[1]) < 0.5
        };
        if !fits.iter().any(same) {
            fits.push(f);
        }
    }
    fits.sort_by(|a, b| a.rms.total_cmp(&b.rms));
    // Signed angles near zero read better: 359.5 is −0.5.
    for f in &mut fits {
        if f.angle > 180.0 {
            f.angle -= 360.0;
        }
    }
    fits.truncate(4);
    fits
}

fn rms(d: &[f64]) -> f64 {
    (d.iter().map(|v| v * v).sum::<f64>() / d.len().max(1) as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 40 × 20 plate with a 10 × 6 notch out of one long side: only one way round.
    fn plate() -> Vec<P> {
        outline(&[
            [0.0, 0.0],
            [40.0, 0.0],
            [40.0, 20.0],
            [25.0, 20.0],
            [25.0, 14.0],
            [15.0, 14.0],
            [15.0, 20.0],
            [0.0, 20.0],
        ])
    }

    /// Points every 0.5 mm round the polygon `corners`.
    fn outline(corners: &[P]) -> Vec<P> {
        let mut out = vec![];
        for i in 0..corners.len() {
            let (a, b): (P, P) = (corners[i], corners[(i + 1) % corners.len()]);
            let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / 0.5).ceil() as usize;
            for k in 0..n {
                let u = k as f64 / n as f64;
                out.push([a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u]);
            }
        }
        out
    }

    /// `points` as they'd be traced: drawing → machine by `angle` (degrees) and `origin`, every
    /// `step`th one, with a little noise.
    fn traced(points: &[P], angle: f64, origin: P, step: usize) -> Vec<P> {
        let (s, c) = angle.to_radians().sin_cos();
        points
            .iter()
            .step_by(step)
            .enumerate()
            .map(|(i, p)| {
                let wobble = 0.003 * ((i * 7919 % 13) as f64 / 6.0 - 1.0);
                [
                    p[0] * c - p[1] * s + origin[0] + wobble,
                    p[0] * s + p[1] * c + origin[1] - wobble,
                ]
            })
            .collect()
    }

    #[test]
    fn finds_how_the_part_sits() {
        let reference = plate();
        for (angle, origin) in [
            (23.0, [-150.0, -120.0]),
            (-131.5, [-60.0, -90.0]),
            (178.0, [-100.0, -100.0]),
        ] {
            let fits = fit(&reference, &traced(&reference, angle, origin, 5), 0.0);
            let best = &fits[0];
            let da = (best.angle - angle).rem_euclid(360.0);
            assert!(da.min(360.0 - da) < 0.02, "angle {} vs {angle}", best.angle);
            assert!(
                (best.origin[0] - origin[0]).abs() < 0.02
                    && (best.origin[1] - origin[1]).abs() < 0.02,
                "{best:?}"
            );
            assert!(best.rms < 0.01 && best.max < 0.02, "{best:?}");
        }
    }

    /// Where a ball of radius `r` rolling along the polygon `corners` on its right (outside a
    /// counter-clockwise polygon, inside a clockwise one) has its centre: each edge moved over by
    /// `r`, and an arc round each corner it goes round; none where it's closer than `r` to another
    /// edge (an inside corner).
    fn ball_centres(corners: &[P], r: f64) -> Vec<P> {
        let n = corners.len();
        let mut out = vec![];
        for i in 0..n {
            let (a, b) = (corners[i], corners[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy);
            let normal = [dy / l, -dx / l];
            let k = (l / 0.25) as usize;
            for j in 0..k {
                let u = j as f64 / k as f64;
                out.push([a[0] + dx * u + r * normal[0], a[1] + dy * u + r * normal[1]]);
            }
            // The arc round the corner at b, from this edge's normal to the next one's.
            let c = corners[(i + 2) % n];
            let (ex, ey) = (c[0] - b[0], c[1] - b[1]);
            let el = ex.hypot(ey);
            let next = [ey / el, -ex / el];
            let (a0, a1) = (normal[1].atan2(normal[0]), next[1].atan2(next[0]));
            let sweep = (a1 - a0 + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            if sweep > 0.0 {
                for j in 0..=8 {
                    let t = a0 + sweep * j as f64 / 8.0;
                    out.push([b[0] + r * t.cos(), b[1] + r * t.sin()]);
                }
            }
        }
        out.retain(|&p| {
            (0..n).all(|i| {
                let (a, b) = (corners[i], corners[(i + 1) % n]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let u = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy))
                    .clamp(0.0, 1.0);
                (a[0] + u * dx - p[0]).hypot(a[1] + u * dy - p[1]) > r - 1e-6
            })
        });
        out
    }

    #[test]
    fn fits_ball_centres_to_the_offset_outline() {
        let corners = [
            [0.0, 0.0],
            [40.0, 0.0],
            [40.0, 20.0],
            [25.0, 20.0],
            [25.0, 14.0],
            [15.0, 14.0],
            [15.0, 20.0],
            [0.0, 20.0],
        ];
        let (angle, origin, r) = (-37.0, [-120.0, -80.0], 1.0);
        let centres = traced(&ball_centres(&corners, r), angle, origin, 7);
        let reference = outline(&corners);
        let best = &fit(&reference, &centres, r)[0];
        assert!(
            (best.angle - angle).abs() < 0.01
                && (best.origin[0] - origin[0]).abs() < 0.01
                && (best.origin[1] - origin[1]).abs() < 0.01,
            "{best:?}"
        );
        assert!(best.rms < 0.005, "{best:?}");
        // A hole that shape, traced from inside: the same, going round the other way, with the
        // centres inside (and arcs round the notch, which now sticks into the hole).
        let reversed: Vec<P> = corners.iter().rev().copied().collect();
        let centres = traced(&ball_centres(&reversed, r), angle, origin, 7);
        let best = &fit(&reference, &centres, -r)[0];
        assert!(
            (best.angle - angle).abs() < 0.01
                && (best.origin[0] - origin[0]).abs() < 0.01
                && (best.origin[1] - origin[1]).abs() < 0.01,
            "{best:?}"
        );
        assert!(best.rms < 0.005, "{best:?}");
    }

    #[test]
    fn offers_the_other_ways_a_symmetrical_part_fits() {
        // A plain rectangle fits as well turned half way round.
        let rect = outline(&[[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]]);
        let fits = fit(&rect, &traced(&rect, 10.0, [-100.0, -100.0], 3), 0.0);
        let good: Vec<&Fit> = fits.iter().filter(|f| f.rms < 0.05).collect();
        assert!(good.len() >= 2, "{fits:?}");
        let mut angles: Vec<f64> = good.iter().map(|f| f.angle.rem_euclid(360.0)).collect();
        angles.sort_by(f64::total_cmp);
        assert!(
            angles.iter().any(|a| (a - 10.0).abs() < 0.1)
                && angles.iter().any(|a| (a - 190.0).abs() < 0.1),
            "{angles:?}"
        );
    }
}
