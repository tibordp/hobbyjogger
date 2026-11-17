//! Reading the outlines from a DXF drawing, for fitting one to a traced contour.
//!
//! Lines, arcs, circles and polylines (with arcs, as bulges, both the light and the old kind) are
//! read from the entities and from blocks (inserts expanded, moved, scaled and turned). Arcs are
//! turned into short straight segments, within [`CHORD`] of the arc. Pieces whose ends meet are
//! chained into outlines; an outline is closed when it comes back to where it started. Closed
//! outlines inside an odd number of others are holes. Splines, ellipses and anything else aren't
//! read: they're counted, so it can say so. Units come from `$INSUNITS` (unitless is taken as mm),
//! and everything comes out in mm.

use serde::Serialize;
use ts_rs::TS;

/// How far a straight segment may be from the arc it stands for, mm.
pub const CHORD: f64 = 0.005;
/// How close two ends must be to be chained, mm.
const JOIN: f64 = 0.01;

type P = [f64; 2];

/// A drawing's outlines, in mm, in the drawing's own coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Drawing {
    pub outlines: Vec<Outline>,
    /// What was there but not read, e.g. "3 SPLINE".
    pub skipped: Vec<String>,
    /// The units it was drawn in ("mm", "in", ...), converted from.
    pub units: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Outline {
    pub points: Vec<[f64; 2]>,
    /// Back where it started (the last point joins the first).
    pub closed: bool,
    /// Closed and inside another closed outline (or an odd number of them).
    pub hole: bool,
    /// Closed outlines: the area inside, mm².
    pub area: f64,
    pub layer: String,
}

/// One entity's worth of points.
#[derive(Clone, Debug)]
struct Piece {
    pts: Vec<P>,
    closed: bool,
    layer: String,
}

/// A group code and its value.
struct Pair<'a> {
    code: i32,
    value: &'a str,
}

fn pairs(text: &str) -> Result<Vec<Pair<'_>>, String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out = Vec::with_capacity(lines.len() / 2);
    for chunk in lines.chunks(2) {
        let [code, value] = chunk else { break };
        let code = code
            .parse()
            .map_err(|_| format!("not a DXF file (expected a group code, found {code:?})"))?;
        out.push(Pair { code, value });
    }
    Ok(out)
}

