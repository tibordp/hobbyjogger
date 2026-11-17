//! Tauri adapter: forwards commands to `hj-core` and turns its published changes into events.
//! All application logic lives in `hj-core`.

use std::path::PathBuf;
use std::sync::Arc;

use hj_core::program::{self, Analysis, Program, reference, transform};
use hj_core::state::{ConsoleEntry, Request, Telemetry};
use hj_core::{Core, CoreError, Sink, Snapshot, StateUpdate, dxf, fit};
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

#[tauri::command]
async fn analyze_gcode(text: String) -> Analysis {
    tauri::async_runtime::spawn_blocking(move || program::analyze(&text))
        .await
        .unwrap_or_default()
}

/// The program rotated counter-clockwise about Z by `degrees` about `pivot` (program X/Y), or
/// every line that keeps it from being rotated (see `hj_gcode::transform`).
#[tauri::command]
fn rotate_gcode(
    text: String,
    degrees: f64,
    pivot: [f64; 2],
) -> Result<String, Vec<transform::Problem>> {
    transform::rotate(&text, degrees, pivot)
}

/// The program moved by `offset` (program X/Y/Z), or every line that keeps it from being moved.
#[tauri::command]
fn translate_gcode(text: String, offset: [f64; 3]) -> Result<String, Vec<transform::Problem>> {
    transform::translate(&text, offset)
}

/// The outlines in a DXF drawing (see `hj_core::dxf`).
#[tauri::command]
async fn import_dxf(path: PathBuf) -> Result<dxf::Drawing, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let text =
            std::fs::read(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        // Older DXFs aren't UTF-8; the geometry is ASCII either way.
        dxf::parse(&String::from_utf8_lossy(&text))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The ways `reference` (a drawing's closed outline) fits `measured` (where the ball's centre was
/// at each touch of a traced wall, machine X/Y), best first; `offset` is the ball's radius, negative
/// for a hole traced from inside (see `hj_core::fit`).
#[tauri::command]
async fn fit_outline(
    reference: Vec<[f64; 2]>,
    measured: Vec<[f64; 2]>,
    offset: f64,
) -> Vec<fit::Fit> {
    tauri::async_runtime::spawn_blocking(move || fit::fit(&reference, &measured, offset))
        .await
        .unwrap_or_default()
}

/// What each G/M code and console command does, for completion and help.
#[tauri::command]
fn gcode_reference() -> Vec<reference::CodeRef> {
    reference::codes()
}

#[tauri::command]
async fn open_program(path: PathBuf) -> Result<Program, String> {
    tauri::async_runtime::spawn_blocking(move || program::read(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn save_program(path: PathBuf, text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || program::write(&path, &text))
        .await
        .map_err(|e| e.to_string())?
}

/// Download a program from the machine's SD card. `None` if it's identical to `have`.
#[tauri::command]
async fn download_program(
    core: State<'_, Core>,
    path: String,
    have: Option<String>,
) -> Result<Option<Program>, CoreError> {
    let data = core.download(path.clone(), have.as_deref()).await?;
    Ok(data.map(|data| Program {
        name: path.rsplit('/').next().unwrap_or_default().to_owned(),
        text: String::from_utf8_lossy(&data).into_owned(),
        path,
    }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            request,
            analyze_gcode,
            rotate_gcode,
            translate_gcode,
            import_dxf,
            fit_outline,
            gcode_reference,
            open_program,
            save_program,
            download_program
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
