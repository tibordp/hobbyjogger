//! Turning inbound frames into messages.

use crate::frame::{Frame, FrameType};
use crate::status::{Diagnose, ParseError, Status};

#[derive(Clone, PartialEq, Debug)]
pub enum Message {
    Status(Box<Status>),
    Diagnose(Diagnose),
    /// A report that looked like status or diagnose but didn't parse.
    Malformed {
        line: String,
        error: ParseError,
    },
    /// A complete line of console output (`ok`, `ALARM: ...`, command replies, ...), without the
    /// line terminator.
    Line(String),
    /// End of transmission (0x04): ends a listing such as `config-get-all -e`, without a newline.
    Eot,
    LoadInfo(Vec<u8>),
    LoadFinish,
    LoadError(Vec<u8>),
    /// File transfer and anything else we don't interpret yet.
    Other(Frame),
}

const EOT: char = '\u{4}';

/// Stateful interpreter: console text can arrive split across several frames, so lines are
/// assembled before they are classified.
#[derive(Default)]
pub struct Interpreter {
    partial: String,
}

impl Interpreter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, frame: Frame) -> Vec<Message> {
        match frame.kind {
            // Status and diagnose reports are always sent whole.
            FrameType::STATUS | FrameType::DIAGNOSE => {
                vec![classify(String::from_utf8_lossy(&frame.payload).trim())]
            }
            FrameType::NORMAL_INFO => {
                self.partial
                    .push_str(&String::from_utf8_lossy(&frame.payload));
                let mut out = vec![];
                while let Some(i) = self.partial.find(['\n', EOT]) {
                    let line: String = self.partial.drain(..=i).collect();
                    let eot = line.ends_with(EOT);
                    let line = line.trim_end_matches(['\r', '\n', EOT]);
                    if !line.is_empty() {
                        out.push(classify(line));
                    }
                    if eot {
                        out.push(Message::Eot);
                    }
                }
                out
            }
            FrameType::LOAD_INFO => vec![Message::LoadInfo(frame.payload)],
            FrameType::LOAD_FINISH => vec![Message::LoadFinish],
            FrameType::LOAD_ERROR => vec![Message::LoadError(frame.payload)],
            _ => vec![Message::Other(frame)],
        }
    }
}

fn classify(line: &str) -> Message {
    let parsed = if line.starts_with('<') {
        Status::parse(line).map(|s| Message::Status(Box::new(s)))
    } else if line.starts_with('{') {
        Diagnose::parse(line).map(Message::Diagnose)
    } else {
        return Message::Line(line.to_owned());
    };
    parsed.unwrap_or_else(|error| Message::Malformed {
        line: line.to_owned(),
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(s: &str) -> Frame {
        Frame::new(FrameType::NORMAL_INFO, s.as_bytes())
    }

    #[test]
    fn assembles_fragmented_lines() {
        let mut i = Interpreter::new();
        assert_eq!(i.feed(info("ver")), vec![]);
        assert_eq!(
            i.feed(info("sion = 1.1.3\r\nok\r\npart")),
            vec![
                Message::Line("version = 1.1.3".into()),
                Message::Line("ok".into())
            ]
        );
        assert_eq!(i.feed(info("ial\n")), vec![Message::Line("partial".into())]);
    }

    #[test]
    fn eot_ends_a_listing() {
        let mut i = Interpreter::new();
        assert_eq!(
            i.feed(info("a=1\nb=2\n\u{4}ok\n")),
            vec![
                Message::Line("a=1".into()),
                Message::Line("b=2".into()),
                Message::Eot,
                Message::Line("ok".into())
            ]
        );
    }

    #[test]
    fn classifies_status_frames() {
        let mut i = Interpreter::new();
        let msgs = i.feed(Frame::new(
            FrameType::STATUS,
            b"<Idle|MPos:0,0,0|WPos:0,0,0>\n".to_vec(),
        ));
        assert!(matches!(msgs.as_slice(), [Message::Status(_)]));

        let msgs = i.feed(Frame::new(FrameType::STATUS, b"<Idle>\n".to_vec()));
        assert!(matches!(msgs.as_slice(), [Message::Malformed { .. }]));
    }
}
