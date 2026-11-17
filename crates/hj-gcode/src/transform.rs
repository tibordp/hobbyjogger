//! Transforming a program's text: rotating it about Z ([`rotate`]), for a part that sits on the
//! bed at an angle, and moving it ([`translate`]), for one whose origin is somewhere else (a
//! program referenced from the stock's bottom rather than its top, say).
//!
//! All or nothing: the whole program is checked first, and if any line can't be transformed
//! exactly the text is left as it was and every such line is reported. What changes on a line is
//! only the words that have to; everything else, comments and spacing included, stays as it was.
//!
//! Rotating:
//! - Moves (G0–G3, canned cycles) get both X and Y, rotated about the pivot: a line that gave
//!   only one has the other filled in from where the program was. Incremental moves (G91) rotate
//!   as vectors, as do arc centres in the default incremental mode (G91.1); absolute centres
//!   (G90.1) as points. Arcs by radius (R) need nothing.
//! - Z, machine-coordinate moves (G53) and lines that don't move in X/Y are left alone.
//! - Refused: arcs outside the XY plane (G18/G19: rotated, they'd lie in a plane G-code can't
//!   name), the A axis, work offsets set within the program (G10, G92), probing moves, and X/Y on
//!   G28/G30.
//!
//! Translating:
//! - Absolute positions move: X/Y/Z on moves, a canned cycle's R and Z levels, and arc centres in
//!   the absolute mode (G90.1). A line that doesn't give an axis doesn't move along it, so nothing
//!   needs filling in.
//! - Incremental moves and centres, machine-coordinate moves (G53) and arcs by radius stay.
//! - Refused: work offsets set within the program (G10, G92), probing moves, and G28/G30 by way of
//!   a position.
//!
//! Numbers, the pivot and offsets are in the program's own units, so inch programs work as they
//! are.

use serde::Serialize;
use ts_rs::TS;

use crate::lex::{self, Line};

/// A line that can't be transformed, and why. Lines are 1-based.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Problem {
    pub line: u32,
    pub message: String,
}

/// A word on a line, and where it is in the line's text.
struct Token {
    letter: char,
    start: usize,
    end: usize,
}

/// The words of a line with their positions; comments are skipped (and kept as text).
fn tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            ';' => break,
            '(' => match text[i..].find(')') {
                Some(end) => i += end + 1,
                None => break,
            },
            c if c.is_ascii_alphabetic() => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
                    i += 1;
                }
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                out.push(Token {
                    letter: c.to_ascii_uppercase(),
                    start,
                    end: i,
                });
            }
            _ => i += 1,
        }
    }
    out
}

/// Decimal places to write numbers with: as many as the program's own coordinates use, 3 to 6.
fn precision(text: &str) -> usize {
    let mut most = 3;
    for line in text.lines() {
        for t in tokens(line) {
            if matches!(t.letter, 'X' | 'Y' | 'Z' | 'I' | 'J' | 'K' | 'R') {
                let word = &line[t.start..t.end];
                if let Some(dot) = word.find('.') {
                    most = most.max(word.len() - dot - 1);
                }
            }
        }
    }
    most.min(6)
}