/// The points of an arc from angle `a0` sweeping `sweep` (radians, positive counter-clockwise)
/// about `c`, including both ends.
fn arc(c: P, r: f64, a0: f64, sweep: f64, chord: f64) -> Vec<P> {
    let step = if r > chord {
        2.0 * (1.0 - chord / r).acos()
    } else {
        std::f64::consts::PI / 2.0
    };
    let n = ((sweep.abs() / step).ceil() as usize).clamp(1, 2000);
    (0..=n)
        .map(|i| {
            let a = a0 + sweep * i as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// A polyline's points with its bulges turned into arcs. `bulge[i]` is for the segment from
/// vertex `i` to the next.
fn bulged(vertices: &[P], bulges: &[f64], closed: bool, chord: f64) -> Vec<P> {
    let n = vertices.len();
    let mut out = vec![];
    let segments = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segments {
        let (a, b) = (vertices[i], vertices[(i + 1) % n]);
        let bulge = bulges.get(i).copied().unwrap_or(0.0);
        if out.is_empty() {
            out.push(a);
        }
        if bulge.abs() < 1e-9 {
            out.push(b);
            continue;
        }
        // Included angle 4·atan(bulge), counter-clockwise when positive; the centre is off the
        // chord's middle along its left normal by c(1 − b²)/4b.
        let sweep = 4.0 * bulge.atan();
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let c = dx.hypot(dy);
        if c < 1e-12 {
            continue;
        }
        let m = c * (1.0 - bulge * bulge) / (4.0 * bulge);
        let centre = [
            (a[0] + b[0]) / 2.0 - dy / c * m,
            (a[1] + b[1]) / 2.0 + dx / c * m,
        ];
        let r = (a[0] - centre[0]).hypot(a[1] - centre[1]);
        let a0 = (a[1] - centre[1]).atan2(a[0] - centre[0]);
        let mut pts = arc(centre, r, a0, sweep, chord);
        // Exactly onto the next vertex.
        *pts.last_mut().unwrap() = b;
        out.extend(pts.into_iter().skip(1));
    }
    if closed && out.len() > 1 && dist(out[0], *out.last().unwrap()) < 1e-9 {
        out.pop();
    }
    out
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// An insert: where a block goes, scaled and turned.
#[derive(Clone, Copy)]
struct Place {
    at: P,
    scale: P,
    degrees: f64,
}

impl Place {
    fn apply(&self, p: P, base: P) -> P {
        let (x, y) = (
            (p[0] - base[0]) * self.scale[0],
            (p[1] - base[1]) * self.scale[1],
        );
        let (s, c) = self.degrees.to_radians().sin_cos();
        [self.at[0] + x * c - y * s, self.at[1] + x * s + y * c]
    }
}

/// What a section's entities make: pieces, inserts of blocks, and what was skipped.
#[derive(Default)]
struct Entities {
    pieces: Vec<Piece>,
    inserts: Vec<(String, Place)>,
    skipped: Vec<String>,
}

/// Read entities from `pairs` (starting at an entity's `0` pair) until one of `stop` (an `0` pair).
fn entities(pairs: &[Pair], i: &mut usize, stop: &[&str], chord: f64) -> Entities {
    let mut out = Entities::default();
    while *i < pairs.len() {
        let kind = pairs[*i].value;
        if pairs[*i].code != 0 {
            *i += 1;
            continue;
        }
        if stop.contains(&kind) {
            return out;
        }
        // This entity's pairs, up to the next `0`.
        let start = *i + 1;
        let mut end = start;
        while end < pairs.len() && pairs[end].code != 0 {
            end += 1;
        }
        let body = &pairs[start..end];
        let num = |code: i32| {
            body.iter()
                .find(|p| p.code == code)
                .and_then(|p| p.value.parse::<f64>().ok())
        };
        let layer = body
            .iter()
            .find(|p| p.code == 8)
            .map_or("0", |p| p.value)
            .to_string();
        *i = end;
        match kind {
            "LINE" => {
                if let (Some(x0), Some(y0), Some(x1), Some(y1)) =
                    (num(10), num(20), num(11), num(21))
                {
                    out.pieces.push(Piece {
                        pts: vec![[x0, y0], [x1, y1]],
                        closed: false,
                        layer,
                    });
                }
            }
            "CIRCLE" | "ARC" => {
                let (Some(x), Some(y), Some(r)) = (num(10), num(20), num(40)) else {
                    continue;
                };
                if kind == "CIRCLE" {
                    let mut pts = arc([x, y], r, 0.0, std::f64::consts::TAU, chord);
                    pts.pop();
                    out.pieces.push(Piece {
                        pts,
                        closed: true,
                        layer,
                    });
                } else {
                    let (a0, a1) = (num(50).unwrap_or(0.0), num(51).unwrap_or(360.0));
                    let mut sweep = a1 - a0;
                    while sweep <= 0.0 {
                        sweep += 360.0;
                    }
                    let pts = arc([x, y], r, a0.to_radians(), sweep.to_radians(), chord);
                    out.pieces.push(Piece {
                        pts,
                        closed: false,
                        layer,
                    });
                }
            }
            "LWPOLYLINE" => {
                let closed = num(70).is_some_and(|f| f as i64 & 1 == 1);
                let (mut vertices, mut bulges) = (vec![], vec![]);
                let mut x = None;
                for p in body {
                    match p.code {
                        10 => x = p.value.parse::<f64>().ok(),
                        20 => {
                            if let (Some(x), Ok(y)) = (x, p.value.parse::<f64>()) {
                                vertices.push([x, y]);
                                bulges.push(0.0);
                            }
                        }
                        42 => {
                            if let (Some(b), Ok(v)) = (bulges.last_mut(), p.value.parse::<f64>()) {
                                *b = v;
                            }
                        }
                        _ => {}
                    }
                }
                if vertices.len() > 1 {
                    out.pieces.push(Piece {
                        pts: bulged(&vertices, &bulges, closed, chord),
                        closed,
                        layer,
                    });
                }
            }
            "POLYLINE" => {
                let flags = num(70).unwrap_or(0.0) as i64;
                let closed = flags & 1 == 1;
                // Its vertices follow as entities of their own, to a SEQEND.
                let (mut vertices, mut bulges) = (vec![], vec![]);
                while *i < pairs.len() && pairs[*i].code == 0 && pairs[*i].value == "VERTEX" {
                    let s = *i + 1;
                    let mut e = s;
                    while e < pairs.len() && pairs[e].code != 0 {
                        e += 1;
                    }
                    let v = &pairs[s..e];
                    let num = |code: i32| {
                        v.iter()
                            .find(|p| p.code == code)
                            .and_then(|p| p.value.parse::<f64>().ok())
                    };
                    if let (Some(x), Some(y)) = (num(10), num(20)) {
                        vertices.push([x, y]);
                        bulges.push(num(42).unwrap_or(0.0));
                    }
                    *i = e;
                }
                if *i < pairs.len() && pairs[*i].value == "SEQEND" {
                    *i += 1;
                }
                // Meshes and faces aren't outlines.
                if flags & (16 | 64) != 0 {
                    out.skipped.push("POLYLINE mesh".into());
                } else if vertices.len() > 1 {
                    out.pieces.push(Piece {
                        pts: bulged(&vertices, &bulges, closed, chord),
                        closed,
                        layer,
                    });
                }
            }
            "INSERT" => {
                let name = body
                    .iter()
                    .find(|p| p.code == 2)
                    .map(|p| p.value.to_string());
                if let Some(name) = name {
                    let place = Place {
                        at: [num(10).unwrap_or(0.0), num(20).unwrap_or(0.0)],
                        scale: [num(41).unwrap_or(1.0), num(42).unwrap_or(1.0)],
                        degrees: num(50).unwrap_or(0.0),
                    };
                    out.inserts.push((name, place));
                }
            }
            // Bookkeeping, not geometry.
            "SEQEND" | "ATTRIB" | "ATTDEF" | "VIEWPORT" => {}
            other => out.skipped.push(other.into()),
        }
    }
    out
}

/// A block: its base point and contents.
struct Block {
    base: P,
    contents: Entities,
}

/// Read `text` as a DXF drawing.
pub fn parse(text: &str) -> Result<Drawing, String> {
    let pairs = pairs(text)?;
    let section = |name: &str| {
        pairs
            .windows(2)
            .position(|w| {
                w[0].code == 0 && w[0].value == "SECTION" && w[1].code == 2 && w[1].value == name
            })
            .map(|k| k + 2)
    };

    // Units, from the header.
    let (scale, units) = match section("HEADER")
        .and_then(|k| {
            pairs[k..]
                .windows(2)
                .find(|w| w[0].code == 9 && w[0].value == "$INSUNITS")
                .map(|w| w[1].value)
        })
        .and_then(|v| v.parse::<i32>().ok())
    {
        Some(1) => (25.4, "in"),
        Some(2) => (304.8, "ft"),
        Some(5) => (10.0, "cm"),
        Some(6) => (1000.0, "m"),
        Some(4) => (1.0, "mm"),
        _ => (1.0, "mm (unitless)"),
    };
    let chord = CHORD / scale;

    let mut blocks = std::collections::HashMap::new();
    if let Some(mut k) = section("BLOCKS") {
        while k < pairs.len() && !(pairs[k].code == 0 && pairs[k].value == "ENDSEC") {
            if pairs[k].code == 0 && pairs[k].value == "BLOCK" {
                let s = k + 1;
                let mut e = s;
                while e < pairs.len() && pairs[e].code != 0 {
                    e += 1;
                }
                let head = &pairs[s..e];
                let name = head
                    .iter()
                    .find(|p| p.code == 2)
                    .map_or(String::new(), |p| p.value.to_string());
                let num = |code: i32| {
                    head.iter()
                        .find(|p| p.code == code)
                        .and_then(|p| p.value.parse::<f64>().ok())
                };
                let base = [num(10).unwrap_or(0.0), num(20).unwrap_or(0.0)];
                k = e;
                let contents = entities(&pairs, &mut k, &["ENDBLK", "ENDSEC"], chord);
                blocks.insert(name, Block { base, contents });
            } else {
                k += 1;
            }
        }
    }

    let mut k = section("ENTITIES").ok_or("no ENTITIES section: is it a DXF drawing?")?;
    let top = entities(&pairs, &mut k, &["ENDSEC"], chord);

    // Pieces with inserts expanded (blocks within blocks too, a few deep).
    let mut pieces = top.pieces.clone();
    let mut skipped = top.skipped.clone();
    fn expand(
        blocks: &std::collections::HashMap<String, Block>,
        inserts: &[(String, Place)],
        outer: &dyn Fn(P) -> P,
        depth: u32,
        pieces: &mut Vec<Piece>,
        skipped: &mut Vec<String>,
    ) {
        if depth > 8 {
            return;
        }
        for (name, place) in inserts {
            let Some(block) = blocks.get(name) else {
                skipped.push(format!("INSERT of a missing block {name}"));
                continue;
            };
            let (place, base) = (*place, block.base);
            let f = move |p: P| outer(place.apply(p, base));
            for piece in &block.contents.pieces {
                pieces.push(Piece {
                    pts: piece.pts.iter().map(|&p| f(p)).collect(),
                    ..piece.clone()
                });
            }
            skipped.extend(block.contents.skipped.iter().cloned());
            expand(
                blocks,
                &block.contents.inserts,
                &f,
                depth + 1,
                pieces,
                skipped,
            );
        }
    }
    expand(&blocks, &top.inserts, &|p| p, 0, &mut pieces, &mut skipped);

    for p in &mut pieces {
        for v in &mut p.pts {
            *v = [v[0] * scale, v[1] * scale];
        }
    }
    let mut outlines = chain(pieces);
    mark_holes(&mut outlines);

    // "3 SPLINE, 1 ELLIPSE"
    let mut counts: Vec<(String, usize)> = vec![];
    for s in skipped {
        match counts.iter_mut().find(|(k, _)| *k == s) {
            Some((_, n)) => *n += 1,
            None => counts.push((s, 1)),
        }
    }
    Ok(Drawing {
        outlines,
        skipped: counts
            .into_iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect(),
        units: units.into(),
    })
}

/// Chain open pieces end to end into outlines; closed pieces are outlines as they are.
fn chain(pieces: Vec<Piece>) -> Vec<Outline> {
    let (closed, mut open): (Vec<Piece>, Vec<Piece>) = pieces.into_iter().partition(|p| p.closed);
    let mut out: Vec<Outline> = closed
        .into_iter()
        .map(|p| outline(p.pts, true, p.layer))
        .collect();
    while let Some(first) = open.pop() {
        let layer = first.layer.clone();
        let mut pts = first.pts;
        loop {
            if pts.len() > 2 && dist(pts[0], *pts.last().unwrap()) < JOIN {
                pts.pop();
                out.push(outline(pts, true, layer));
                break;
            }
            let end = *pts.last().unwrap();
            let start = pts[0];
            // Something that carries on from either end.
            let found = open.iter().position(|p| {
                let (a, b) = (p.pts[0], *p.pts.last().unwrap());
                dist(a, end) < JOIN
                    || dist(b, end) < JOIN
                    || dist(a, start) < JOIN
                    || dist(b, start) < JOIN
            });
            let Some(k) = found else {
                out.push(outline(pts, false, layer));
                break;
            };
            let mut next = open.swap_remove(k).pts;
            let (a, b) = (next[0], *next.last().unwrap());
            if dist(a, end) < JOIN {
                pts.extend(next.into_iter().skip(1));
            } else if dist(b, end) < JOIN {
                next.reverse();
                pts.extend(next.into_iter().skip(1));
            } else if dist(b, start) < JOIN {
                next.pop();
                next.extend(pts);
                pts = next;
            } else {
                next.reverse();
                next.pop();
                next.extend(pts);
                pts = next;
            }
        }
    }
    out
}

fn signed_area(pts: &[P]) -> f64 {
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

fn outline(points: Vec<P>, closed: bool, layer: String) -> Outline {
    let area = if closed {
        signed_area(&points).abs()
    } else {
        0.0
    };
    Outline {
        points,
        closed,
        hole: false,
        area,
        layer,
    }
}

/// Whether `p` is inside the closed polygon `poly` (even-odd).
pub fn inside(poly: &[P], p: P) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            odd = !odd;
        }
    }
    odd
}

fn mark_holes(outlines: &mut [Outline]) {
    let closed: Vec<(usize, f64)> = outlines
        .iter()
        .enumerate()
        .filter(|(_, o)| o.closed)
        .map(|(i, o)| (i, o.area))
        .collect();
    for &(i, area) in &closed {
        let p = outlines[i].points[0];
        let around = closed
            .iter()
            .filter(|&&(j, a)| j != i && a > area && inside(&outlines[j].points, p))
            .count();
        outlines[i].hole = around % 2 == 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DXF from (code, value) pairs.
    fn dxf(pairs: &[(i32, &str)]) -> String {
        pairs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
    }

    fn entities(body: &[(i32, &str)]) -> String {
        let mut all = vec![(0, "SECTION"), (2, "ENTITIES")];
        all.extend_from_slice(body);
        all.extend([(0, "ENDSEC"), (0, "EOF")]);
        dxf(&all)
    }

    #[test]
    fn chains_lines_and_arcs_into_a_closed_outline() {
        // A 20 × 10 slot: two lines and two half circles, drawn in no particular order or
        // direction.
        let text = entities(&[
            (0, "LINE"),
            (8, "cut"),
            (10, "0"),
            (20, "0"),
            (11, "20"),
            (21, "0"),
            (0, "ARC"),
            (10, "0"),
            (20, "5"),
            (40, "5"),
            (50, "90"),
            (51, "270"),
            (0, "LINE"),
            (10, "0"),
            (20, "10"),
            (11, "20"),
            (21, "10"),
            (0, "ARC"),
            (10, "20"),
            (20, "5"),
            (40, "5"),
            (50, "270"),
            (51, "90"),
            (0, "SPLINE"),
            (10, "1"),
        ]);
        let d = parse(&text).unwrap();
        assert_eq!(d.outlines.len(), 1, "{:?}", d.outlines);
        let o = &d.outlines[0];
        assert!(o.closed && !o.hole);
        // 20 × 10 rectangle plus a 5-radius circle.
        let want = 200.0 + std::f64::consts::PI * 25.0;
        // The chords cut inside the arcs by up to CHORD, about ⅔ of that over their length.
        assert!((o.area - want).abs() < 0.2, "{} vs {want}", o.area);
        assert_eq!(d.skipped, ["1 SPLINE"]);
    }

    #[test]
    fn reads_bulged_polylines_holes_blocks_and_units() {
        let text = dxf(&[
            (0, "SECTION"),
            (2, "HEADER"),
            (9, "$INSUNITS"),
            (70, "1"),
            (0, "ENDSEC"),
            (0, "SECTION"),
            (2, "BLOCKS"),
            (0, "BLOCK"),
            (2, "HOLE"),
            (10, "0"),
            (20, "0"),
            (0, "CIRCLE"),
            (10, "0"),
            (20, "0"),
            (40, "0.1"),
            (0, "ENDBLK"),
            (0, "ENDSEC"),
            (0, "SECTION"),
            (2, "ENTITIES"),
            // A 2 × 1 inch plate whose right end is a half circle (bulge 1 from (2, 0) to (2, 1)).
            (0, "LWPOLYLINE"),
            (90, "4"),
            (70, "1"),
            (10, "0"),
            (20, "0"),
            (10, "2"),
            (20, "0"),
            (42, "1"),
            (10, "2"),
            (20, "1"),
            (10, "0"),
            (20, "1"),
            (0, "INSERT"),
            (2, "HOLE"),
            (10, "0.5"),
            (20, "0.5"),
            (0, "ENDSEC"),
            (0, "EOF"),
        ]);
        let d = parse(&text).unwrap();
        assert_eq!(d.units, "in");
        assert_eq!(d.outlines.len(), 2);
        let plate = d.outlines.iter().find(|o| !o.hole).unwrap();
        let hole = d.outlines.iter().find(|o| o.hole).unwrap();
        // In mm: 2 × 1 in plus a half circle of radius 0.5 in.
        let inch2 = 25.4 * 25.4;
        let want = (2.0 + std::f64::consts::PI * 0.25 / 2.0) * inch2;
        assert!((plate.area - want).abs() < 0.5, "{} vs {want}", plate.area);
        // The half circle bulges out to the right, to x = 2.5 in.
        let right = plate.points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        assert!((right - 2.5 * 25.4).abs() < 0.01, "{right}");
        // The hole: a 0.1 in circle at (0.5, 0.5) in.
        let c = hole
            .points
            .iter()
            .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
        let n = hole.points.len() as f64;
        assert!((c[0] / n - 12.7).abs() < 0.01 && (c[1] / n - 12.7).abs() < 0.01);
    }

    #[test]
    fn open_chains_stay_open() {
        let text = entities(&[
            (0, "LINE"),
            (10, "0"),
            (20, "0"),
            (11, "10"),
            (21, "0"),
            (0, "LINE"),
            (10, "10"),
            (20, "0"),
            (11, "10"),
            (21, "10"),
        ]);
        let d = parse(&text).unwrap();
        assert_eq!(d.outlines.len(), 1);
        assert!(!d.outlines[0].closed);
        assert_eq!(d.outlines[0].points.len(), 3);
        assert!(parse("hello").is_err());
    }
}
