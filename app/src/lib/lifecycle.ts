// Leaving the app: the window can't close or quit while the machine is busy or the program has
// unsaved edits without asking first. Rust holds the close and asks here (`confirm-quit`).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";

import { useProgram } from "@/lib/program";

interface QuitCheck {
  busy: string | null;
  unsaved: boolean;
}

let asking = false;

async function confirmQuit({ busy, unsaved }: QuitCheck) {
  if (asking) return;
  asking = true;
  try {
    const name = useProgram.getState().name;
    const lines = [
      busy && `The machine is still ${busy}. Quitting doesn't stop it, but nothing here will follow it.`,
      unsaved && `${name} has unsaved changes, which will be lost.`,
    ].filter(Boolean);
    const quit = await ask(lines.join("\n\n"), {
      title: "Quit Hobby Jogger?",
      kind: "warning",
      okLabel: "Quit",
      cancelLabel: "Cancel",
    });
    if (quit) await invoke("quit");
  } finally {
    asking = false;
  }
}

/** Call once at startup. */
export function startLifecycle() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  void listen<QuitCheck>("confirm-quit", (e) => void confirmQuit(e.payload));
  let unsaved = useProgram.getState().dirty;
  void invoke("set_unsaved", { unsaved });
  useProgram.subscribe((s) => {
    if (s.dirty !== unsaved) {
      unsaved = s.dirty;
      void invoke("set_unsaved", { unsaved });
    }
  });
}
