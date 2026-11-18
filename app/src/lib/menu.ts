// The menu bar is native (built in Rust, `src-tauri/src/menu.rs`); picking an item that the
// window does arrives here as a `menu` event with the item's id.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { act } from "@/lib/actions";
import { forgetRecent, newProgram, openPath, openWithDialog, saveFile, useRecent } from "@/lib/program";
import { fitBed, setMapView, useUi } from "@/lib/ui";

function onMenu(id: string) {
  if (id.startsWith("recent:")) {
    const path = useRecent.getState().paths[Number(id.slice("recent:".length))];
    if (path) void openPath(path);
    return;
  }
  switch (id) {
    case "new":
      return void newProgram();
    case "open":
      return void openWithDialog();
    case "save":
      return void saveFile();
    case "saveAs":
      return void saveFile(true);
    case "clearRecent":
      return forgetRecent();
    case "view2d":
      return setMapView("2d");
    case "view3d":
      return setMapView("3d");
    case "fit":
      setMapView("2d");
      return fitBed();
    case "home":
      return void act({ type: "home" });
    case "unlock":
      return void act({ type: "unlock" });
    case "halt":
      return void act({ type: "halt" });
    case "about":
      return useUi.setState({ aboutOpen: true });
  }
}

/** Call once at startup. */
export function startMenu() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  void listen<string>("menu", (e) => onMenu(e.payload));
  const sync = (paths: string[]) => void invoke("set_recent_files", { paths });
  sync(useRecent.getState().paths);
  useRecent.subscribe((s, prev) => s.paths !== prev.paths && sync(s.paths));
}
