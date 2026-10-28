//! The machine's configuration, `/sd/config.txt`, explained where we know what a key does.

use crate::state::Setting;

/// Keys worth explaining, in the order they're shown: (group, key, title, detail).
#[rustfmt::skip]
const KNOWN: &[(&str, &str, &str, &str)] = &[
    ("General", "wifi.machine_name", "Machine name", "How the machine announces itself on the network."),
    ("General", "switch.light.startup_state", "Light on at startup", ""),
    ("General", "light.turn_off_min", "Light off after", "Minutes idle before the light turns off; 0 keeps it on."),
    ("General", "stop_on_cover_open", "Stop when the cover opens", "Halts the machine (halt reason CoverOpen)."),
    ("General", "power.auto_sleep", "Sleep when idle", ""),
    ("General", "power.auto_sleep_min", "Sleep after", "Minutes idle before sleeping."),
    ("Travel", "soft_endstop.enable", "Soft limits", "Refuse moves outside the travel. The max side is the homed position, machine −1."),
    ("Travel", "soft_endstop.x_min", "X min", "Machine coordinates, mm."),
    ("Travel", "soft_endstop.y_min", "Y min", "Machine coordinates, mm. With the 4th axis on, the firmware uses −160 instead."),
    ("Travel", "soft_endstop.z_min", "Z min", "Machine coordinates, mm."),
    ("Travel", "coordinate.worksize_x", "Work area X", "mm."),
    ("Travel", "coordinate.worksize_y", "Work area Y", "mm."),
    ("Travel", "coordinate.clearance_x", "Clearance X", "Where the machine parks, machine coordinates."),
    ("Travel", "coordinate.clearance_y", "Clearance Y", ""),
    ("Travel", "coordinate.clearance_z", "Clearance Z", ""),
    ("Travel", "coordinate.anchor1_x", "Anchor 1 X", "Machine position of the bed's corner anchor."),
    ("Travel", "coordinate.anchor1_y", "Anchor 1 Y", ""),
    ("Travel", "coordinate.anchor2_offset_x", "Anchor 2 offset X", "From anchor 1."),
    ("Travel", "coordinate.anchor2_offset_y", "Anchor 2 offset Y", ""),
    ("Travel", "coordinate.anchor_width", "Anchor width", "mm."),
    ("Travel", "coordinate.anchor_length", "Anchor length", "mm."),
    ("Travel", "coordinate.rotation_offset_x", "4th axis offset X", "Rotation axis centre from anchor 1."),
    ("Travel", "coordinate.rotation_offset_y", "4th axis offset Y", ""),
    ("Travel", "coordinate.rotation_offset_z", "4th axis offset Z", ""),
    ("Motion", "default_feed_rate", "Default feed", "mm/min, for moves before a program sets F."),
    ("Motion", "default_seek_rate", "Default rapid", "mm/min."),
    ("Motion", "alpha_max_rate", "X max rate", "mm/min. Jogs at full speed run at this."),
    ("Motion", "beta_max_rate", "Y max rate", "mm/min."),
    ("Motion", "gamma_max_rate", "Z max rate", "mm/min."),
    ("Motion", "delta_max_rate", "A max rate", "°/min."),
    ("Motion", "acceleration", "Acceleration", "mm/s²."),
    ("Motion", "delta_acceleration", "A acceleration", "°/s²."),
    ("Spindle", "spindle.default_rpm", "Default speed", "rpm, for M3 without S."),
    ("Spindle", "spindle.max_rpm", "Max speed", "rpm."),
    ("Spindle", "temperature_control.spindle.max_temp", "Overheat limit", "°C; the machine halts above it."),
    ("Spindle", "temperatureswitch.spindle.threshold_temp", "Fan on at", "°C."),
    ("Spindle", "temperatureswitch.spindle.cooldown_power_init", "Fan start power", "%."),
    ("Spindle", "temperatureswitch.spindle.cooldown_power_step", "Fan power step", "% per °C above the threshold."),
    ("Spindle", "temperatureswitch.spindle.cooldown_delay", "Fan run-on", "Seconds after the spindle stops."),
    ("Laser", "laser_module_test_power", "Test fire power", "0–1 of full power, before the power scale."),
    ("Laser", "laser_module_maximum_power", "Max power", "0–1."),
    ("Laser", "laser_module_minimum_power", "Min power", "0–1."),
    ("Laser", "laser_module_offset_x", "Offset X", "Laser from the spindle, mm."),
    ("Laser", "laser_module_offset_y", "Offset Y", ""),
    ("Laser", "laser_module_offset_z", "Offset Z", ""),
    ("Laser", "temperatureswitch.spindle.cooldown_power_laser", "Fan power", "Spindle fan in laser mode, %."),
    ("Laser", "laser_module_clustering", "Clustering", "Groups short moves; helps fine raster engraving."),
    ("Probing", "zprobe.fast_feedrate", "Fast feed", "mm/s, the firmware's own probing."),
    ("Probing", "zprobe.slow_feedrate", "Slow feed", "mm/s."),
    ("Probing", "zprobe.return_feedrate", "Return feed", "mm/s."),
    ("Probing", "zprobe.probe_height", "Probe height", "mm above the surface to start from."),
    ("Probing", "zprobe.max_z", "Max travel", "mm."),
];

