//! Tauri adapter: forwards commands to `hj-core` and turns its published changes into events.
//! All application logic lives in `hj-core`.

use std::sync::Arc;

use hj_core::state::{ConsoleEntry, Request, Telemetry};
use hj_core::{Core, CoreError, Sink, Snapshot, StateUpdate};
use tauri::{AppHandle, Emitter, Manager, State};

struct EventSink(AppHandle);

impl Sink for EventSink {
    fn state(&self, update: &StateUpdate) {
        let _ = self.0.emit("state-sync", update);
    }
    fn telemetry(&self, telemetry: &Telemetry) {
        let _ = self.0.emit("telemetry", telemetry);
    }
    fn console(&self, entries: &[ConsoleEntry]) {
        let _ = self.0.emit("console", entries);
    }
}

#[tauri::command]
fn get_state(core: State<'_, Core>) -> Snapshot {
    core.snapshot()
}

#[tauri::command]
fn console_history(core: State<'_, Core>) -> Vec<ConsoleEntry> {
    core.console_history()
}

#[tauri::command]
async fn request(core: State<'_, Core>, request: Request) -> Result<(), CoreError> {
    core.request(request).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();
    tauri::Builder::default()
        .setup(|app| {
            let sink = Arc::new(EventSink(app.handle().clone()));
            // The core spawns its tasks on Tauri's tokio runtime.
            let core = tauri::async_runtime::block_on(async { Core::start(sink) });
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            console_history,
            request
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
