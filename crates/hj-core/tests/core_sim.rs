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
    for _ in 0..600 {
        if f(&sink.0.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let m = sink.0.lock().unwrap();
    for e in m.console.iter().rev().take(25).rev() {
        eprintln!("{:?} {}", e.kind, e.text);
    }
    eprintln!("probe: {}", m.state.as_ref().unwrap()["probe"]);
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

    // Set origin (click on the bed view): work zero at a machine point, without moving.
    core.request(Request::SetOrigin {
        x: Some(-100.0),
        y: Some(-120.0),
        z: None,
    })
    .await
    .unwrap();
    until(&sink, "origin set", |m| {
        m.state.as_ref().unwrap()["status"]["workOffset"][0] == -100.0
            && m.state.as_ref().unwrap()["status"]["workOffset"][1] == -120.0
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[0] + 1.01).abs() < 1e-6)
    })
    .await;

    // Move-to (click on the bed view): absolute machine XY, clamped inside the min limit.
    core.request(Request::MoveTo {
        x: Some(-300.0),
        y: Some(-50.0),
        z: None,
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
    // And Z alone (the Z bar), leaving X/Y.
    core.request(Request::MoveTo {
        x: None,
        y: None,
        z: Some(-20.0),
        speed: 1.0,
    })
    .await
    .unwrap();
    until(&sink, "move-to Z", |m| {
        state_of(m) == Some("idle")
            && m.telemetry.as_ref().is_some_and(|t| {
                (t.machine[2] + 20.0).abs() < 1e-6 && (t.machine[0] + 206.99).abs() < 1e-6
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

fn file_names(m: &Mirror) -> Vec<String> {
    m.state.as_ref().unwrap()["files"]["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| e["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn uploads_and_runs_jobs() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();

    // The program folder is listed on connect, folders first.
    until(&sink, "listing", |m| {
        file_names(m) == ["Examples", "sim-square.nc"] && state_of(m) == Some("idle")
    })
    .await;

    // Upload only.
    core.request(Request::Upload {
        name: "test.nc".into(),
        text: "G0 X0 Y0\n".into(),
        run: false,
    })
    .await
    .unwrap();
    until(&sink, "uploaded file listed", |m| {
        file_names(m).contains(&"test.nc".to_owned())
            && m.state.as_ref().unwrap()["transfer"].is_null()
    })
    .await;

    // Upload and run: a 20 mm square at 300 mm/min takes ~16 s.
    let program = &format!(
        "{}G21 G90\nG0 Z5\nG0 X0 Y0\nG1 Z-0.5 F300\nG1 X20\nG1 Y20\nG1 X0\nG1 Y0\nG0 Z5\n",
        "(padding so a download spans several blocks)\n".repeat(40)
    );
    core.request(Request::Upload {
        name: "square run.nc".into(),
        text: program.clone(),
        run: true,
    })
    .await
    .unwrap();
    until(&sink, "job running", |m| {
        state_of(m) == Some("run")
            && m.state.as_ref().unwrap()["job"]["path"] == "/sd/gcodes/square run.nc"
            && !m.state.as_ref().unwrap()["status"]["job"].is_null()
    })
    .await;
    // Can't start another or jog meanwhile.
    assert!(matches!(
        core.request(Request::Play {
            path: "/sd/gcodes/test.nc".into()
        })
        .await,
        Err(CoreError::Rejected(_))
    ));

    core.request(Request::Pause).await.unwrap();
    until(&sink, "paused", |m| state_of(m) == Some("pause")).await;
    core.request(Request::Resume).await.unwrap();
    until(&sink, "resumed", |m| state_of(m) == Some("run")).await;
    core.request(Request::Abort).await.unwrap();
    until(&sink, "aborted", |m| {
        state_of(m) == Some("idle")
            && m.state.as_ref().unwrap()["job"].is_null()
            && m.console
                .iter()
                .any(|e| e.kind == ConsoleKind::Sent && e.text == "G90")
    })
    .await;

    // Delete: success refreshes the listing; a missing file fails.
    core.request(Request::DeleteFile {
        path: "/sd/gcodes/test.nc".into(),
    })
    .await
    .unwrap();
    until(&sink, "deleted", |m| {
        !file_names(m).contains(&"test.nc".to_owned())
            && file_names(m).contains(&"square run.nc".to_owned())
    })
    .await;
    assert!(matches!(
        core.request(Request::DeleteFile {
            path: "/sd/gcodes/test.nc".into(),
        })
        .await,
        Err(CoreError::Failed(_))
    ));
    assert!(matches!(
        core.request(Request::DeleteFile {
            path: "/sd/../etc".into(),
        })
        .await,
        Err(CoreError::Rejected(_))
    ));

    // Download: the whole file, nothing if we already have it, an error if it's missing.
    let data = core
        .download("/sd/gcodes/square run.nc".into(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&String::from_utf8(data).unwrap(), program);
    assert_eq!(
        core.download("/sd/gcodes/square run.nc".into(), Some(program.as_str()))
            .await,
        Ok(None)
    );
    assert!(matches!(
        core.download("/sd/gcodes/nope.nc".into(), None).await,
        Err(CoreError::Failed(_))
    ));
    until(&sink, "no transfer", |m| {
        m.state.as_ref().unwrap()["transfer"].is_null()
    })
    .await;

    // Browse into a folder and run a file from there to the end.
    core.request(Request::ListFiles {
        dir: Some("/sd/gcodes/Examples".into()),
    })
    .await
    .unwrap();
    until(&sink, "subfolder", |m| file_names(m) == ["sim circle.nc"]).await;
    core.request(Request::Play {
        path: "/sd/gcodes/Examples/sim circle.nc".into(),
    })
    .await
    .unwrap();
    until(&sink, "job started", |m| {
        !m.state.as_ref().unwrap()["status"]["job"].is_null()
    })
    .await;
    until(&sink, "job finished", |m| {
        state_of(m) == Some("idle") && m.state.as_ref().unwrap()["job"].is_null()
    })
    .await;
}

#[tokio::test]
async fn manual_tool_change() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle", |m| state_of(m) == Some("idle")).await;

    core.request(Request::ChangeTool { tool: PROBE_TOOL })
        .await
        .unwrap();
    until(&sink, "waiting for the tool", |m| {
        let s = m.state.as_ref().unwrap();
        s["toolChange"]["phase"] == "waiting" && s["toolChange"]["target"] == PROBE_TOOL
    })
    .await;
    // No other motion until the change is done.
    assert!(matches!(
        core.request(Request::Jog {
            x: Some(1.0),
            y: None,
            z: None,
            a: None,
            speed: 0.5,
        })
        .await,
        Err(CoreError::Rejected(_))
    ));

    core.request(Request::ConfirmToolChange).await.unwrap();
    until(&sink, "measuring", |m| {
        m.state.as_ref().unwrap()["toolChange"]["phase"] == "measuring"
    })
    .await;
    until(&sink, "tool change done", |m| {
        let s = m.state.as_ref().unwrap();
        s["toolChange"].is_null() && s["status"]["tool"]["number"] == PROBE_TOOL
    })
    .await;
    // Confirming again is refused: nothing is waiting.
    assert!(matches!(
        core.request(Request::ConfirmToolChange).await,
        Err(CoreError::Rejected(_))
    ));
}

#[tokio::test]
async fn probes_the_simulated_part() {
    use hj_core::probe::{Across, Corner, Routine};

    use hj_sim::{BORE_CENTER, BORE_DIAMETER, BOSS_CENTER, BOSS_DIAMETER, PART_MIN, PART_TOP};

    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle", |m| state_of(m) == Some("idle")).await;

    // The probe is required.
    let corner = Request::Probe {
        routine: Routine::Corner {
            corner: Corner::FrontLeft,
            depth: 3.0,
            reach: 8.0,
        },
    };
    assert!(matches!(
        core.request(corner.clone()).await,
        Err(CoreError::Rejected(_))
    ));
    core.request(Request::Mdi {
        line: "M493.2 T9999".into(),
    })
    .await
    .unwrap();
    // Faster than real probing, to keep the test short.
    core.request(Request::SetProbeSettings {
        settings: hj_core::probe::ProbeSettings {
            fast_feed: 600.0,
            slow_feed: 300.0,
            travel_feed: 1200.0,
            ..Default::default()
        },
    })
    .await
    .unwrap();

    let goto = |x: f64, y: f64, z: f64| Request::Mdi {
        line: format!("G53 G0 X{x} Y{y} Z{z}"),
    };
    let result = |m: &Mirror| m.state.as_ref().unwrap()["probe"]["result"].clone();
    let finished = |m: &Mirror| {
        let p = &m.state.as_ref().unwrap()["probe"];
        p["running"].is_null() && (!p["result"].is_null() || !p["error"].is_null())
    };
    let close = |v: &serde_json::Value, want: f64| (v.as_f64().unwrap() - want).abs() < 0.002;

    // Corner: start above the part, 5 mm in from both edges.
    core.request(goto(PART_MIN[0] + 5.0, PART_MIN[1] + 5.0, PART_TOP + 8.0))
        .await
        .unwrap();
    until(&sink, "at the corner", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[2] - PART_TOP - 8.0).abs() < 1e-6)
    })
    .await;
    core.request(corner.clone()).await.unwrap();
    // Nothing else moves the machine meanwhile.
    assert!(matches!(
        core.request(Request::Home).await,
        Err(CoreError::Rejected(_))
    ));
    until(&sink, "corner probed", finished).await;
    let r = sink.0.lock().unwrap().state.as_ref().unwrap()["probe"].clone();
    assert!(r["error"].is_null(), "{r}");
    assert!(close(&r["result"]["x"], PART_MIN[0]), "{r}");
    assert!(close(&r["result"]["y"], PART_MIN[1]), "{r}");
    assert!(close(&r["result"]["z"], PART_TOP), "{r}");
    assert_eq!(r["contacts"].as_array().unwrap().len(), 3);

    // Make it the work origin, without moving.
    let (x, y, z) = (
        r["result"]["x"].as_f64(),
        r["result"]["y"].as_f64(),
        r["result"]["z"].as_f64(),
    );
    until(&sink, "idle after probing", |m| state_of(m) == Some("idle")).await;
    core.request(Request::SetOrigin { x, y, z }).await.unwrap();
    until(&sink, "origin at the corner", |m| {
        let o = &m.state.as_ref().unwrap()["status"]["workOffset"];
        close(&o[0], PART_MIN[0]) && close(&o[1], PART_MIN[1]) && close(&o[2], PART_TOP)
    })
    .await;
    // Done with it: the result and its contacts go off the map.
    core.request(Request::DismissProbe).await.unwrap();
    until(&sink, "probe dismissed", |m| {
        let p = &m.state.as_ref().unwrap()["probe"];
        p["result"].is_null() && p["contacts"].as_array().is_some_and(|c| c.is_empty())
    })
    .await;

    // Edge: start beside the part, outside the left edge, 3 mm below the top; it touches in.
    core.request(goto(PART_MIN[0] - 6.0, PART_MIN[1] + 20.0, PART_TOP + 5.0))
        .await
        .unwrap();
    core.request(goto(PART_MIN[0] - 6.0, PART_MIN[1] + 20.0, PART_TOP - 3.0))
        .await
        .unwrap();
    until(&sink, "beside the left edge", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[2] - PART_TOP + 3.0).abs() < 1e-6)
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Edge {
            side: hj_core::probe::Side::Left,
            search: 20.0,
        },
    })
    .await
    .unwrap();
    until(&sink, "edge probed", finished).await;
    let r = result(&sink.0.lock().unwrap());
    let e = sink.0.lock().unwrap().state.as_ref().unwrap()["probe"]["error"].clone();
    assert!(e.is_null(), "{e}");
    assert!(
        close(&r["x"], PART_MIN[0]) && r["y"].is_null() && r["z"].is_null(),
        "{r}"
    );
    until(&sink, "idle after the edge", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    core.request(goto(PART_MIN[0] - 6.0, PART_MIN[1] + 20.0, PART_TOP + 5.0))
        .await
        .unwrap();

    // Bore: start inside it, 3 mm below the top, 1.5 mm off centre.
    core.request(goto(
        BORE_CENTER[0] + 1.5,
        BORE_CENTER[1] - 1.0,
        PART_TOP + 5.0,
    ))
    .await
    .unwrap();
    core.request(goto(
        BORE_CENTER[0] + 1.5,
        BORE_CENTER[1] - 1.0,
        PART_TOP - 3.0,
    ))
    .await
    .unwrap();
    until(&sink, "in the bore", |m| {
        state_of(m) == Some("idle")
            && m.telemetry.as_ref().is_some_and(|t| {
                (t.machine[0] - BORE_CENTER[0] - 1.5).abs() < 1e-6
                    && (t.machine[2] - PART_TOP + 3.0).abs() < 1e-6
            })
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Bore {
            diameter: 15.0,
            across: Across::Xy,
        },
    })
    .await
    .unwrap();
    until(&sink, "bore probed", finished).await;
    let r = result(&sink.0.lock().unwrap());
    let e = sink.0.lock().unwrap().state.as_ref().unwrap()["probe"]["error"].clone();
    assert!(e.is_null(), "{e}");
    assert!(
        close(&r["x"], BORE_CENTER[0]) && close(&r["y"], BORE_CENTER[1]),
        "{r}"
    );
    assert!(close(&r["diameter"], BORE_DIAMETER), "{r}");

    // Across Y only, from 1.5 mm off its centreline: just Y and the width.
    until(&sink, "idle after the bore", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    core.request(goto(BORE_CENTER[0], BORE_CENTER[1] + 1.5, PART_TOP - 3.0))
        .await
        .unwrap();
    until(&sink, "off the bore's centreline", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[1] - BORE_CENTER[1] - 1.5).abs() < 1e-6)
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Bore {
            diameter: 15.0,
            across: Across::Y,
        },
    })
    .await
    .unwrap();
    until(&sink, "bore probed in Y", finished).await;
    let r = result(&sink.0.lock().unwrap());
    assert!(
        r["x"].is_null() && close(&r["y"], BORE_CENTER[1]) && r["diameter"].is_null(),
        "{r}"
    );
    assert!(close(&r["width"], BORE_DIAMETER), "{r}");

    // Boss: start above it, a bit off centre.
    core.request(goto(
        BOSS_CENTER[0] - 1.0,
        BOSS_CENTER[1] + 1.0,
        PART_TOP + 12.0,
    ))
    .await
    .unwrap();
    until(&sink, "over the boss", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[2] - PART_TOP - 12.0).abs() < 1e-6)
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Boss {
            diameter: 12.0,
            depth: 3.0,
            across: Across::Xy,
        },
    })
    .await
    .unwrap();
    until(&sink, "boss probed", finished).await;
    let r = result(&sink.0.lock().unwrap());
    assert!(
        close(&r["x"], BOSS_CENTER[0]) && close(&r["y"], BOSS_CENTER[1]),
        "{r}"
    );
    assert!(close(&r["diameter"], BOSS_DIAMETER), "{r}");
    assert!(close(&r["z"], PART_TOP + hj_sim::BOSS_HEIGHT), "{r}");

    until(&sink, "idle after the boss", |m| {
        state_of(m) == Some("idle")
    })
    .await;

    // Across X only, from 1 mm off its centreline.
    core.request(goto(BOSS_CENTER[0] + 1.0, BOSS_CENTER[1], PART_TOP + 12.0))
        .await
        .unwrap();
    until(&sink, "off the boss's centreline", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[2] - PART_TOP - 12.0).abs() < 1e-6)
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Boss {
            diameter: 12.0,
            depth: 3.0,
            across: Across::X,
        },
    })
    .await
    .unwrap();
    until(&sink, "boss probed in X", finished).await;
    let r = result(&sink.0.lock().unwrap());
    assert!(
        close(&r["x"], BOSS_CENTER[0]) && r["y"].is_null() && close(&r["width"], BOSS_DIAMETER),
        "{r}"
    );
    until(&sink, "idle after the boss in X", |m| {
        state_of(m) == Some("idle")
    })
    .await;

    // A boss given as much too small: coming down beside it lands on its top. The routine stops,
    // backs off to where that move started (above the top) and says why.
    core.request(Request::Probe {
        routine: Routine::Boss {
            diameter: 2.0,
            depth: 3.0,
            across: Across::Xy,
        },
    })
    .await
    .unwrap();
    until(&sink, "boss misjudged", finished).await;
    {
        let m = sink.0.lock().unwrap();
        let e = m.state.as_ref().unwrap()["probe"]["error"]
            .as_str()
            .unwrap();
        assert!(
            e.contains("touched something") && e.contains("backed off"),
            "{e}"
        );
    }
    until(&sink, "backed off above the boss", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| t.machine[2] > PART_TOP + hj_sim::BOSS_HEIGHT + 1.0)
    })
    .await;

    // Halted partway: Recover clears the halt, backs the probe off and clears the error.
    core.request(Request::Probe {
        routine: Routine::Surface,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    core.request(Request::Halt).await.unwrap();
    until(&sink, "halted while probing", |m| {
        state_of(m) == Some("alarm") && finished(m)
    })
    .await;
    assert_eq!(
        sink.0.lock().unwrap().state.as_ref().unwrap()["probe"]["canBackOff"],
        true
    );
    core.request(Request::RecoverProbe).await.unwrap();
    until(&sink, "recovered", |m| {
        let p = &m.state.as_ref().unwrap()["probe"];
        state_of(m) == Some("idle")
            && p["running"].is_null()
            && p["error"].is_null()
            && p["canBackOff"] == false
    })
    .await;

    // Nothing below within reach: a clear error, and the machine isn't halted.
    core.request(goto(-50.0, -50.0, -10.0)).await.unwrap();
    until(&sink, "over the bed", |m| {
        state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[0] + 50.0).abs() < 1e-6)
    })
    .await;
    core.request(Request::Probe {
        routine: Routine::Surface,
    })
    .await
    .unwrap();
    until(&sink, "surface miss", finished).await;
    let e = sink.0.lock().unwrap().state.as_ref().unwrap()["probe"]["error"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(e.starts_with("no contact"), "{e}");
    until(&sink, "idle, not halted", |m| state_of(m) == Some("idle")).await;

    // The depth probe (T0) measures Z only, switched on for the routine and off after.
    core.request(Request::Mdi {
        line: "M493.2 T0".into(),
    })
    .await
    .unwrap();
    core.request(goto(PART_MIN[0] + 10.0, PART_MIN[1] + 10.0, PART_TOP + 8.0))
        .await
        .unwrap();
    until(&sink, "over the part with the depth probe", |m| {
        state_of(m) == Some("idle")
            && m.state.as_ref().unwrap()["status"]["tool"]["number"] == 0
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[2] - PART_TOP - 8.0).abs() < 1e-6)
    })
    .await;
    assert!(matches!(
        core.request(corner).await,
        Err(CoreError::Rejected(_))
    ));
    core.request(Request::Probe {
        routine: Routine::Surface,
    })
    .await
    .unwrap();
    until(&sink, "surface with the depth probe", finished).await;
    let m = sink.0.lock().unwrap();
    let r = result(&m);
    assert!(close(&r["z"], PART_TOP) && r["x"].is_null(), "{r}");
    let sent: Vec<&str> = m.console.iter().map(|l| l.text.as_str()).collect();
    let on = sent
        .iter()
        .rposition(|l| *l == "M494.1")
        .expect("switched on");
    let off = sent
        .iter()
        .rposition(|l| *l == "M494.2")
        .expect("switched off");
    assert!(on < off, "{sent:?}");
}

