//! End-to-end: the core driving the simulated Z1.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hj_core::state::*;
use hj_core::sync::{Patch, PatchOp, PathSegment};
use hj_core::{Core, CoreError, Sink, StateUpdate};
use serde_json::Value;

/// Mirrors what the frontend does: applies patches in order and keeps the latest telemetry.
#[derive(Default)]
struct Mirror {
    seq: u64,
    state: Option<Value>,
    telemetry: Option<Telemetry>,
    console: Vec<ConsoleEntry>,
}

#[derive(Default)]
struct TestSink(Mutex<Mirror>);

fn apply(doc: &mut Value, patches: &[Patch]) {
    for p in patches {
        let (last, parents) = p.path.split_last().unwrap();
        let mut target = &mut *doc;
        for seg in parents {
            target = match seg {
                PathSegment::Key(k) => &mut target[k.as_str()],
                PathSegment::Index(i) => &mut target[*i],
            };
        }
        match (p.op, last, target) {
            (PatchOp::Remove, PathSegment::Index(i), Value::Array(a)) => {
                a.remove(*i);
            }
            (PatchOp::Add, PathSegment::Index(i), Value::Array(a)) => {
                a.insert(*i, p.value.clone().unwrap())
            }
            (PatchOp::Remove, PathSegment::Key(k), Value::Object(m)) => {
                m.remove(k);
            }
            (_, PathSegment::Key(k), Value::Object(m)) => {
                m.insert(k.clone(), p.value.clone().unwrap());
            }
            (_, PathSegment::Index(i), Value::Array(a)) => a[*i] = p.value.clone().unwrap(),
            other => panic!("bad patch target {other:?}"),
        }
    }
}

impl Sink for TestSink {
    fn state(&self, update: &StateUpdate) {
        let mut m = self.0.lock().unwrap();
        assert_eq!(update.seq, m.seq + 1, "sequence gap");
        m.seq = update.seq;
        let doc = m
            .state
            .get_or_insert_with(|| serde_json::to_value(AppState::default()).unwrap());
        apply(doc, &update.patches);
    }
    fn telemetry(&self, t: &Telemetry) {
        self.0.lock().unwrap().telemetry = Some(t.clone());
    }
    fn console(&self, entries: &[ConsoleEntry]) {
        self.0.lock().unwrap().console.extend_from_slice(entries);
    }
}

async fn until<F: Fn(&Mirror) -> bool>(sink: &TestSink, what: &str, f: F) {
    for _ in 0..300 {
        if f(&sink.0.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn state_of(m: &Mirror) -> Option<&str> {
    m.state.as_ref()?["status"]["state"].as_str()
}

#[tokio::test]
async fn drives_simulator() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());

    // Not connected yet: requests are rejected.
    assert_eq!(
        core.request(Request::Home).await,
        Err(CoreError::NotConnected)
    );

    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle status and firmware", |m| {
        state_of(m) == Some("idle")
            && m.state.as_ref().unwrap()["machine"]["firmware"] == "1.1.3.0.1.13"
    })
    .await;

    // Jog: relative, at 50 % of the 1200 mm/min X rate.
    core.request(Request::Jog {
        x: Some(-5.0),
        y: None,
        z: None,
        a: None,
        speed: 0.5,
    })
    .await
    .unwrap();
    until(&sink, "jog to finish", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[0] + 6.0).abs() < 1e-6)
    })
    .await;

    // Jogging past the soft limit is clamped short of it rather than halting the machine.
    core.request(Request::Jog {
        x: Some(20.0),
        y: None,
        z: None,
        a: None,
        speed: 1.0,
    })
    .await
    .unwrap();
    until(&sink, "clamped jog", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[0] + 1.01).abs() < 1e-6)
    })
    .await;

    // Setting work coordinates: zero X, an arbitrary value on Y.
    core.request(Request::SetWork {
        x: Some(0.0),
        y: Some(12.5),
        z: None,
        a: None,
    })
    .await
    .unwrap();
    until(&sink, "work coordinates", |m| {
        m.telemetry
            .as_ref()
            .is_some_and(|t| t.work[0].abs() < 1e-6 && (t.work[1] - 12.5).abs() < 1e-6)
    })
    .await;

    // Travel limits were read from the machine's config after connecting.
    until(&sink, "travel limits", |m| {
        m.state.as_ref().unwrap()["machine"]["travel"]["min"]
            == serde_json::json!([-207.0, -206.0, -102.0])
    })
    .await;

    // Move-to (click on the bed view): absolute machine XY, clamped inside the min limit.
    core.request(Request::MoveTo {
        x: Some(-300.0),
        y: Some(-50.0),
        speed: 1.0,
    })
    .await
    .unwrap();
    until(&sink, "move-to", |m| {
        state_of(m) == Some("idle")
            && m.telemetry.as_ref().is_some_and(|t| {
                (t.machine[0] + 206.99).abs() < 1e-6 && (t.machine[1] + 50.0).abs() < 1e-6
            })
    })
    .await;

    // A: rotate past a full turn, then shrink back within ±360° without moving.
    core.request(Request::Jog {
        x: None,
        y: None,
        z: None,
        a: Some(370.0),
        speed: 1.0,
    })
    .await
    .unwrap();
    until(&sink, "A rotation", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.work[3] - 370.0).abs() < 1e-6)
    })
    .await;
    core.request(Request::ShrinkA).await.unwrap();
    until(&sink, "A shrink", |m| {
        m.telemetry
            .as_ref()
            .is_some_and(|t| (t.work[3] - 10.0).abs() < 1e-6)
    })
    .await;

    // Halt → alarm with a reason; homing is allowed in alarm, zeroing isn't.
    core.request(Request::Halt).await.unwrap();
    until(&sink, "alarm", |m| state_of(m) == Some("alarm")).await;
    {
        let m = sink.0.lock().unwrap();
        let halt = &m.state.as_ref().unwrap()["status"]["halt"];
        assert_eq!(halt["reason"], "Manual");
        assert_eq!(halt["recovery"], "clear");
    }
    assert!(matches!(
        core.request(Request::SetWork {
            x: None,
            y: None,
            z: Some(0.0),
            a: None,
        })
        .await,
        Err(CoreError::Rejected(_))
    ));
    core.request(Request::Unlock).await.unwrap();
    until(&sink, "idle after unlock", |m| state_of(m) == Some("idle")).await;

    // Reset reboots the controller, which also clears a halt; the link stays up.
    core.request(Request::Halt).await.unwrap();
    until(&sink, "alarm before reset", |m| {
        state_of(m) == Some("alarm")
    })
    .await;
    core.request(Request::Reset).await.unwrap();
    until(&sink, "idle after reset", |m| {
        state_of(m) == Some("idle") && m.console.iter().any(|e| e.text.contains("Rebooting"))
    })
    .await;

    // The patched mirror matches the core's own snapshot.
    let snapshot = core.snapshot();
    let m = sink.0.lock().unwrap();
    assert_eq!(m.seq, snapshot.seq);
    assert_eq!(
        m.state.as_ref().unwrap(),
        &serde_json::to_value(&snapshot.state).unwrap()
    );
    // Commands we sent and machine replies show up in the console.
    assert!(
        m.console
            .iter()
            .any(|e| e.kind == ConsoleKind::Sent && e.text.starts_with("$J X-5.0000"))
    );
    assert!(m.console.iter().any(|e| e.text == "[Caution: Unlocked]"));
}
