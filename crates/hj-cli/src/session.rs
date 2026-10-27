//! Request/response helpers on top of a raw connection: send a line and wait for its `ok`, wait for
//! motion to finish, run a probe move and collect its result.
//!
//! Motion completion is judged from status reports, not from `ok`: G-code lines are acknowledged
//! as soon as they are queued, and a status read right after the `ok` can still say `Idle` before
//! the move has started.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Result, bail};
use hj_makera::command::{self, Realtime};
use hj_makera::connection::{Connection, Event, Sender};
use hj_makera::message::Message;
use hj_makera::status::{MachineState, Status};
use tokio::time::Instant;

use crate::FrameLog;

const POLL: Duration = Duration::from_millis(100);
/// Positions in the status are rounded to 4 decimals.
const POSITION_TOLERANCE: f64 = 0.002;

/// A probe contact in machine coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ProbeHit {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub struct Session {
    conn: Connection,
    pub log: FrameLog,
    pub status: Option<Status>,
    lines: VecDeque<String>,
    started: Instant,
    next_poll: Instant,
}

impl Session {
    pub fn new(conn: Connection, log: FrameLog) -> Self {
        let now = Instant::now();
        Self {
            conn,
            log,
            status: None,
            lines: VecDeque::new(),
            started: now,
            next_poll: now,
        }
    }

    pub fn sender(&self) -> Sender {
        self.conn.sender.clone()
    }

    fn t(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Process events for up to `dur`, polling status every [`POLL`].
    async fn pump(&mut self, dur: Duration) -> Result<()> {
        let deadline = Instant::now() + dur;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            if now >= self.next_poll {
                let frame = Realtime::Status.frame();
                self.log.record("tx", &frame);
                self.conn.sender.send(frame).await?;
                self.next_poll = now + POLL;
            }
            let wake = deadline.min(self.next_poll);
            match tokio::time::timeout_at(wake, self.conn.events.recv()).await {
                Err(_) => {}
                Ok(None) => bail!("connection closed"),
                Ok(Some(event)) => self.handle(event)?,
            }
        }
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Frame(frame) => self.log.record("rx", &frame),
            Event::Message(Message::Status(status)) => {
                if let Some(reason) = status.halt_reason {
                    bail!("machine halted: {reason:?}");
                }
                self.status = Some(*status);
            }
            Event::Message(Message::Line(line)) => {
                println!("  [{:7.2}] < {line}", self.t());
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("alarm") || lower.starts_with("error") {
                    bail!("machine reported: {line}");
                }
                self.lines.push_back(line);
            }
            Event::Closed(err) => bail!("connection lost: {err:?}"),
            _ => {}
        }
        Ok(())
    }

    /// Send a line and wait for its `ok`. Returns the lines received before it.
    pub async fn command(&mut self, line: &str, timeout: Duration) -> Result<Vec<String>> {
        println!("  [{:7.2}] > {line}", self.t());
        self.lines.clear();
        let frame = command::command(line);
        self.log.record("tx", &frame);
        self.conn.sender.send(frame).await?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(i) = self.lines.iter().position(|l| l == "ok") {
                let before: Vec<String> = self.lines.drain(..i).collect();
                self.lines.pop_front();
                return Ok(before);
            }
            if Instant::now() >= deadline {
                bail!("no ok for {line:?} within {timeout:?}");
            }
            self.pump(POLL).await?;
        }
    }

    /// Wait until the machine is idle at `target` (machine X, Y, Z).
    pub async fn wait_at(&mut self, target: [f64; 3], timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(s) = &self.status {
                let p = &s.machine_pos;
                let there = [p.x(), p.y(), p.z()]
                    .iter()
                    .zip(target)
                    .all(|(a, b)| (a - b).abs() <= POSITION_TOLERANCE);
                if s.state == MachineState::Idle && there {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!("did not reach {target:?} within {timeout:?}");
            }
            self.pump(POLL).await?;
        }
    }

    /// Wait for a fresh status report and return it.
    pub async fn fresh_status(&mut self) -> Result<Status> {
        self.status = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.status.is_none() {
            if Instant::now() >= deadline {
                bail!("no status report");
            }
            self.pump(POLL).await?;
        }
        Ok(self.status.clone().unwrap())
    }

    /// Absolute move in machine coordinates (`G53 G1`), waiting until it completes.
    pub async fn move_machine(&mut self, target: [f64; 3], feed: f64) -> Result<()> {
        self.command("G90", Duration::from_secs(5)).await?;
        self.command(
            &format!(
                "G53 G1 X{:.4} Y{:.4} Z{:.4} F{feed:.0}",
                target[0], target[1], target[2]
            ),
            Duration::from_secs(5),
        )
        .await?;
        self.wait_at(target, Duration::from_secs(120)).await
    }

    /// Run a probe line (`G38.x`, relative distances expected under `G91`) and return the contact.
    /// Fails if the probe didn't trigger.
    pub async fn probe(&mut self, line: &str) -> Result<ProbeHit> {
        let sent = Instant::now();
        let mut seen = self.command(line, Duration::from_secs(120)).await?;
        println!(
            "  [{:7.2}]   ok after {:.2}s",
            self.t(),
            sent.elapsed().as_secs_f64()
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(hit) = seen.iter().find_map(|l| parse_prb(l)) {
                let (hit, triggered) = hit?;
                if !triggered {
                    bail!("probe did not trigger: {line}");
                }
                // The hit is reported once the probe move has stopped; wait for the machine to
                // settle so the next command starts from a known position.
                self.wait_idle_stable().await?;
                return Ok(hit);
            }
            if Instant::now() >= deadline {
                bail!("no probe result for {line:?}");
            }
            self.pump(POLL).await?;
            seen.extend(self.lines.drain(..));
        }
    }

    /// Wait until three consecutive status reports say `Idle` at the same position.
    pub async fn wait_idle_stable(&mut self) -> Result<()> {
        let mut streak = 0;
        let mut last = None;
        let deadline = Instant::now() + Duration::from_secs(60);
        while streak < 3 {
            if Instant::now() >= deadline {
                bail!("machine did not settle");
            }
            self.status = None;
            self.pump(POLL).await?;
            match &self.status {
                Some(s) if s.state == MachineState::Idle => {
                    let pos = s.machine_pos;
                    streak = if last == Some(pos) { streak + 1 } else { 1 };
                    last = Some(pos);
                }
                Some(_) => streak = 0,
                None => {}
            }
        }
        Ok(())
    }
}

/// `[PRB:x,y,z:flag]`
fn parse_prb(line: &str) -> Option<Result<(ProbeHit, bool)>> {
    let inner = line.strip_prefix("[PRB:")?.strip_suffix(']')?;
    Some((|| {
        let (coords, flag) = inner.rsplit_once(':').ok_or(anyhow::anyhow!("bad PRB"))?;
        let v: Vec<f64> = coords
            .split(',')
            .map(|s| s.trim().parse())
            .collect::<Result<_, _>>()?;
        if v.len() < 3 {
            bail!("bad PRB: {line}");
        }
        Ok((
            ProbeHit {
                x: v[0],
                y: v[1],
                z: v[2],
            },
            flag.trim() == "1",
        ))
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prb() {
        let (hit, ok) = parse_prb("[PRB:-9.400,-13.300,-82.154:1]")
            .unwrap()
            .unwrap();
        assert!(ok);
        assert_eq!(hit.z, -82.154);
        assert!(parse_prb("ok").is_none());
        assert!(!parse_prb("[PRB:0,0,0:0]").unwrap().unwrap().1);
    }
}
