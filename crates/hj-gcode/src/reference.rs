//! What the Z1 does with each G and M code and console command, in its firmware's terms (Z1
//! firmware 1.1.3 source, `docs/research/`). The one source for the editor's and the console's
//! completion and help, and for this crate's "unknown code" diagnostics.

use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CodeKind {
    G,
    M,
    /// `$` commands (`$H`, `$X`, `$J`).
    System,
    /// Console commands (`ls`, `play`, `config-get`, …): not G-code, never in a program.
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Scope {
    /// In programs and from the console.
    Both,
    /// Only makes sense typed at the console (interactive, or not G-code).
    Console,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Param {
    /// A word letter (`S`) or, for console commands, a placeholder (`<path>`).
    pub name: &'static str,
    pub meaning: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodeRef {
    /// `G1`, `G38.2`, `M220`, `$H`, `ls`.
    pub code: &'static str,
    pub kind: CodeKind,
    pub scope: Scope,
    pub title: &'static str,
    #[ts(optional)]
    pub detail: Option<&'static str>,
    pub params: Vec<Param>,
}

const fn p(name: &'static str, meaning: &'static str) -> Param {
    Param { name, meaning }
}

macro_rules! codes {
    ($( $kind:ident $scope:ident $code:literal $title:literal $(, $detail:literal)? $(; $($pn:literal = $pm:literal),+)? ;)*) => {
        vec![$(CodeRef {
            code: $code,
            kind: CodeKind::$kind,
            scope: Scope::$scope,
            title: $title,
            detail: codes!(@opt $($detail)?),
            params: vec![$($(p($pn, $pm)),+)?],
        }),*]
    };
    (@opt) => { None };
    (@opt $d:literal) => { Some($d) };
}

/// Every code and command we know, G codes first, then M, `$` and console commands.
pub fn codes() -> Vec<CodeRef> {
    codes! {
        G Both "G0" "Rapid move", "Moves at full speed. Not for cutting.";
            "X" = "X position", "Y" = "Y position", "Z" = "Z position", "A" = "A angle, °";
        G Both "G1" "Linear move", "Cuts in a straight line at feed rate F.";
            "X" = "X position", "Y" = "Y position", "Z" = "Z position", "A" = "A angle, °", "F" = "Feed, mm/min";
        G Both "G2" "Clockwise arc", "Arc to X Y (Z for a helix) around a centre offset I J, or radius R.";
            "X" = "End X", "Y" = "End Y", "Z" = "End Z (helix)", "I" = "Centre X offset", "J" = "Centre Y offset", "R" = "Radius", "F" = "Feed, mm/min";
        G Both "G3" "Counter-clockwise arc", "Arc to X Y (Z for a helix) around a centre offset I J, or radius R.";
            "X" = "End X", "Y" = "End Y", "Z" = "End Z (helix)", "I" = "Centre X offset", "J" = "Centre Y offset", "R" = "Radius", "F" = "Feed, mm/min";
        G Both "G4" "Dwell", "Waits for P or S seconds.";
            "P" = "Seconds", "S" = "Seconds";
        G Both "G10" "Set work coordinates", "G10 L20 P0 X… sets the work position of the given axes to those values, here. L2 sets the offset itself.";
            "L" = "20: from the position, 2: the offset", "P" = "Coordinate system, 0 = current", "X" = "X", "Y" = "Y", "Z" = "Z", "A" = "A";
        G Both "G17" "XY plane", "Arcs in the XY plane (default).";
        G Both "G18" "ZX plane", "Arcs in the ZX plane, centre offsets K I.";
        G Both "G19" "YZ plane", "Arcs in the YZ plane, centre offsets J K.";
        G Both "G20" "Inches", "Program units in inches. Hobby Jogger works in mm.";
        G Both "G21" "Millimetres", "Program units in mm.";
        G Both "G28" "Go to stored position", "Moves to the stored reference position (G28.1 stores it).";
        G Both "G28.1" "Store position", "Stores the current position for G28.";
        G Both "G30" "Probe Z down", "Smoothie's single Z probe from here.";
        G Both "G32" "Probe a grid (auto-levelling)", "The firmware's levelling probe; M495 wraps it. Hobby Jogger doesn't use it.";
        G Both "G38.2" "Probe toward", "Probes until contact; alarm if none. On the Z1 the distance is relative, and it replies [PRB:x,y,z:flag].";
            "X" = "X distance", "Y" = "Y distance", "Z" = "Z distance", "F" = "Feed, mm/min";
        G Both "G38.3" "Probe toward, no error", "Probes until contact; no alarm if none. Relative distance on the Z1.";
            "X" = "X distance", "Y" = "Y distance", "Z" = "Z distance", "F" = "Feed, mm/min";
        G Both "G38.4" "Probe away", "Probes until contact is lost; alarm if it isn't. Relative distance.";
            "X" = "X distance", "Y" = "Y distance", "Z" = "Z distance", "F" = "Feed, mm/min";
        G Both "G38.5" "Probe away, no error", "Probes until contact is lost. Relative distance.";
            "X" = "X distance", "Y" = "Y distance", "Z" = "Z distance", "F" = "Feed, mm/min";
        G Both "G40" "Cutter compensation off";
        G Both "G43" "Tool length offset", "Not needed: the Z1 measures tool lengths at tool change.";
        G Both "G49" "Cancel tool length offset";
        G Both "G53" "Machine coordinates", "This line's move is in machine coordinates.";
        G Both "G54" "Work coordinate system 1", "The default (G55–G59 select others).";
        G Both "G55" "Work coordinate system 2";
        G Both "G56" "Work coordinate system 3";
        G Both "G57" "Work coordinate system 4";
        G Both "G58" "Work coordinate system 5";
        G Both "G59" "Work coordinate system 6";
        G Both "G61" "Exact path mode";
        G Both "G64" "Path blending mode";
        G Both "G80" "Cancel canned cycle";
        G Both "G81" "Drilling cycle", "At each X Y: rapid to R, feed to Z, retract.";
            "X" = "Hole X", "Y" = "Hole Y", "Z" = "Bottom", "R" = "Retract height", "F" = "Feed, mm/min";
        G Both "G82" "Drilling cycle with dwell", "Like G81, with a dwell of P seconds at the bottom.";
            "X" = "Hole X", "Y" = "Hole Y", "Z" = "Bottom", "R" = "Retract height", "P" = "Dwell, s", "F" = "Feed, mm/min";
        G Both "G83" "Peck drilling cycle", "Like G81, drilling in pecks of Q.";
            "X" = "Hole X", "Y" = "Hole Y", "Z" = "Bottom", "R" = "Retract height", "Q" = "Peck depth", "F" = "Feed, mm/min";
        G Both "G90" "Absolute positioning", "Coordinates are positions in the work coordinate system.";
        G Both "G91" "Incremental positioning", "Coordinates are distances from the current position.";
        G Both "G90.1" "Absolute arc centres", "I J K are absolute positions.";
        G Both "G91.1" "Incremental arc centres", "I J K are offsets from the start point (default).";
        G Both "G92" "Set position offset", "Makes the current position read as the given values, until G92.1.";
            "X" = "X", "Y" = "Y", "Z" = "Z", "A" = "A";
        G Both "G92.1" "Clear position offset";
        G Both "G92.4" "Redefine machine position (Makera)", "G92.4 A0 S0 drops whole turns from A without moving.";
            "A" = "A", "S" = "0: shrink whole turns";
        G Both "G94" "Feed per minute", "F is in mm/min (default).";
        G Both "G98" "Retract to initial height", "Canned cycles retract to where they started.";
        G Both "G99" "Retract to R", "Canned cycles retract to the R level.";

        M Both "M0" "Pause", "Waits for resume.";
        M Both "M2" "Program end";
        M Both "M3" "Spindle on", "Clockwise at S rpm; needs a tool in. In laser mode, the beam at S power (0–1) while moving.";
            "S" = "rpm, or laser power 0–1";
        M Both "M5" "Spindle off", "Also the laser in laser mode. An M5 during an M3 spin-up is ignored on the Z1.";
        M Both "M6" "Tool change", "Change to tool T. On the Z1 the machine waits for you to swap it, then measures it. T0 is the depth probe, T8888 the laser, T9999 the 3D probe.";
            "T" = "Tool number";
        M Both "M7" "Air on", "Air assist, on machines that have it.";
        M Both "M9" "Air off";
        M Both "M30" "Program end";
        M Both "M220" "Feed override", "Scales feeds, 10–1000 %; the firmware applies it to moves queued after it.";
            "S" = "Percent";
        M Both "M223" "Spindle override", "Scales the spindle speed, 50–200 %.";
            "S" = "Percent";
        M Both "M321" "Laser mode", "Changes to the laser module (T8888) and makes M3 S drive the laser.";
        M Both "M322" "Leave laser mode", "Back to CNC mode. The laser stays the tool until the next tool change.";
        M Console "M323" "Laser test fire", "Fires at the configured test power, until M324. Wear laser goggles.";
        M Console "M324" "Stop laser test fire";
        M Both "M325" "Laser power scale", "Scales the laser power, percent.";
            "S" = "Percent";
        M Both "M331" "Vacuum mode on", "Runs the vacuum while the spindle runs (machines with one).";
        M Both "M332" "Vacuum mode off";
        M Both "M370" "Clear levelling", "Clears the firmware's auto-levelling compensation.";
        M Console "M490.2" "Tool is in: measure it", "Confirms a manual tool change; the machine measures the new tool.";
        M Both "M491" "Measure the tool", "Re-measures the current tool's length on the sensor.";
        M Both "M493.2" "Set the current tool", "Tells the machine which tool is in, without moving or measuring.";
            "T" = "Tool number";
        M Both "M494.1" "Probe on", "Switches the probe input on (the firmware does it around its own probing).";
        M Both "M494.2" "Probe off";
        M Both "M495" "Firmware auto routines", "Outline scan (C D), Z probe (O F) or levelling (A B I J H) from X Y. Hobby Jogger's own routines don't use it.";
            "X" = "Start X", "Y" = "Start Y", "C" = "Outline X max", "D" = "Outline Y max", "O" = "Z probe X offset", "F" = "Z probe Y offset", "A" = "Levelling X size", "B" = "Levelling Y size", "I" = "Grid columns", "J" = "Grid rows", "H" = "Probe height";
        M Both "M801" "Power fan on", "The vacuum on machines with an ATC. Laser mode runs it at 40 %.";
            "S" = "Power, %";
        M Both "M802" "Power fan off";
        M Both "M811" "Spindle fan on";
            "S" = "Power, %";
        M Both "M812" "Spindle fan off";
        M Both "M821" "Light on";
        M Both "M822" "Light off";
        M Both "M831" "Tool sensor on";
        M Both "M832" "Tool sensor off";
        M Both "M851" "External output on", "The auxiliary output port, at S percent PWM.";
            "S" = "Power, %";
        M Both "M852" "External output off";
        M Console "M999" "Clear halt", "Same as $X.";

        System Console "$H" "Home", "Homes all axes.";
        System Console "$X" "Clear halt", "Unlocks after a halt or alarm.";
        System Console "$J" "Jog", "A relative move with no reply; F is a fraction of the max rate (0–1).";
            "X" = "X distance", "Y" = "Y distance", "Z" = "Z distance", "A" = "A angle", "F" = "Speed, 0–1 of max";
        System Console "$G" "Modal state", "Shows the active G-code modes.";
        System Console "$#" "Offsets", "Shows the work coordinate offsets.";

        Shell Console "version" "Firmware version";
        Shell Console "model" "Machine model";
        Shell Console "diagnose" "Inputs and outputs", "Switch, sensor and output states (Hobby Jogger polls it for the outputs).";
        Shell Console "ls" "List files", "-s adds sizes and dates.";
            "<dir>" = "Directory, e.g. /sd/gcodes";
        Shell Console "cat" "Show a file";
            "<path>" = "File on the SD card";
        Shell Console "rm" "Delete a file";
            "<path>" = "File on the SD card";
        Shell Console "mv" "Rename a file";
            "<from>" = "File", "<to>" = "New name";
        Shell Console "mkdir" "Make a directory";
            "<dir>" = "Directory";
        Shell Console "md5sum" "File digest";
            "<path>" = "File on the SD card";
        Shell Console "play" "Run a file";
            "<path>" = "File on the SD card";
        Shell Console "suspend" "Pause the job", "Finishes the queued moves first.";
        Shell Console "resume" "Resume the job";
        Shell Console "abort" "Stop the job", "Finishes the queued moves first.";
        Shell Console "progress" "Job progress";
        Shell Console "config-get" "Read a setting";
            "sd" = "From the SD card's config", "<key>" = "e.g. soft_endstop.x_min";
        Shell Console "config-get-all" "Read all settings", "Hobby Jogger shows them under Settings.";
        Shell Console "config-set" "Change a setting", "Writes the SD card's config; takes effect after a reset.";
            "sd" = "In the SD card's config", "<key>" = "Setting", "<value>" = "New value";
        Shell Console "time" "Set the clock", "New files get this date.";
            "<unix>" = "Seconds since 1970";
        Shell Console "net" "Network status";
        Shell Console "mem" "Memory use";
        Shell Console "ftype" "Supported upload types";
        Shell Console "reset" "Reboot the controller", "Aborts whatever is running.";
    }
}

/// Normalise a G/M word: `G01`, `g1` and `G38.20` become G1 and G38.2. Other text is returned as
/// typed.
pub fn normalise(word: &str) -> String {
    let w = word.trim();
    let (letter, rest) = w.split_at(w.chars().next().map_or(0, char::len_utf8));
    if !matches!(letter, "G" | "g" | "M" | "m") {
        return w.to_owned();
    }
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return w.to_owned();
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let mut out = format!(
        "{}{}",
        letter.to_ascii_uppercase(),
        if int.is_empty() { "0" } else { int }
    );
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// Whether a code (as `G`/`M` and its value × 10, like the analyser keeps them) is in the table.
pub(crate) fn known(letter: char, code: i32) -> bool {
    let name = if code % 10 == 0 {
        format!("{letter}{}", code / 10)
    } else {
        format!("{letter}{}.{}", code / 10, code % 10)
    };
    thread_local! {
        static NAMES: std::collections::HashSet<&'static str> =
            codes().into_iter().map(|c| c.code).collect();
    }
    NAMES.with(|n| n.contains(name.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_words() {
        assert_eq!(normalise("G01"), "G1");
        assert_eq!(normalise("g38.20"), "G38.2");
        assert_eq!(normalise("M0"), "M0");
        assert_eq!(normalise("G00"), "G0");
        assert_eq!(normalise("ls"), "ls");
        assert_eq!(normalise("Gx"), "Gx");
    }

    #[test]
    fn codes_are_unique_and_normalised() {
        let all = codes();
        let mut seen = std::collections::HashSet::new();
        for c in &all {
            assert!(seen.insert(c.code), "{} twice", c.code);
            if matches!(c.kind, CodeKind::G | CodeKind::M) {
                assert_eq!(normalise(c.code), c.code);
            }
        }
        let get = |code: &str| all.iter().find(|c| c.code == code).unwrap();
        assert_eq!(get("G1").params.len(), 5);
        assert_eq!(get("G17").detail, Some("Arcs in the XY plane (default)."));
        assert!(get("M5").params.is_empty());
        assert_eq!(get("config-set").params[1].name, "<key>");
        assert!(known('G', 382));
        assert!(known('M', 2200));
        assert!(!known('M', 1234));
    }
}
