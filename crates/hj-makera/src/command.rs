//! Building outbound frames.

use crate::frame::{Frame, FrameType};

/// Single-byte realtime commands, handled immediately by the firmware rather than queued.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Realtime {
    /// Request a status report (`?`).
    Status,
    /// Feed hold (`!`). Ignored unless feed hold is enabled in the firmware.
    FeedHold,
    /// Resume from feed hold (`~`).
    Resume,
    /// Halt (Ctrl-X). Stops everything and puts the machine into the halted state.
    Halt,
}

impl Realtime {
    pub fn byte(self) -> u8 {
        match self {
            Self::Status => b'?',
            Self::FeedHold => b'!',
            Self::Resume => b'~',
            Self::Halt => 0x18,
        }
    }

    pub fn frame(self) -> Frame {
        Frame::new(FrameType::CTRL_SINGLE, [self.byte()])
    }
}

/// A console command or G-code line. Sent without a trailing newline: the firmware compares some
/// commands (`suspend`, `abort`, ...) against the exact payload.
pub fn command(line: &str) -> Frame {
    Frame::new(
        FrameType::CTRL_MULTI,
        line.trim_end_matches(['\r', '\n']).as_bytes(),
    )
}

/// Escape a file path or other argument for a console command. Spaces and the characters the
/// firmware treats as realtime commands are replaced by control codes, following the reference
/// controller (`Controller.escape`).
pub fn escape_arg(arg: &str) -> String {
    arg.chars()
        .map(|c| match c {
            ' ' => '\x01',
            '?' => '\x02',
            '&' => '\x03',
            '!' => '\x04',
            '~' => '\x05',
            c => c,
        })
        .collect()
}

/// Reverse [`escape_arg`], for names the machine sends back (directory listings).
pub fn unescape_arg(arg: &str) -> String {
    arg.chars()
        .map(|c| match c {
            '\x01' => ' ',
            '\x02' => '?',
            '\x03' => '&',
            '\x04' => '!',
            '\x05' => '~',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_strips_newline() {
        assert_eq!(command("$H\r\n").payload, b"$H");
    }

    #[test]
    fn escapes_special_characters() {
        assert_eq!(
            escape_arg("/sd/gcodes/my part?!.nc"),
            "/sd/gcodes/my\x01part\x02\x04.nc"
        );
        assert_eq!(unescape_arg(&escape_arg("my part?!.nc")), "my part?!.nc");
    }
}
