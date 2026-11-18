//! What the core keeps between runs, in one JSON file: the probe settings, and the last surface
//! map and contour (in machine coordinates, so they still line up after a restart, and one left
//! unfinished can be resumed).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::contour::Contour;
use crate::map::SurfaceMap;
use crate::probe::ProbeSettings;
use crate::state::ProbeState;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Saved {
    pub probe_settings: Option<ProbeSettings>,
    pub map: Option<SurfaceMap>,
    pub contour: Option<Contour>,
}

impl Saved {
    /// What's in `path`; nothing if there's no file yet or it can't be read.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|e| {
                tracing::warn!("ignoring {}: {e}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!("couldn't read {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Into `probe`, leaving what wasn't saved as it is.
    pub fn restore(self, probe: &mut ProbeState) {
        if let Some(settings) = self.probe_settings {
            probe.settings = settings;
        }
        probe.map = self.map;
        probe.contour = self.contour;
    }

    /// Write `probe`'s to `path`, replacing the file whole (never half-written).
    pub fn write(probe: &ProbeState, path: &Path) -> std::io::Result<()> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Saving<'a> {
            probe_settings: &'a ProbeSettings,
            map: &'a Option<SurfaceMap>,
            contour: &'a Option<Contour>,
        }
        let data = serde_json::to_vec(&Saving {
            probe_settings: &probe.settings,
            map: &probe.map,
            contour: &probe.contour,
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MapSpec;

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("hj-saved-{}", std::process::id()));
        let path = dir.join("saved.json");
        assert!(Saved::load(&path).map.is_none());

        let mut probe = ProbeState::default();
        probe.settings.ball_diameter = 3.0;
        let spec: MapSpec = serde_json::from_value(serde_json::json!({
            "min": [0.0, 0.0], "max": [10.0, 10.0], "columns": 3, "rows": 3, "depth": 5.0,
            "precise": false, "levels": 0, "tolerance": 0.05
        }))
        .unwrap();
        probe.map = Some(SurfaceMap::new(spec, -5.0));
        Saved::write(&probe, &path).unwrap();

        let mut back = ProbeState::default();
        Saved::load(&path).restore(&mut back);
        assert_eq!(back.settings, probe.settings);
        assert_eq!(back.map, probe.map);
        assert_eq!(back.contour, None);

        // A file that doesn't parse is ignored.
        std::fs::write(&path, "{").unwrap();
        assert!(Saved::load(&path).probe_settings.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
