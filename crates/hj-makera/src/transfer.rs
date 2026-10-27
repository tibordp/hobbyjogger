//! File upload to the machine's SD card over the framed protocol.
//!
//! The machine drives the transfer (`Player::upload_command` in the firmware):
//!
//! 1. host → `FILE_START` `upload <path>`, then `FILE_MD5` with the hex digest
//! 2. machine → `FILE_VIEW`; host → `FILE_VIEW` `u32 block_count, u16 block_size`
//! 3. machine → `FILE_DATA` `u32 seq` (1-based); host → `FILE_DATA` `u32 seq` + block
//! 4. machine → `FILE_END` when done, or `FILE_CANCEL` (optionally with a reason) to abort
//!
//! `FILE_MD5` from the machine asks for the digest again and `FILE_RETRY` asks for the last frame
//! again.

use std::time::Duration;

use md5::{Digest, Md5};

use crate::command::escape_arg;
use crate::connection::{Closed, Connection, Event};
use crate::frame::{Frame, FrameType};
use crate::message::Message;

/// Block size the reference controller uses over WiFi.
pub const TCP_BLOCK_SIZE: u16 = 8192;
/// Block size the reference controller uses over USB.
pub const USB_BLOCK_SIZE: u16 = 128;

/// The machine cancels after ~9 s without traffic; give up a little later.
const IDLE_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("the machine cancelled the transfer{}", .0.as_ref().map(|r| format!(": {r}")).unwrap_or_default())]
    Cancelled(Option<String>),
    #[error("the machine requested block {0}, outside 1..={1}")]
    BadSequence(u32, u32),
    #[error("file too large for {0}-byte blocks")]
    TooLarge(u16),
    #[error("no response from the machine for {0:?}")]
    Timeout(Duration),
    #[error(transparent)]
    Closed(#[from] Closed),
    #[error("connection lost")]
    Disconnected,
}

pub fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Sans-IO upload state machine.
pub struct Upload<'a> {
    path: String,
    data: &'a [u8],
    md5: String,
    block_size: u16,
    blocks: u32,
    last: Option<Frame>,
    /// Highest block sent so far.
    pub sent_blocks: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Send(Vec<Frame>),
    /// Not a transfer frame; the caller should handle it.
    Ignored,
    Done,
}

impl<'a> Upload<'a> {
    pub fn new(path: &str, data: &'a [u8], block_size: u16) -> Result<Self, TransferError> {
        let blocks = u32::try_from(data.len().div_ceil(usize::from(block_size)))
            .map_err(|_| TransferError::TooLarge(block_size))?;
        Ok(Self {
            path: path.to_owned(),
            data,
            md5: md5_hex(data),
            block_size,
            blocks,
            last: None,
            sent_blocks: 0,
        })
    }

    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    pub fn md5(&self) -> &str {
        &self.md5
    }

    pub fn start(&mut self) -> Vec<Frame> {
        let md5 = self.md5_frame();
        self.last = Some(md5.clone());
        vec![
            Frame::new(
                FrameType::FILE_START,
                format!("upload {}\n", escape_arg(&self.path)).into_bytes(),
            ),
            md5,
        ]
    }

    fn md5_frame(&self) -> Frame {
        Frame::new(FrameType::FILE_MD5, self.md5.as_bytes())
    }

    fn reply(&mut self, frame: Frame) -> Step {
        self.last = Some(frame.clone());
        Step::Send(vec![frame])
    }

    pub fn receive(&mut self, frame: &Frame) -> Result<Step, TransferError> {
        Ok(match frame.kind {
            FrameType::FILE_MD5 => self.reply(self.md5_frame()),
            FrameType::FILE_VIEW => {
                let mut payload = self.blocks.to_be_bytes().to_vec();
                payload.extend_from_slice(&self.block_size.to_be_bytes());
                self.reply(Frame::new(FrameType::FILE_VIEW, payload))
            }
            FrameType::FILE_DATA if frame.payload.len() >= 4 => {
                let p = &frame.payload;
                let seq = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                if seq == 0 || seq > self.blocks {
                    return Err(TransferError::BadSequence(seq, self.blocks));
                }
                let size = usize::from(self.block_size);
                let start = (seq as usize - 1) * size;
                let end = (start + size).min(self.data.len());
                let mut payload = seq.to_be_bytes().to_vec();
                payload.extend_from_slice(&self.data[start..end]);
                self.sent_blocks = self.sent_blocks.max(seq);
                self.reply(Frame::new(FrameType::FILE_DATA, payload))
            }
            FrameType::FILE_RETRY => match self.last.clone() {
                Some(last) => Step::Send(vec![last]),
                None => Step::Ignored,
            },
            FrameType::FILE_END => Step::Done,
            FrameType::FILE_CANCEL => {
                let reason = String::from_utf8_lossy(&frame.payload).trim().to_owned();
                return Err(TransferError::Cancelled(
                    (!reason.is_empty()).then_some(reason),
                ));
            }
            _ => Step::Ignored,
        })
    }
}

