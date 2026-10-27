//! A running connection: decodes inbound frames on a background task and serializes outbound
//! frames on another.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::frame::{DecodeError, Decoder, Frame};
use crate::message::{Interpreter, Message};
use crate::transport::{BoxRead, BoxWrite, Link};

#[derive(Debug)]
pub enum Event {
    /// A raw inbound frame, emitted before the messages decoded from it.
    Frame(Frame),
    Message(Message),
    DecodeError(DecodeError),
    /// The link went down. `None` means a clean EOF.
    Closed(Option<io::Error>),
}

/// Cloneable handle for sending frames.
#[derive(Clone)]
pub struct Sender {
    tx: mpsc::Sender<Frame>,
}

impl Sender {
    /// Queue a frame for sending. Fails only if the connection is closed.
    pub async fn send(&self, frame: Frame) -> Result<(), Closed> {
        self.tx.send(frame).await.map_err(|_| Closed)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("connection closed")]
pub struct Closed;

pub struct Connection {
    pub sender: Sender,
    pub events: mpsc::Receiver<Event>,
    pub description: String,
    tasks: [AbortHandle; 2],
}

impl Connection {
    /// Start the I/O tasks on the current tokio runtime. Dropping the connection stops them and
    /// releases the link; frames still queued are discarded.
    pub fn start(link: Link) -> Self {
        let (frame_tx, frame_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(1024);
        let read = tokio::spawn(read_loop(link.reader, event_tx.clone()));
        let write = tokio::spawn(write_loop(link.writer, frame_rx, event_tx));
        Self {
            sender: Sender { tx: frame_tx },
            events: event_rx,
            description: link.description,
            tasks: [read.abort_handle(), write.abort_handle()],
        }
    }
}

impl Drop for Connection {
    /// The read task would otherwise stay parked in `read()` on a quiet link, keeping it open.
    /// For USB that leaves the interface claimed, so the next open fails with
    /// `kIOReturnExclusiveAccess` until the device is replugged.
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn read_loop(mut reader: BoxRead, events: mpsc::Sender<Event>) {
    let mut decoder = Decoder::new();
    let mut interpreter = Interpreter::new();
    let mut buf = vec![0u8; 16384];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => {
                let _ = events.send(Event::Closed(None)).await;
                return;
            }
            Ok(n) => n,
            Err(e) => {
                let _ = events.send(Event::Closed(Some(e))).await;
                return;
            }
        };
        decoder.push(&buf[..n]);
        while let Some(item) = decoder.next() {
            let batch = match item {
                Ok(frame) => {
                    let messages = interpreter.feed(frame.clone());
                    std::iter::once(Event::Frame(frame))
                        .chain(messages.into_iter().map(Event::Message))
                        .collect()
                }
                Err(e) => vec![Event::DecodeError(e)],
            };
            for event in batch {
                if events.send(event).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn write_loop(
    mut writer: BoxWrite,
    mut frames: mpsc::Receiver<Frame>,
    events: mpsc::Sender<Event>,
) {
    while let Some(frame) = frames.recv().await {
        let result = async {
            writer.write_all(&frame.encode()).await?;
            writer.flush().await
        }
        .await;
        if let Err(e) = result {
            let _ = events.send(Event::Closed(Some(e))).await;
            return;
        }
    }
    let _ = writer.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{self, Realtime};
    use crate::frame::FrameType;

    fn duplex_link() -> (Link, tokio::io::DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(ours);
        (
            Link {
                reader: Box::new(r),
                writer: Box::new(w),
                description: "test".into(),
            },
            theirs,
        )
    }

    #[tokio::test]
    async fn dropping_releases_the_link() {
        let (link, mut machine) = duplex_link();
        let conn = Connection::start(link);
        // Give the read task time to park in read() on the quiet link.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(conn);
        // Both halves of our end are dropped, so the machine side sees EOF.
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(std::time::Duration::from_secs(1), machine.read(&mut buf))
            .await
            .expect("link still held open after drop")
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn sends_and_receives_frames() {
        let (link, mut machine) = duplex_link();
        let mut conn = Connection::start(link);

        conn.sender.send(Realtime::Status.frame()).await.unwrap();
        let expected = Realtime::Status.frame().encode();
        let mut got = vec![0u8; expected.len()];
        machine.read_exact(&mut got).await.unwrap();
        assert_eq!(got, expected);

        let reply = Frame::new(
            FrameType::STATUS,
            b"<Idle|MPos:1,2,3|WPos:0,0,0>\n".to_vec(),
        );
        machine.write_all(&reply.encode()).await.unwrap();
        machine
            .write_all(&command::command("ok\n").encode()) // any frame type passes through
            .await
            .unwrap();
        drop(machine);

        let mut events = vec![];
        // The write side stays open while `conn.sender` lives, so stop at the close event rather
        // than at the end of the channel.
        while let Some(e) = conn.events.recv().await {
            let closed = matches!(e, Event::Closed(_));
            events.push(e);
            if closed {
                break;
            }
        }
        assert!(matches!(&events[0], Event::Frame(f) if *f == reply));
        assert!(
            matches!(&events[1], Event::Message(Message::Status(s)) if s.machine_pos.y() == 2.0)
        );
        assert!(matches!(&events[2], Event::Frame(f) if f.kind == FrameType::CTRL_MULTI));
        assert!(matches!(&events[3], Event::Message(Message::Other(_))));
        assert!(matches!(&events[4], Event::Closed(None)));
    }
}
