//! The menu bar. Items that only the window can do (open, save, switching the map) are sent to it
//! as a `menu` event carrying the item's id; quitting goes through the same check as closing.

use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

const RECENT: &str = "recent";
/// Recent file items are `recent:<index>`; the window knows the paths.
const RECENT_PREFIX: &str = "recent:";

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let item =
        |id: &str, text: &str, accel: Option<&str>| MenuItem::with_id(app, id, text, true, accel);
    let about = item("about", "About Hobby Jogger", None)?;
    let quit = item("quit", "Quit Hobby Jogger", Some("CmdOrCtrl+Q"))?;

    let recent = SubmenuBuilder::with_id(app, RECENT, "Open Recent").build()?;
    let mut file = SubmenuBuilder::new(app, "File")
        .item(&item("new", "New Program", Some("CmdOrCtrl+N"))?)
        .item(&item("open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&recent)
        .separator()
        .item(&item("save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item("saveAs", "Save As…", Some("CmdOrCtrl+Shift+S"))?);
    if !cfg!(target_os = "macos") {
        file = file.separator().item(&quit);
    }
    let file = file.build()?;

    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let mut view = SubmenuBuilder::new(app, "View")
        .item(&item("view2d", "Top-Down Map", Some("CmdOrCtrl+1"))?)
        .item(&item("view3d", "3D Map", Some("CmdOrCtrl+2"))?)
        .item(&item("fit", "Show the Whole Bed", Some("CmdOrCtrl+0"))?);
    if cfg!(target_os = "macos") {
        view = view.separator().fullscreen();
    }
    let view = view.build()?;

    let machine = SubmenuBuilder::new(app, "Machine")
        .item(&item("home", "Home", None)?)
        .item(&item("unlock", "Unlock", None)?)
        .separator()
        .item(&item("halt", "Halt", Some("CmdOrCtrl+."))?)
        .build()?;

    let menu = Menu::new(app)?;
    if cfg!(target_os = "macos") {
        let app_menu = SubmenuBuilder::new(app, "Hobby Jogger")
            .item(&about)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit)
            .build()?;
        menu.append(&app_menu)?;
    }
    menu.append_items(&[&file, &edit, &view, &machine])?;
    if cfg!(target_os = "macos") {
        let window = SubmenuBuilder::new(app, "Window")
            .minimize()
            .maximize()
            .separator()
            .close_window()
            .build()?;
        menu.append(&window)?;
    } else {
        let help = SubmenuBuilder::new(app, "Help").item(&about).build()?;
        menu.append(&help)?;
    }
    Ok(menu)
}

/// Fill Open Recent with `paths` (most recent first).
pub fn set_recent(app: &AppHandle, paths: &[String]) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let Some(recent) = find_submenu(&menu.items()?, RECENT) else {
        return Ok(());
    };
    for old in recent.items()? {
        recent.remove(&old)?;
    }
    for (i, path) in paths.iter().enumerate() {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let item = MenuItem::with_id(app, format!("{RECENT_PREFIX}{i}"), name, true, None::<&str>)?;
        recent.append(&item)?;
    }
    if paths.is_empty() {
        recent.append(&MenuItem::with_id(
            app,
            "noRecent",
            "No Recent Files",
            false,
            None::<&str>,
        )?)?;
    } else {
        recent.append(&PredefinedMenuItem::separator(app)?)?;
        recent.append(&MenuItem::with_id(
            app,
            "clearRecent",
            "Clear Menu",
            true,
            None::<&str>,
        )?)?;
    }
    Ok(())
}

fn find_submenu(items: &[MenuItemKind<Wry>], id: &str) -> Option<Submenu<Wry>> {
    items.iter().find_map(|item| match item {
        MenuItemKind::Submenu(s) if s.id() == id => Some(s.clone()),
        MenuItemKind::Submenu(s) => find_submenu(&s.items().ok()?, id),
        _ => None,
    })
}

/// A menu item was picked.
pub fn on_event(app: &AppHandle, id: &str) {
    if id == "quit" {
        crate::request_quit(app);
    } else if let Some(w) = app.get_webview_window("main") {
        let _ = w.emit("menu", id);
    }
}