/// The settings in display order: the known ones first, as listed in [`KNOWN`], then the rest
/// grouped by their first key segment, in file order.
pub fn arrange(raw: Vec<(String, String)>) -> Vec<Setting> {
    let mut known = vec![];
    let mut rest = vec![];
    for (key, value) in raw {
        match KNOWN.iter().position(|k| k.1 == key) {
            Some(i) => {
                let (group, _, title, detail) = KNOWN[i];
                known.push((
                    i,
                    Setting {
                        key,
                        value,
                        title: Some(title.into()),
                        detail: (!detail.is_empty()).then(|| detail.into()),
                        group: group.into(),
                    },
                ));
            }
            None => {
                let group = match key.split_once('.') {
                    Some((prefix, _)) => format!("Other: {prefix}"),
                    None => "Other".into(),
                };
                rest.push(Setting {
                    key,
                    value,
                    title: None,
                    detail: None,
                    group,
                });
            }
        }
    }
    known.sort_by_key(|(i, _)| *i);
    // Stable, so file order stays within each group.
    rest.sort_by(|a, b| a.group.cmp(&b.group));
    known.into_iter().map(|(_, s)| s).chain(rest).collect()
}

/// The settings in a `config.txt`: `key value` lines, `#` comments, as the firmware reads them
/// (`SimpleShell::config_get_all_command`): the value is the first word after the key.
pub fn parse_config(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut words = line.split_whitespace();
            let key = words.next()?;
            let value = words.next()?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_first_then_grouped() {
        let raw = [
            ("zz.a", "1"),
            ("spindle.max_rpm", "13000"),
            ("aa.b", "2"),
            ("soft_endstop.enable", "true"),
            ("zz.c", "3"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let s = arrange(raw.into());
        let keys: Vec<&str> = s.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "soft_endstop.enable",
                "spindle.max_rpm",
                "aa.b",
                "zz.a",
                "zz.c"
            ]
        );
        assert_eq!(s[1].title.as_deref(), Some("Max speed"));
        assert_eq!(s[3].group, "Other: zz");
    }

    #[test]
    fn parses_config() {
        let text =
            "# Robot\nalpha_max_rate  1200.0   # mm/min\n\n  spindle.max_rpm\t13000\nlonely\n";
        assert_eq!(
            parse_config(text),
            [
                ("alpha_max_rate".to_owned(), "1200.0".to_owned()),
                ("spindle.max_rpm".to_owned(), "13000".to_owned()),
            ]
        );
    }
}
