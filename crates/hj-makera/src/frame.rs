//! Makera framed protocol, as spoken by the Z1 (and current Carvera / Carvera Air firmware).
//!
//! Wire layout, all integers big-endian:
//!
//! ```text
//! 0x86 0x68 | len: u16 | type: u8 | payload | crc: u16 | 0x55 0xAA
//! ```
//!
//! `len` counts `type + payload + crc`. The CRC is CRC-16/XMODEM (poly 0x1021, init 0) over
//! `len + type + payload`. See `SerialConsole::PacketMessage` in the Z1 firmware.

use std::fmt;

pub const HEADER: [u8; 2] = [0x86, 0x68];
pub const FOOTER: [u8; 2] = [0x55, 0xAA];

/// Bytes outside the payload: header, len, type, crc, footer.
const OVERHEAD: usize = 9;

/// Largest payload we accept from the machine. File data frames carry an 8 KiB block plus a
/// 4-byte sequence number; everything else is much smaller.
pub const MAX_PAYLOAD: usize = 8192 + 64;

/// Frame type byte. Kept open-ended (not an enum) so that unknown types pass through and can be
/// logged rather than rejected.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameType(pub u8);

impl FrameType {
    // Host -> machine
    pub const CTRL_SINGLE: Self = Self(0xA1);
    pub const CTRL_MULTI: Self = Self(0xA2);
    pub const FILE_START: Self = Self(0xB0);
    // Both directions
    pub const FILE_MD5: Self = Self(0xB1);
    pub const FILE_VIEW: Self = Self(0xB2);
    pub const FILE_DATA: Self = Self(0xB3);
    pub const FILE_END: Self = Self(0xB4);
    pub const FILE_CANCEL: Self = Self(0xB5);
    pub const FILE_RETRY: Self = Self(0xB6);
    // Machine -> host
    pub const FIRMWARE_VERSION: Self = Self(0x71);
    pub const STATUS: Self = Self(0x81);
    pub const DIAGNOSE: Self = Self(0x82);
    pub const LOAD_INFO: Self = Self(0x83);
    pub const LOAD_FINISH: Self = Self(0x84);
    pub const LOAD_ERROR: Self = Self(0x85);
    pub const NORMAL_INFO: Self = Self(0x90);

    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::CTRL_SINGLE => "CTRL_SINGLE",
            Self::CTRL_MULTI => "CTRL_MULTI",
            Self::FILE_START => "FILE_START",
            Self::FILE_MD5 => "FILE_MD5",
            Self::FILE_VIEW => "FILE_VIEW",
            Self::FILE_DATA => "FILE_DATA",
            Self::FILE_END => "FILE_END",
            Self::FILE_CANCEL => "FILE_CANCEL",
            Self::FILE_RETRY => "FILE_RETRY",
            Self::FIRMWARE_VERSION => "FIRMWARE_VERSION",
            Self::STATUS => "STATUS",
            Self::DIAGNOSE => "DIAGNOSE",
            Self::LOAD_INFO => "LOAD_INFO",
            Self::LOAD_FINISH => "LOAD_FINISH",
            Self::LOAD_ERROR => "LOAD_ERROR",
            Self::NORMAL_INFO => "NORMAL_INFO",
            _ => return None,
        })
    }
}

impl fmt::Debug for FrameType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name}"),
            None => write!(f, "0x{:02X}", self.0),
        }
    }
}

impl fmt::Display for FrameType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub kind: FrameType,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameType, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            kind,
            payload: payload.into(),
        }
    }

    /// Serialize to wire bytes.
    ///
    /// # Panics
    /// If the payload is larger than `u16::MAX - 3` bytes, which no valid frame is.
    pub fn encode(&self) -> Vec<u8> {
        let len = u16::try_from(self.payload.len() + 3).expect("frame payload too large");
        let mut out = Vec::with_capacity(self.payload.len() + OVERHEAD);
        out.extend_from_slice(&HEADER);
        out.extend_from_slice(&len.to_be_bytes());
        out.push(self.kind.0);
        out.extend_from_slice(&self.payload);
        let crc = crc16(&out[2..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&FOOTER);
        out
    }
}

/// CRC-16/XMODEM, matching the firmware's `crc16_ccitt`.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Something the decoder couldn't turn into a frame. Not fatal: the decoder resynchronizes on
/// the next header.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum DecodeError {
    /// Bytes that arrived outside any frame. On a framed connection this is noise; if the
    /// machine speaks the legacy plain-text protocol, this is where its output ends up.
    #[error("{} bytes outside a frame", .0.len())]
    Unframed(Vec<u8>),
    #[error("invalid frame length {0}")]
    BadLength(u16),
    #[error("bad CRC on {kind} frame (expected {expected:#06x}, got {actual:#06x})")]
    BadCrc {
        kind: FrameType,
        expected: u16,
        actual: u16,
    },
    #[error("missing frame footer")]
    BadFooter,
}

