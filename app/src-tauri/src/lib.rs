//! Tauri adapter: forwards commands to `hj-core` and turns its published changes into events.
//! All application logic lives in `hj-core`.

mod menu;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hj_core::program::{self, Analysis, Program, reference, transform};
use hj_core::state::{ConsoleEntry, Request, Telemetry};
use hj_core::{Core, CoreError, Notice, Sink, Snapshot, StateUpdate, dxf, fit};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

struct EventSink {
    app: AppHandle,
    /// Held while the machine is busy, so the computer doesn't sleep in the middle of it.
    awake: Mutex<Option<keepawake::KeepAwake>>,
}

impl Sink for EventSink {
    fn state(&self, update: &StateUpdate) {
        let _ = self.app.emit("state-sync", update);
    }
    fn telemetry(&self, telemetry: &Telemetry) {
        let _ = self.app.emit("telemetry", telemetry);
    }
    fn console(&self, entries: &[ConsoleEntry]) {
        let _ = self.app.emit("console", entries);
    }
    fn busy(&self, busy: Option<&str>) {
        let mut awake = self.awake.lock().unwrap();
        if busy.is_none() {
            *awake = None;
        } else if awake.is_none() {
            // Idle sleep only: the display may still turn off, and closing the lid still sleeps.
            match keepawake::Builder::default()
                .idle(true)
                .reason("The machine is running")
                .app_name("Hobby Jogger")
                .app_reverse_domain("com.hobbyjogger.app")
                .create()
            {
                Ok(a) => *awake = Some(a),
                Err(e) => tracing::warn!("couldn't keep the computer awake: {e}"),
            }
        }
    }
    fn notice(&self, notice: &Notice) {
        // Only when looking elsewhere: the window shows it otherwise.
        let focused = self
            .app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
        if focused {
            return;
        }
        use tauri_plugin_notification::NotificationExt;
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(&notice.title)
            .body(&notice.body)
            .show()
        {
            tracing::warn!("couldn't show a notification: {e}");
        }
    }
}

/// What the window says about leaving: whether the editor has unsaved edits, and whether the user
/// has already agreed to quit.
#[derive(Default)]
struct Leaving {
    unsaved: AtomicBool,
    confirmed: AtomicBool,
}

/// Why quitting needs asking about; the window asks, then calls [`quit`].
#[derive(Clone, Serialize)]
struct QuitCheck {
    busy: Option<String>,
    unsaved: bool,
}

/// `None` if nothing would be lost by quitting now.
fn quit_check(app: &AppHandle) -> Option<QuitCheck> {
    let leaving = app.state::<Leaving>();
    if leaving.confirmed.load(Ordering::Relaxed) {
        return None;
    }
    let check = QuitCheck {
        busy: app.state::<Core>().busy(),
        unsaved: leaving.unsaved.load(Ordering::Relaxed),
    };
    (check.busy.is_some() || check.unsaved).then_some(check)
}

#[tauri::command]
fn set_unsaved(leaving: State<'_, Leaving>, unsaved: bool) {
    leaving.unsaved.store(unsaved, Ordering::Relaxed);
}

/// Quit, unless something would be lost: then the window asks first (and calls [`quit`]).
fn request_quit(app: &AppHandle) {
    match quit_check(app) {
        None => app.exit(0),
        Some(check) => {
            let _ = app.emit("confirm-quit", check);
        }
    }
}

/// Fill File ▸ Open Recent (most recent first).
#[tauri::command]
fn set_recent_files(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    menu::set_recent(&app, &paths).map_err(|e| e.to_string())
}

/// Quit, having asked (see [`QuitCheck`]).
#[tauri::command]
fn quit(app: AppHandle) {
    app.state::<Leaving>()
        .confirmed
        .store(true, Ordering::Relaxed);
    app.exit(0);
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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(Leaving::default())
        .menu(menu::build)
        .on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && let Some(check) = quit_check(window.app_handle())
            {
                api.prevent_close();
                let _ = window.emit("confirm-quit", check);
            }
        })
        .setup(|app| {
            let sink = Arc::new(EventSink {
                app: app.handle().clone(),
                awake: Mutex::new(None),
            });
            // The core spawns its tasks on Tauri's tokio runtime.
            // Probe settings, maps and contours, kept between runs.
            let saved = app
                .path()
                .app_data_dir()
                .ok()
                .map(|d| d.join("probing.json"));
            let core = tauri::async_runtime::block_on(async { Core::start_saving(sink, saved) });
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
            download_program,
            set_unsaved,
            set_recent_files,
            quit
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting from the app menu or the dock rather than closing the window.
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = event
                && let Some(check) = quit_check(app)
            {
                api.prevent_exit();
                let _ = app.emit("confirm-quit", check);
            }
        });
}