/// Run an upload over a connection. Events unrelated to the transfer are passed to `other`;
/// `progress` gets `(blocks_sent, total_blocks)`.
pub async fn upload(
    conn: &mut Connection,
    path: &str,
    data: &[u8],
    block_size: u16,
    mut progress: impl FnMut(u32, u32),
    mut other: impl FnMut(Event),
) -> Result<(), TransferError> {
    let mut up = Upload::new(path, data, block_size)?;
    for frame in up.start() {
        conn.sender.send(frame).await?;
    }
    loop {
        let event = tokio::time::timeout(IDLE_TIMEOUT, conn.events.recv())
            .await
            .map_err(|_| TransferError::Timeout(IDLE_TIMEOUT))?;
        let frame = match event {
            None | Some(Event::Closed(_)) => return Err(TransferError::Disconnected),
            Some(Event::Frame(frame)) => frame,
            // The interpreter also reports transfer frames as uninterpreted messages; those are
            // handled here via the raw frame.
            Some(Event::Message(Message::Other(f))) if is_transfer_frame(f.kind) => continue,
            Some(other_event) => {
                other(other_event);
                continue;
            }
        };
        match up.receive(&frame) {
            Ok(Step::Send(frames)) => {
                for f in frames {
                    conn.sender.send(f).await?;
                }
                progress(up.sent_blocks, up.blocks());
            }
            Ok(Step::Done) => return Ok(()),
            Ok(Step::Ignored) => other(Event::Frame(frame)),
            Err(e) => {
                if !matches!(e, TransferError::Cancelled(_)) {
                    let _ = conn
                        .sender
                        .send(Frame::new(FrameType::FILE_CANCEL, []))
                        .await;
                }
                return Err(e);
            }
        }
    }
}

fn is_transfer_frame(kind: FrameType) -> bool {
    (FrameType::FILE_START.0..=FrameType::FILE_RETRY.0).contains(&kind.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_req(seq: u32) -> Frame {
        Frame::new(FrameType::FILE_DATA, seq.to_be_bytes())
    }

    #[test]
    fn md5_known_value() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn full_handshake() {
        let data = b"G0 X0\nG0 X1\n";
        let mut up = Upload::new("/sd/gcodes/a b.nc", data, 8).unwrap();
        assert_eq!(up.blocks(), 2);

        let start = up.start();
        assert_eq!(start[0].payload, b"upload /sd/gcodes/a\x01b.nc\n");
        assert_eq!(start[1].payload, md5_hex(data).as_bytes());

        let view = up.receive(&Frame::new(FrameType::FILE_VIEW, [])).unwrap();
        assert_eq!(
            view,
            Step::Send(vec![Frame::new(FrameType::FILE_VIEW, [0, 0, 0, 2, 0, 8])])
        );

        let Step::Send(b1) = up.receive(&data_req(1)).unwrap() else {
            panic!()
        };
        assert_eq!(b1[0].payload, [&[0, 0, 0, 1][..], b"G0 X0\nG0"].concat());
        let Step::Send(b2) = up.receive(&data_req(2)).unwrap() else {
            panic!()
        };
        assert_eq!(b2[0].payload, [&[0, 0, 0, 2][..], b" X1\n"].concat());

        // Retry resends the last block.
        assert_eq!(
            up.receive(&Frame::new(FrameType::FILE_RETRY, [])).unwrap(),
            Step::Send(b2)
        );
        assert_eq!(
            up.receive(&Frame::new(FrameType::FILE_END, *b"ok\r\n"))
                .unwrap(),
            Step::Done
        );
    }

    #[test]
    fn rejects_bad_sequence_and_reports_cancel_reason() {
        let mut up = Upload::new("/sd/x.nc", b"abc", 8).unwrap();
        up.start();
        assert!(matches!(
            up.receive(&data_req(2)),
            Err(TransferError::BadSequence(2, 1))
        ));
        assert!(matches!(
            up.receive(&Frame::new(FrameType::FILE_CANCEL, *b"open failed\r\n")),
            Err(TransferError::Cancelled(Some(r))) if r == "open failed"
        ));
    }
}