/// Incremental, sans-IO frame decoder. Feed it bytes with [`Decoder::push`], then drain
/// [`Decoder::next`] until it returns `None`.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes buffered but not yet decoded.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Result<Frame, DecodeError>> {
        // Skip to the next header, reporting anything we drop.
        let start = match find_header(&self.buf) {
            Some(i) => i,
            None => {
                // Keep a trailing 0x86, which may be the first half of a header.
                let keep = usize::from(self.buf.last() == Some(&HEADER[0]));
                let drop = self.buf.len() - keep;
                if drop == 0 {
                    return None;
                }
                return Some(Err(DecodeError::Unframed(self.buf.drain(..drop).collect())));
            }
        };
        if start > 0 {
            return Some(Err(DecodeError::Unframed(
                self.buf.drain(..start).collect(),
            )));
        }

        if self.buf.len() < 5 {
            return None;
        }
        let len = u16::from_be_bytes([self.buf[2], self.buf[3]]);
        if !(3..=(MAX_PAYLOAD + 3) as u16).contains(&len) {
            self.skip_header();
            return Some(Err(DecodeError::BadLength(len)));
        }
        let total = usize::from(len) + 6;
        if self.buf.len() < total {
            return None;
        }

        let frame = &self.buf[..total];
        if frame[total - 2..] != FOOTER {
            self.skip_header();
            return Some(Err(DecodeError::BadFooter));
        }
        let kind = FrameType(frame[4]);
        let expected = crc16(&frame[2..total - 4]);
        let actual = u16::from_be_bytes([frame[total - 4], frame[total - 3]]);
        if expected != actual {
            self.skip_header();
            return Some(Err(DecodeError::BadCrc {
                kind,
                expected,
                actual,
            }));
        }

        let payload = frame[5..total - 4].to_vec();
        self.buf.drain(..total);
        Some(Ok(Frame { kind, payload }))
    }

    /// Drop the header of a corrupt frame so that the search for the next one starts after it.
    fn skip_header(&mut self) {
        self.buf.drain(..2);
    }
}

fn find_header(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == HEADER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(dec: &mut Decoder) -> Vec<Result<Frame, DecodeError>> {
        std::iter::from_fn(|| dec.next()).collect()
    }

    #[test]
    fn crc_matches_xmodem_check_value() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }

    #[test]
    fn encodes_status_query_like_reference_controller() {
        // len = 4 (type + 1 byte + crc), CRC over [00 04 A1 3F].
        let bytes = Frame::new(FrameType::CTRL_SINGLE, *b"?").encode();
        let crc = crc16(&[0x00, 0x04, 0xA1, b'?']).to_be_bytes();
        assert_eq!(
            bytes,
            [
                0x86, 0x68, 0x00, 0x04, 0xA1, b'?', crc[0], crc[1], 0x55, 0xAA
            ]
        );
    }

    #[test]
    fn roundtrip_split_across_pushes() {
        let a = Frame::new(FrameType::STATUS, b"<Idle|MPos:0,0,0>".to_vec());
        let b = Frame::new(FrameType::NORMAL_INFO, b"ok\r\n".to_vec());
        let mut wire = a.encode();
        wire.extend(b.encode());

        let mut dec = Decoder::new();
        let mut out = vec![];
        for chunk in wire.chunks(3) {
            dec.push(chunk);
            out.extend(decode_all(&mut dec));
        }
        assert_eq!(out, vec![Ok(a), Ok(b)]);
        assert_eq!(dec.pending(), 0);
    }

    #[test]
    fn reports_garbage_and_resyncs() {
        let f = Frame::new(FrameType::NORMAL_INFO, b"hello\n".to_vec());
        let mut wire = b"ok\n".to_vec();
        wire.extend(f.encode());

        let mut dec = Decoder::new();
        dec.push(&wire);
        assert_eq!(
            decode_all(&mut dec),
            vec![Err(DecodeError::Unframed(b"ok\n".to_vec())), Ok(f)]
        );
    }

    #[test]
    fn keeps_partial_header() {
        let mut dec = Decoder::new();
        dec.push(&[b'x', 0x86]);
        assert_eq!(
            decode_all(&mut dec),
            vec![Err(DecodeError::Unframed(b"x".to_vec()))]
        );
        assert_eq!(dec.pending(), 1);
    }

    #[test]
    fn bad_crc_skips_frame() {
        let good = Frame::new(FrameType::NORMAL_INFO, b"b".to_vec());
        let mut bad = Frame::new(FrameType::NORMAL_INFO, b"a".to_vec()).encode();
        bad[5] ^= 0xFF;
        let mut wire = bad;
        wire.extend(good.encode());

        let mut dec = Decoder::new();
        dec.push(&wire);
        let out = decode_all(&mut dec);
        assert!(matches!(out[0], Err(DecodeError::BadCrc { .. })));
        assert_eq!(out.last(), Some(&Ok(good)));
    }
}
