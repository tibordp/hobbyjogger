//! Probing routines driven from the host: each probe move is a `G38.x` line, and positions are
//! computed here from the `[PRB:…]` results. Nothing is written to the machine's work offsets.
//!
//! `G38.x` distances are always relative (`ZProbe::probe_XYZ` uses a delta move), so the routines
//! never switch to `G91`; positioning uses absolute machine-coordinate `G53 G1` moves.

use anyhow::{Result, bail};
use clap::Args;

use crate::session::Session;

/// The highest machine coordinate we position to. The soft limits sit exactly at the homed
/// position (-1.0); stay clear of it so rounding can't trip them.
const MAX_MACHINE_COORD: f64 = -1.5;

#[derive(Args, Clone)]
pub struct ProbeParams {
    /// Probe ball diameter, mm.
    #[arg(long, default_value_t = 2.0)]
    pub ball_diameter: f64,
    /// Fast probing feed, mm/min.
    #[arg(long, default_value_t = 100.0)]
    pub fast_feed: f64,
    /// Slow (measuring) probing feed, mm/min.
    #[arg(long, default_value_t = 20.0)]
    pub slow_feed: f64,
    /// Feed for positioning moves, mm/min.
    #[arg(long, default_value_t = 1000.0)]
    pub travel_feed: f64,
    /// Feed for plunging to probing depth, mm/min.
    #[arg(long, default_value_t = 300.0)]
    pub plunge_feed: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    #[allow(dead_code)] // for X routines (shoulders, corners)
    X = 0,
    Y = 1,
    Z = 2,
}

impl Axis {
    fn letter(self) -> char {
        ['X', 'Y', 'Z'][self as usize]
    }
}

async fn position(s: &mut Session) -> Result<[f64; 3]> {
    let st = s.fresh_status().await?;
    let p = st.machine_pos;
    Ok([p.x(), p.y(), p.z()])
}

/// Two-pass touch along `axis` in direction `dir` (+1/-1): fast touch, back off until the probe
/// releases, 1 mm clearance, slow touch, release, 1 mm clearance. Returns the machine coordinate
/// on `axis` at the slow contact.
async fn touch(s: &mut Session, p: &ProbeParams, axis: Axis, dir: f64, travel: f64) -> Result<f64> {
    let a = axis.letter();
    s.probe(&format!("G38.2 {a}{:.3} F{:.0}", dir * travel, p.fast_feed))
        .await?;
    s.probe(&format!(
        "G38.4 {a}{:.3} F{:.0}",
        -dir * 2.0,
        p.fast_feed / 2.0
    ))
    .await?;
    let mut back = position(s).await?;
    back[axis as usize] -= dir;
    s.move_machine(back, p.plunge_feed).await?;

    let hit = s
        .probe(&format!("G38.2 {a}{:.3} F{:.0}", dir * 2.0, p.slow_feed))
        .await?;
    let contact = [hit.x, hit.y, hit.z][axis as usize];

    s.probe(&format!("G38.4 {a}{:.3} F{:.0}", -dir * 2.0, p.slow_feed))
        .await?;
    let mut back = position(s).await?;
    back[axis as usize] -= dir;
    s.move_machine(back, p.plunge_feed).await?;
    Ok(contact)
}

#[derive(Args, Clone)]
pub struct CylinderYParams {
    #[command(flatten)]
    pub probe: ProbeParams,
    /// How far either side of the start position (in Y) to go down for the side touches, mm.
    #[arg(long, default_value_t = 35.0)]
    pub offset: f64,
    /// How far below the touched top surface to probe the sides, mm.
    #[arg(long, default_value_t = 8.0)]
    pub depth: f64,
    /// Clearance above the touched top for moves over the part, mm.
    #[arg(long, default_value_t = 5.0)]
    pub clearance: f64,
    /// Maximum travel for the first Z touch, mm.
    #[arg(long, default_value_t = 15.0)]
    pub z_travel: f64,
}

