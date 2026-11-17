//! G-code programs on disk. The text being edited lives in the frontend's editor; this reads and
//! writes files and analyses text for the preview (see [`hj_gcode`]).

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

pub use hj_gcode::{Analysis, analyze, reference, transform};

/// Larger files are refused: the editor and preview aren't built for them.
pub const MAX_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Program {
    pub path: String,
    /// File name, used as the name on the machine.
    pub name: String,
    pub text: String,
}

pub fn read(path: &Path) -> Result<Program, String> {
    let fail = |e: std::io::Error| format!("couldn't read {}: {e}", path.display());
    let size = std::fs::metadata(path).map_err(fail)?.len();
    if size > MAX_SIZE {
        return Err(format!(
            "{} is {} MB; programs up to {} MB are supported",
            path.display(),
            size / 1024 / 1024,
            MAX_SIZE / 1024 / 1024
        ));
    }
    let bytes = std::fs::read(path).map_err(fail)?;
    if bytes.contains(&0) {
        return Err(format!("{} isn't a text file", path.display()));
    }
    Ok(Program {
        path: path.display().to_string(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

pub fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("couldn't save {}: {e}", path.display()))
}