#[tokio::test]
async fn jogs_while_held() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle", |m| state_of(m) == Some("idle")).await;
    let pos = |m: &Mirror| m.telemetry.as_ref().unwrap().machine;
    let hold = |x: Option<f64>, y: Option<f64>| Request::JogHold {
        x,
        y,
        z: None,
        a: None,
        speed: 0.5,
    };
    let start = pos(&sink.0.lock().unwrap());

    // Held for 1.5 s at 50 % (10 mm/s), renewed like the frontend does: it moves the whole time
    // and stops soon after release.
    for _ in 0..10 {
        core.request(hold(Some(-1.0), None)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    // Nothing else moves the machine meanwhile.
    assert!(matches!(
        core.request(Request::Home).await,
        Err(CoreError::Rejected(_))
    ));
    core.request(Request::JogRelease).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    until(&sink, "stopped after release", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    let moved = start[0] - pos(&sink.0.lock().unwrap())[0];
    assert!((12.0..20.0).contains(&moved), "moved {moved} mm");
    assert!((pos(&sink.0.lock().unwrap())[1] - start[1]).abs() < 1e-6);

    // Pressing again while it's still coasting carries on from where the queued moves end,
    // instead of being refused because the machine is running: another hold, a step, a hold.
    let before = pos(&sink.0.lock().unwrap());
    for _ in 0..4 {
        core.request(hold(Some(1.0), None)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    core.request(Request::JogRelease).await.unwrap();
    core.request(hold(None, Some(1.0))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    core.request(Request::JogRelease).await.unwrap();
    core.request(Request::Jog {
        x: None,
        y: Some(-2.0),
        z: None,
        a: None,
        speed: 0.5,
    })
    .await
    .unwrap();
    core.request(hold(Some(-1.0), None)).await.unwrap();
    core.request(Request::JogRelease).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    until(&sink, "stopped after the follow-ons", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    // Back to standstill, other requests work again.
    tokio::time::sleep(Duration::from_millis(600)).await;
    core.request(Request::SetWork {
        x: Some(0.0),
        y: None,
        z: None,
        a: None,
    })
    .await
    .unwrap();
    let after = pos(&sink.0.lock().unwrap());
    assert!(after[0] > before[0] + 1.0, "{before:?} → {after:?}");

    // Not renewed (a lost key-up): it stops on its own within about the timeout plus the lead.
    let before = pos(&sink.0.lock().unwrap());
    core.request(hold(None, Some(-1.0))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    until(&sink, "stopped without renewal", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    let moved = before[1] - pos(&sink.0.lock().unwrap())[1];
    assert!((2.0..10.0).contains(&moved), "moved {moved} mm");

    // Toward the soft limit (max -1): it stops just short of it, not in an alarm.
    for _ in 0..20 {
        core.request(hold(Some(1.0), None)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    core.request(Request::JogRelease).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    until(&sink, "stopped at the limit", |m| {
        state_of(m) == Some("idle")
    })
    .await;
    let x = pos(&sink.0.lock().unwrap())[0];
    assert!((x + 1.01).abs() < 1e-3, "x {x}");
}

#[tokio::test]
async fn overrides_outputs_laser_and_settings() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle and outputs", |m| {
        state_of(m) == Some("idle") && !m.state.as_ref().unwrap()["outputs"].is_null()
    })
    .await;
    let s = |m: &Mirror| m.state.as_ref().unwrap().clone();

    // Overrides.
    core.request(Request::SetOverride {
        feed: Some(150.0),
        spindle: Some(80.0),
        laser: None,
    })
    .await
    .unwrap();
    until(&sink, "overrides", |m| {
        let s = s(m);
        s["status"]["feedOverridePct"] == 150.0 && s["status"]["spindleOverridePct"] == 80.0
    })
    .await;
    assert!(matches!(
        core.request(Request::SetOverride {
            feed: Some(5.0),
            spindle: None,
            laser: None
        })
        .await,
        Err(CoreError::Rejected(_))
    ));

    // Outputs, as diagnose reports them.
    assert_eq!(s(&sink.0.lock().unwrap())["outputs"]["light"], true);
    core.request(Request::SetOutput {
        output: Output::Light,
        on: false,
        power: None,
    })
    .await
    .unwrap();
    core.request(Request::SetOutput {
        output: Output::SpindleFan,
        on: true,
        power: Some(60.0),
    })
    .await
    .unwrap();
    until(&sink, "outputs switched", |m| {
        let o = &s(m)["outputs"];
        o["light"] == false && o["spindleFan"]["on"] == true && o["spindleFan"]["power"] == 60.0
    })
    .await;

    // Laser: just a tool. Changing to it enters laser mode, changing away leaves it; test fire
    // needs it in.
    assert!(matches!(
        core.request(Request::LaserTest { on: true }).await,
        Err(CoreError::Rejected(_))
    ));
    core.request(Request::ChangeTool { tool: LASER_TOOL })
        .await
        .unwrap();
    until(&sink, "laser change waiting", |m| {
        let s = s(m);
        s["toolChange"]["phase"] == "waiting" && s["toolChange"]["target"] == LASER_TOOL
    })
    .await;
    core.request(Request::ConfirmToolChange).await.unwrap();
    until(&sink, "laser in", |m| {
        let s = s(m);
        s["toolChange"].is_null()
            && s["status"]["tool"]["number"] == LASER_TOOL
            && s["status"]["laser"]["mode"] == true
            && state_of(m) == Some("idle")
    })
    .await;
    core.request(Request::LaserTest { on: true }).await.unwrap();
    until(&sink, "test fire", |m| {
        s(m)["status"]["laser"]["testing"] == true
    })
    .await;
    core.request(Request::LaserTest { on: false })
        .await
        .unwrap();
    core.request(Request::SetOverride {
        feed: None,
        spindle: None,
        laser: Some(50.0),
    })
    .await
    .unwrap();
    core.request(Request::ChangeTool { tool: 2 }).await.unwrap();
    until(&sink, "change from the laser waiting", |m| {
        let s = s(m);
        s["toolChange"]["phase"] == "waiting"
            && s["toolChange"]["target"] == 2
            && s["status"]["laser"]["mode"] == false
    })
    .await;
    core.request(Request::ConfirmToolChange).await.unwrap();
    until(&sink, "T2 in", |m| {
        let s = s(m);
        s["toolChange"].is_null()
            && s["status"]["tool"]["number"] == 2
            && s["status"]["laser"]["scale"] == 50.0
            && state_of(m) == Some("idle")
    })
    .await;

    // Settings: read whole, explained where known, and kept out of the console.
    core.request(Request::LoadSettings).await.unwrap();
    until(&sink, "settings", |m| {
        let st = &s(m)["settings"];
        st["loading"] == false && st["entries"].is_array()
    })
    .await;
    let st = s(&sink.0.lock().unwrap())["settings"].clone();
    let entries = st["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 7);
    assert_eq!(entries[0]["key"], "soft_endstop.enable");
    assert_eq!(entries[0]["group"], "Travel");
    let x_min = entries
        .iter()
        .find(|e| e["key"] == "soft_endstop.x_min")
        .unwrap();
    assert_eq!(x_min["value"], "-207.0");
    assert_eq!(entries.last().unwrap()["group"], "Other: switch");
    assert!(
        !sink
            .0
            .lock()
            .unwrap()
            .console
            .iter()
            .any(|e| e.text.contains("spindle.max_rpm")),
        "settings lines leaked into the console"
    );
}

#[tokio::test]
async fn sends_console_batches_line_by_line() {
    let sink = Arc::new(TestSink::default());
    let core = Core::start(sink.clone());
    core.request(Request::Connect {
        target: ConnectTarget::Simulator,
    })
    .await
    .unwrap();
    until(&sink, "idle", |m| state_of(m) == Some("idle")).await;
    let x0 = sink.0.lock().unwrap().telemetry.as_ref().unwrap().machine[0];

    // Each line goes once the previous one is accepted; comments are skipped.
    core.request(Request::Mdi {
        line: "G91\n(two steps)\nG1 X-2 F600\n\nG1 X-3\nG90\n".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        sink.0.lock().unwrap().state.as_ref().unwrap()["batch"]["total"],
        4
    );
    // Nothing else moves the machine meanwhile.
    assert!(matches!(
        core.request(Request::Home).await,
        Err(CoreError::Rejected(_))
    ));
    until(&sink, "batch done and moved", |m| {
        m.state.as_ref().unwrap()["batch"].is_null()
            && state_of(m) == Some("idle")
            && m.telemetry
                .as_ref()
                .is_some_and(|t| (t.machine[0] - (x0 - 5.0)).abs() < 1e-3)
    })
    .await;
    let sent: Vec<String> = sink
        .0
        .lock()
        .unwrap()
        .console
        .iter()
        .filter(|e| e.kind == ConsoleKind::Sent)
        .map(|e| e.text.clone())
        .collect();
    assert!(sent.ends_with(&["G91", "G1 X-2 F600", "G1 X-3", "G90"].map(String::from)));

    // Console commands don't get an `ok`, so they can't be part of a batch.
    assert!(matches!(
        core.request(Request::Mdi {
            line: "G0 X-10\nls".into()
        })
        .await,
        Err(CoreError::Rejected(_))
    ));

    // An error stops the rest.
    core.request(Request::Mdi {
        line: "G91\nG1 Z500 F600\nG1 X-1\nG90".into(),
    })
    .await
    .unwrap();
    until(&sink, "batch stopped by the error", |m| {
        m.state.as_ref().unwrap()["batch"].is_null()
            && m.console
                .iter()
                .any(|e| e.text.contains("2 lines not sent"))
    })
    .await;
    assert!(
        !sink
            .0
            .lock()
            .unwrap()
            .console
            .iter()
            .any(|e| e.kind == ConsoleKind::Sent && e.text == "G1 X-1")
    );
}