/// Find the centre in Y of a cylinder lying along X (e.g. the 4th-axis tailstock), starting with
/// the probe somewhere above it.
pub async fn cylinder_y_center(s: &mut Session, c: &CylinderYParams) -> Result<()> {
    let p = &c.probe;
    let r = p.ball_diameter / 2.0;

    let st = s.fresh_status().await?;
    if st.state != hj_makera::status::MachineState::Idle {
        bail!("machine must be idle (is {:?})", st.state);
    }
    let start = [st.machine_pos.x(), st.machine_pos.y(), st.machine_pos.z()];
    let wcs = [
        st.work_pos.x() - st.machine_pos.x(),
        st.work_pos.y() - st.machine_pos.y(),
        st.work_pos.z() - st.machine_pos.z(),
    ];
    println!(
        "start MPos {start:?}, tool {:?}",
        st.tool.map(|t| (t.number, t.offset))
    );
    for side in [1.0, -1.0] {
        if start[1] + side * c.offset > MAX_MACHINE_COORD {
            bail!(
                "Y{:.3} would be beyond the soft limit; start further from Y max",
                start[1] + side * c.offset
            );
        }
    }

    println!("== top (first pass)");
    let z_top = touch(s, p, Axis::Z, -1.0, c.z_travel).await?;
    let z_safe = (z_top + c.clearance).min(MAX_MACHINE_COORD);
    if z_safe < z_top + 2.0 {
        bail!("not enough room above the part for clearance moves");
    }
    let z_side = z_top - c.depth;
    println!("   top at MZ {z_top:.3}; safe Z {z_safe:.3}, side probing at MZ {z_side:.3}");
    s.move_machine([start[0], start[1], z_safe], p.plunge_feed)
        .await?;

    let mut sides = [0.0; 2];
    for (i, side) in [1.0, -1.0].into_iter().enumerate() {
        let y = start[1] + side * c.offset;
        println!("== {} side", if side > 0.0 { "+Y" } else { "-Y" });
        s.move_machine([start[0], y, z_safe], p.travel_feed).await?;
        s.move_machine([start[0], y, z_side], p.plunge_feed).await?;
        sides[i] = touch(s, p, Axis::Y, -side, c.offset + 5.0).await?;
        let here = position(s).await?;
        s.move_machine([start[0], here[1], z_safe], p.plunge_feed)
            .await?;
        println!("   contact at MY {:.3}", sides[i]);
    }
    let [y_plus, y_minus] = sides;
    let y_center = (y_plus + y_minus) / 2.0;
    let chord = y_plus - y_minus - p.ball_diameter;

    println!("== top at centre");
    s.move_machine([start[0], y_center, z_safe], p.travel_feed)
        .await?;
    let z_top_c = touch(s, p, Axis::Z, -1.0, c.clearance + 5.0).await?;
    s.move_machine([start[0], y_center, z_safe], p.plunge_feed)
        .await?;

    // Positions are of the ball's bottom (the tool tip). At the side contacts the ball centre is
    // at z_side + r and R + r from the axis; at the top the tip is R above the axis.
    // (R + r)² = h² + (k - R)² with h = half the ball-centre spacing, k = z_top_c - (z_side + r)
    let h = (y_plus - y_minus) / 2.0;
    let k = z_top_c - (z_side + r);
    let radius = (h * h + k * k - r * r) / (2.0 * (r + k));
    let z_axis = z_top_c - radius;

    println!();
    println!("Results (machine coordinates; work = machine + current WCS offset):");
    println!(
        "  +Y contact (ball centre)  MY {y_plus:9.4}\n  -Y contact (ball centre)  MY {y_minus:9.4}"
    );
    println!(
        "  centre Y                  MY {y_center:9.4}   WY {:9.4}",
        y_center + wcs[1]
    );
    println!("  chord at probe height     {chord:9.4} mm");
    println!(
        "  top at centre             MZ {z_top_c:9.4}   WZ {:9.4}",
        z_top_c + wcs[2]
    );
    println!("  first top (off-centre)    MZ {z_top:9.4}");
    println!("  if round: diameter        {:9.4} mm", 2.0 * radius);
    println!(
        "  if round: axis Z          MZ {z_axis:9.4}   WZ {:9.4}",
        z_axis + wcs[2]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The cylinder maths from `cylinder_y_center`, checked against a synthetic cylinder.
    #[test]
    fn recovers_cylinder_radius() {
        let (radius, axis_z, r) = (10.0_f64, -60.0_f64, 1.0_f64);
        let z_top_c = axis_z + radius;
        let z_side = z_top_c - 8.0;
        let ball_center_z = z_side + r;
        let h = ((radius + r).powi(2) - (axis_z - ball_center_z).powi(2)).sqrt();
        let k = z_top_c - (z_side + r);
        let got = (h * h + k * k - r * r) / (2.0 * (r + k));
        assert!((got - radius).abs() < 1e-9);
    }
}