fn number(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    match s {
        "-0" | "" => "0".into(),
        s => s.into(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Motion {
    Straight,
    Arc,
    Cycle,
}

/// `text` rotated counter-clockwise by `degrees` about `pivot` (program X/Y), with a comment at
/// the top saying so; or, if any line can't be, every reason why (and nothing changed).
pub fn rotate(text: &str, degrees: f64, pivot: [f64; 2]) -> Result<String, Vec<Problem>> {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let turn = |v: [f64; 2]| [v[0] * cos - v[1] * sin, v[0] * sin + v[1] * cos];
    let about = |p: [f64; 2]| {
        let r = turn([p[0] - pivot[0], p[1] - pivot[1]]);
        [r[0] + pivot[0], r[1] + pivot[1]]
    };
    let places = precision(text);
    // Words separated by spaces, as most of the program's lines with several have them.
    let (mut spaced_lines, mut compact_lines) = (0, 0);
    for line in text.lines() {
        let toks = tokens(line);
        for w in toks.windows(2) {
            if line[w[0].end..w[1].start].contains(' ') {
                spaced_lines += 1;
            } else {
                compact_lines += 1;
            }
        }
    }
    let spaced_program = spaced_lines >= compact_lines;

    let mut problems = vec![];
    let mut out: Vec<String> = vec![];
    let mut absolute = true;
    let mut arc_absolute = false;
    let mut xy_plane = true;
    let mut motion: Option<Motion> = None;
    // Where the program is, in program X/Y, while known.
    let mut pos: [Option<f64>; 2] = [None, None];

    for (n, line) in text.lines().enumerate() {
        let line_no = n as u32 + 1;
        let mut refuse = |message: String| {
            problems.push(Problem {
                line: line_no,
                message,
            })
        };
        let words = match lex::line(line) {
            Ok(Line::Words(words)) => words,
            Ok(Line::Command) => {
                out.push(line.into());
                continue;
            }
            Err(e) => {
                refuse(format!("can't read it: {e}"));
                out.push(line.into());
                continue;
            }
        };
        let get = |l: char| words.iter().find(|w| w.letter == l).map(|w| w.value);
        let gs: Vec<i32> = words
            .iter()
            .filter(|w| w.letter == 'G')
            .map(|w| (w.value * 10.0).round() as i32)
            .collect();
        let has = |g: i32| gs.contains(&g);
        let (x, y) = (get('X'), get('Y'));

        if get('A').is_some() {
            refuse("moves the A axis; rotating about Z would change what it cuts".into());
        }
        for g in &gs {
            match g {
                100 => refuse("sets a work offset (G10) within the program".into()),
                920..=923 => refuse("sets a coordinate offset (G92) within the program".into()),
                382..=385 => refuse("a probing move".into()),
                _ => {}
            }
        }
        for g in &gs {
            match g {
                900 => absolute = true,
                910 => absolute = false,
                901 => arc_absolute = true,
                911 => arc_absolute = false,
                170 => xy_plane = true,
                180 | 190 => xy_plane = false,
                0 | 10 => motion = Some(Motion::Straight),
                20 | 30 => motion = Some(Motion::Arc),
                730 | 810 | 820 | 830 | 850 | 860 | 890 => motion = Some(Motion::Cycle),
                800 => motion = None,
                _ => {}
            }
        }

        // Moves that aren't rotated, and what they do to where the program is.
        if has(530) {
            for (i, v) in [x, y].iter().enumerate() {
                if v.is_some() {
                    pos[i] = None;
                }
            }
            out.push(line.into());
            continue;
        }
        if has(280) || has(300) {
            if x.is_some() || y.is_some() {
                refuse("moves through a stored position by way of X/Y (G28/G30)".into());
            }
            pos = [None, None];
            out.push(line.into());
            continue;
        }
        let arc = motion == Some(Motion::Arc);
        let (i, j) = (get('I'), get('J'));
        let moves_xy = x.is_some() || y.is_some() || (arc && (i.is_some() || j.is_some()));
        if !moves_xy
            || motion.is_none()
            || gs.iter().any(|g| matches!(g, 100 | 920..=923 | 382..=385))
        {
            out.push(line.into());
            continue;
        }
        if arc && !xy_plane {
            refuse("an arc outside the XY plane (G18/G19): rotated, it would lie in a plane G-code can't express".into());
            out.push(line.into());
            continue;
        }

        // The new X/Y.
        let mut new_words = vec![];
        let start = pos;
        if absolute {
            let target = [x.or(pos[0]), y.or(pos[1])];
            let [Some(tx), Some(ty)] = target else {
                let missing = if target[0].is_none() { 'X' } else { 'Y' };
                refuse(format!(
                    "moves without {missing}, before the program has said where {missing} is: rotated, \
                     the move needs both"
                ));
                out.push(line.into());
                continue;
            };
            let [rx, ry] = about([tx, ty]);
            new_words.push(format!("X{}", number(rx, places)));
            new_words.push(format!("Y{}", number(ry, places)));
            pos = [Some(tx), Some(ty)];
        } else {
            let [rx, ry] = turn([x.unwrap_or(0.0), y.unwrap_or(0.0)]);
            new_words.push(format!("X{}", number(rx, places)));
            new_words.push(format!("Y{}", number(ry, places)));
            pos = [
                pos[0].map(|p| p + x.unwrap_or(0.0)),
                pos[1].map(|p| p + y.unwrap_or(0.0)),
            ];
        }
        if arc && (i.is_some() || j.is_some()) {
            let [ri, rj] = if arc_absolute {
                let centre = [i.or(start[0]), j.or(start[1])];
                let [Some(ci), Some(cj)] = centre else {
                    refuse("an absolute arc centre (G90.1) from an unknown position".into());
                    out.push(line.into());
                    continue;
                };
                about([ci, cj])
            } else {
                turn([i.unwrap_or(0.0), j.unwrap_or(0.0)])
            };
            new_words.push(format!("I{}", number(ri, places)));
            new_words.push(format!("J{}", number(rj, places)));
        }

        // Swap them in where the old X/Y/I/J were, keeping the rest of the line.
        let rotated: &[char] = if arc {
            &['X', 'Y', 'I', 'J']
        } else {
            &['X', 'Y']
        };
        let toks = tokens(line);
        let old: Vec<&Token> = toks
            .iter()
            .filter(|t| rotated.contains(&t.letter))
            .collect();
        // As this line does, or as the program mostly does if it has only the one word.
        let spaced = if toks.len() > 1 {
            toks.windows(2)
                .any(|w| line[w[0].end..w[1].start].contains(' '))
        } else {
            spaced_program
        };
        let sep = if spaced { " " } else { "" };
        let mut text = String::new();
        let mut cursor = 0;
        for (k, t) in old.iter().enumerate() {
            if k == 0 {
                text.push_str(&line[cursor..t.start]);
                text.push_str(&new_words.join(sep));
            } else {
                // Drop the space before a word that's gone.
                text.push_str(line[cursor..t.start].trim_end_matches([' ', '\t']));
            }
            cursor = t.end;
        }
        if old.is_empty() {
            // Only from a modal arc with I/J missing can this happen; put them at the end.
            text.push_str(line);
            text.push_str(sep);
            text.push_str(&new_words.join(sep));
        } else {
            text.push_str(&line[cursor..]);
        }
        out.push(text);
    }

    if !problems.is_empty() {
        return Err(problems);
    }
    let note = format!(
        "(rotated {} degrees counter-clockwise about X{} Y{})",
        number(degrees, 3),
        number(pivot[0], places),
        number(pivot[1], places)
    );
    // After a leading `%`, which has to stay first.
    let at = usize::from(out.first().is_some_and(|l| l.trim() == "%"));
    out.insert(at, note);
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

/// The G codes on a line, ×10 (G38.2 → 382).
fn g_codes(words: &[lex::Word]) -> Vec<i32> {
    words
        .iter()
        .filter(|w| w.letter == 'G')
        .map(|w| (w.value * 10.0).round() as i32)
        .collect()
}

/// `text` moved by `offset` (program X/Y/Z), with a comment at the top saying so; or, if any line
/// can't be, every reason why (and nothing changed).
pub fn translate(text: &str, offset: [f64; 3]) -> Result<String, Vec<Problem>> {
    let places = precision(text);
    let mut problems = vec![];
    let mut out: Vec<String> = vec![];
    let mut absolute = true;
    let mut arc_absolute = false;
    let mut motion: Option<Motion> = None;

    for (n, line) in text.lines().enumerate() {
        let line_no = n as u32 + 1;
        let mut refuse = |message: String| {
            problems.push(Problem {
                line: line_no,
                message,
            })
        };
        let words = match lex::line(line) {
            Ok(Line::Words(words)) => words,
            Ok(Line::Command) => {
                out.push(line.into());
                continue;
            }
            Err(e) => {
                refuse(format!("can't read it: {e}"));
                out.push(line.into());
                continue;
            }
        };
        let gs = g_codes(&words);
        let has_axes = words.iter().any(|w| matches!(w.letter, 'X' | 'Y' | 'Z'));
        let mut refused = false;
        for g in &gs {
            let why = match g {
                100 => "sets a work offset (G10) within the program",
                920..=923 => "sets a coordinate offset (G92) within the program",
                382..=385 => "a probing move",
                280 | 300 if has_axes => {
                    "moves through a stored position by way of a position (G28/G30)"
                }
                _ => continue,
            };
            refuse(why.into());
            refused = true;
        }
        for g in &gs {
            match g {
                900 => absolute = true,
                910 => absolute = false,
                901 => arc_absolute = true,
                911 => arc_absolute = false,
                0 | 10 => motion = Some(Motion::Straight),
                20 | 30 => motion = Some(Motion::Arc),
                730 | 810 | 820 | 830 | 850 | 860 | 890 => motion = Some(Motion::Cycle),
                800 => motion = None,
                _ => {}
            }
        }
        // Only absolute positions on moves change.
        let fixed = gs.iter().any(|g| matches!(g, 530 | 280 | 300));
        if refused || fixed || !absolute || motion.is_none() {
            out.push(line.into());
            continue;
        }
        let shift = |letter: char| -> f64 {
            match (letter, motion) {
                ('X', _) => offset[0],
                ('Y', _) => offset[1],
                ('Z', _) => offset[2],
                ('I', Some(Motion::Arc)) if arc_absolute => offset[0],
                ('J', Some(Motion::Arc)) if arc_absolute => offset[1],
                ('K', Some(Motion::Arc)) if arc_absolute => offset[2],
                // A canned cycle's retract level.
                ('R', Some(Motion::Cycle)) => offset[2],
                _ => 0.0,
            }
        };
        let mut text = String::new();
        let mut cursor = 0;
        for t in tokens(line) {
            let d = shift(t.letter);
            if d == 0.0 {
                continue;
            }
            // The value: what follows the letter (and any spaces).
            let raw = &line[t.start + 1..t.end];
            let Ok(v) = raw.trim_start().parse::<f64>() else {
                continue;
            };
            text.push_str(&line[cursor..t.start + 1]);
            text.push_str(&raw[..raw.len() - raw.trim_start().len()]);
            text.push_str(&number(v + d, places));
            cursor = t.end;
        }
        text.push_str(&line[cursor..]);
        out.push(text);
    }

    if !problems.is_empty() {
        return Err(problems);
    }
    let note = format!(
        "(moved by X{} Y{} Z{})",
        number(offset[0], places),
        number(offset[1], places),
        number(offset[2], places)
    );
    let at = usize::from(out.first().is_some_and(|l| l.trim() == "%"));
    out.insert(at, note);
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;

    fn body(text: &str) -> Vec<String> {
        // Without the comment it adds at the top.
        text.lines().skip(1).map(String::from).collect()
    }

    #[test]
    fn rotates_moves_about_the_pivot() {
        let r = rotate("G0 X0 Y0\nG1 X10 Y0 F100\nG1 X10\nY5\n", 90.0, [0.0, 0.0]).unwrap();
        assert!(r.starts_with("(rotated 90 degrees counter-clockwise about X0 Y0)\n"));
        assert_eq!(
            body(&r),
            ["G0 X0 Y0", "G1 X0 Y10 F100", "G1 X0 Y10", "X-5 Y10"]
        );
        // About another point: (12, 0) is 2 to the right of (10, 0), so 2 up from it after.
        let r = rotate("G0 X12 Y0\n", 90.0, [10.0, 0.0]).unwrap();
        assert_eq!(body(&r), ["G0 X10 Y2"]);
    }

    #[test]
    fn keeps_the_rest_of_each_line() {
        let r = rotate(
            "N10 G1 X1 Y0 Z-1 F200 (cut it) ; done\nG1X1Y0\n%\n",
            90.0,
            [0.0, 0.0],
        );
        assert_eq!(
            body(&r.unwrap()),
            ["N10 G1 X0 Y1 Z-1 F200 (cut it) ; done", "G1X0Y1", "%"]
        );
        // Lines without X/Y are untouched, byte for byte.
        let r = rotate("G0 X0 Y0\nG0  Z5 \n M3 S10000\n", 30.0, [0.0, 0.0]).unwrap();
        assert_eq!(body(&r)[1..], ["G0  Z5 ", " M3 S10000"]);
    }

    #[test]
    fn rotates_incremental_moves_and_arc_centres_as_vectors() {
        let r = rotate("G91\nG1 X10 F100\nG2 X10 Y0 I5 J0\n", 90.0, [3.0, 3.0]).unwrap();
        assert_eq!(body(&r), ["G91", "G1 X0 Y10 F100", "G2 X0 Y10 I0 J5"]);
        // Absolute centres (G90.1) as points.
        let r = rotate("G90.1\nG0 X0 Y0\nG2 X10 Y0 I5 J0\n", 90.0, [0.0, 0.0]).unwrap();
        assert_eq!(body(&r)[2], "G2 X0 Y10 I0 J5");
    }

    #[test]
    fn refuses_what_it_cant_rotate_and_changes_nothing() {
        let text =
            "G0 X0 Y0\nG18\nG2 X10 Z0 I5 K0\nG1 A90\nG10 L20 P0 X0\nG92 X0\nG38.3 X5\nG28 X0 Y0\n";
        let problems = rotate(text, 10.0, [0.0, 0.0]).unwrap_err();
        let lines: Vec<u32> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [3, 4, 5, 6, 7, 8]);
        assert!(problems[0].message.contains("XY plane"));
        // A move before the program has said where Y is.
        let problems = rotate("G1 X10 F100\n", 10.0, [0.0, 0.0]).unwrap_err();
        assert_eq!(problems[0].line, 1);
        // Quarter turns of XY-plane programs, G53 and $ commands are fine.
        assert!(
            rotate(
                "$H\nG53 G0 Z-3\nG0 X0 Y0\nG17\nG2 X1 Y1 R1\n",
                10.0,
                [0.0, 0.0]
            )
            .is_ok()
        );
    }

    #[test]
    fn translates_absolute_positions_only() {
        let r = translate(
            "G0 X0 Y0 Z5\nG1 Z-1 F100\nG91 G1 X10 Z-1\nG90\nG53 G0 Z-3\nG81 X10 Y5 Z-2 R1\n",
            [1.0, 2.0, -12.0],
        )
        .unwrap();
        assert!(r.starts_with("(moved by X1 Y2 Z-12)\n"));
        assert_eq!(
            body(&r),
            [
                "G0 X1 Y2 Z-7",
                "G1 Z-13 F100",
                "G91 G1 X10 Z-1",
                "G90",
                "G53 G0 Z-3",
                "G81 X11 Y7 Z-14 R-11"
            ]
        );
        // Only Z: lines without it are untouched, byte for byte.
        let r = translate("G0 X1.25 Y3\nG1 Z0.5 F100 (down)\n", [0.0, 0.0, -10.0]).unwrap();
        assert_eq!(body(&r), ["G0 X1.25 Y3", "G1 Z-9.5 F100 (down)"]);
        // Arcs: incremental centres and radii stay; absolute centres move.
        let r = translate(
            "G0 X0 Y0\nG2 X10 Y0 I5 J0\nG3 X0 Y0 R5\nG90.1\nG2 X10 Y0 I5 J0\n",
            [1.0, 1.0, 0.0],
        )
        .unwrap();
        assert_eq!(
            body(&r)[1..],
            ["G2 X11 Y1 I5 J0", "G3 X1 Y1 R5", "G90.1", "G2 X11 Y1 I6 J1"]
        );
    }

    #[test]
    fn translation_refuses_what_it_cant_and_changes_nothing() {
        let problems = translate(
            "G0 X0 Y0\nG10 L20 P0 Z0\nG92 X0\nG38.3 Z-5\nG28 Z10\n",
            [0.0, 0.0, 1.0],
        )
        .unwrap_err();
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            [2, 3, 4, 5]
        );
        // A rotary job only moves along X/Y/Z.
        assert!(translate("G0 X0 Y0 A90\n", [0.0, 0.0, 1.0]).is_ok());
    }

    #[test]
    fn the_toolpath_moves_with_the_text() {
        let program = "G21 G90\nG0 Z5\nG0 X10 Y5\nG1 Z-1 F300\nG1 X30 Y5\nG3 X30 Y25 I0 J10\n\
                       G91\nG1 X5 Y-3\nG90\nG0 Z5\nG81 X40 Y10 Z-2 R1 F100\nX45 Y12\nG80\n";
        let d = [3.5, -2.0, -12.0];
        let (a, b) = (analyze(program), analyze(&translate(program, d).unwrap()));
        assert_eq!(a.points.len(), b.points.len());
        for (p, q) in a.points.chunks(3).zip(b.points.chunks(3)) {
            for k in 0..3 {
                assert!(
                    (q[k] as f64 - (p[k] as f64 + d[k])).abs() < 1e-3,
                    "{p:?} → {q:?}"
                );
            }
        }
    }

    #[test]
    fn the_toolpath_rotates_with_the_text() {
        // Moves, arcs both ways, a canned cycle and incremental moves: the rotated program's
        // toolpath is the original's, rotated.
        let program = "G21 G90\nG0 Z5\nG0 X10 Y5\nG1 Z-1 F300\nG1 X30 Y5\nG3 X30 Y25 I0 J10\n\
                       G2 X10 Y25 I-10 J0\nG1 Y5\nG91\nG1 X5 Y-3\nG90\nG0 Z5\nG81 X40 Y10 Z-2 R1 F100\n\
                       X45 Y12\nG80\n";
        let (deg, pivot) = (23.5f64, [7.0, -3.0]);
        let rotated = rotate(program, deg, pivot).unwrap();
        let (a, b) = (analyze(program), analyze(&rotated));
        assert!(
            a.diagnostics
                .iter()
                .all(|d| d.severity != crate::Severity::Error),
            "{:?}",
            a.diagnostics
        );
        assert_eq!(a.points.len(), b.points.len());
        let (sin, cos) = deg.to_radians().sin_cos();
        for (p, q) in a.points.chunks(3).zip(b.points.chunks(3)) {
            let (x, y) = (p[0] as f64 - pivot[0], p[1] as f64 - pivot[1]);
            let want = [
                x * cos - y * sin + pivot[0],
                x * sin + y * cos + pivot[1],
                p[2] as f64,
            ];
            for k in 0..3 {
                assert!(
                    (q[k] as f64 - want[k]).abs() < 1e-3,
                    "{p:?} → {q:?}, want {want:?}"
                );
            }
        }
    }
}
