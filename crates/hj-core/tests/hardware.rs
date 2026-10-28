//! Checks against a real machine, which only touch a scratch file on its SD card and don't move
//! anything. Run with `HJ_Z1_HOST=<ip> cargo test -p hj-core --test hardware -- --ignored`.

use std::sync::Arc;
use std::time::Duration;

use hj_core::state::*;
use hj_core::{Core, Sink, StateUpdate};

struct NoSink;

impl Sink for NoSink {
    fn state(&self, _: &StateUpdate) {}
    fn telemetry(&self, _: &Telemetry) {}
    fn console(&self, _: &[ConsoleEntry]) {}
}

async fn until(core: &Core, what: &str, f: impl Fn(&AppState) -> bool) {
    for _ in 0..200 {
        if f(&core.snapshot().state) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn names(s: &AppState) -> Vec<String> {
    s.files
        .entries
        .iter()
        .flatten()
        .map(|e| e.name.clone())
        .collect()
}

#[tokio::test]
#[ignore = "needs a machine: set HJ_Z1_HOST"]
async fn upload_list_delete() {
    let host = std::env::var("HJ_Z1_HOST").expect("HJ_Z1_HOST");
    let core = Core::start(Arc::new(NoSink));
    core.request(Request::Connect {
        target: ConnectTarget::Tcp { host, port: None },
    })
    .await
    .unwrap();
    until(&core, "listing", |s| s.files.entries.is_some()).await;

    // Big enough for several 8 KiB blocks, with status polling running alongside.
    let name = "hj core test.nc";
    let text: String = (0..3000).map(|i| format!("(line {i})\n")).collect();
    core.request(Request::Upload {
        name: name.into(),
        text: text.clone(),
        run: false,
    })
    .await
    .unwrap();
    until(&core, "uploaded file listed", |s| {
        s.files
            .entries
            .iter()
            .flatten()
            .any(|e| e.name == name && e.size == text.len() as u64)
    })
    .await;

    // Download it back, and skip the download when we have the same text.
    let path = format!("/sd/gcodes/{name}");
    let data = core.download(path.clone(), None).await.unwrap().unwrap();
    assert_eq!(data, text.as_bytes());
    assert_eq!(core.download(path.clone(), Some(&text)).await, Ok(None));

    core.request(Request::DeleteFile {
        path: format!("/sd/gcodes/{name}"),
    })
    .await
    .unwrap();
    until(&core, "deleted", |s| {
        !s.files.busy && s.files.entries.is_some() && !names(s).contains(&name.to_owned())
    })
    .await;
}
