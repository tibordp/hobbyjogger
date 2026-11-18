import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FileDown, OctagonX, Power } from "lucide-react";
import { useEffect, useState } from "react";
import { useDefaultLayout } from "react-resizable-panels";
import { toast } from "sonner";

import { AboutDialog } from "@/components/AboutDialog";
import { BedView } from "@/components/BedView";
import { ConnectionBar } from "@/components/ConnectionBar";
import { ConsolePanel } from "@/components/ConsolePanel";
import { Dro } from "@/components/Dro";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { JobPanel } from "@/components/JobPanel";
import { JogPanel } from "@/components/JogPanel";
import { LaserPanel } from "@/components/LaserPanel";
import { MachineControls } from "@/components/MachineControls";
import { MarkersPanel } from "@/components/MarkersPanel";
import { ProbePanel } from "@/components/ProbePanel";
import { ProgramEditor } from "@/components/ProgramEditor";
import { Panel, ScrollColumn } from "@/components/ScrollColumn";
import { SettingsDialog } from "@/components/SettingsDialog";
import { TitleTooltips } from "@/components/TitleTooltips";
import { ToolChangeDialog } from "@/components/ToolChangeDialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { act } from "@/lib/actions";
import { useConnected } from "@/lib/machine";
import { isGcodePath, openPath } from "@/lib/program";
import { useStore } from "@/lib/store";

/** Opens G-code files dropped onto the window; returns the name being dragged in, if any. */
function useFileDrop() {
  const [dragging, setDragging] = useState<string | null>(null);
  useEffect(() => {
    // Only inside Tauri (not when the frontend is opened in a plain browser).
    if (!("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview()
      .onDragDropEvent(({ payload: p }) => {
        if (p.type === "enter") {
          const path = p.paths.find(isGcodePath);
          setDragging(path ? (path.split(/[\\/]/).pop() ?? path) : "");
        } else if (p.type === "leave") {
          setDragging(null);
        } else if (p.type === "drop") {
          setDragging(null);
          const path = p.paths.find(isGcodePath);
          if (path) void openPath(path);
          else if (p.paths.length > 0) toast.error("Only G-code files (.nc, .gcode, …) can be opened.");
        }
      })
      .then((u) => (cancelled ? u() : (unlisten = u)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  return dragging;
}

export default function App() {
  const ready = useStore((s) => s.state !== null);
  const connected = useConnected();
  const laserIn = useStore((s) => s.state?.status?.laser?.mode === true);
  const dragging = useFileDrop();
  const mainLayout = useDefaultLayout({ id: "main", panelIds: ["top", "bottom"] });
  const bottomLayout = useDefaultLayout({ id: "bottom", panelIds: ["editor", "console"] });

  return (
    <TooltipProvider>
      <div className="flex h-screen flex-col bg-background text-foreground">
        <header className="flex items-center gap-4 border-b px-4 py-2">
          <h1 className="flex items-center gap-2 font-heading text-sm font-semibold tracking-wide">
            <img src="/logo.svg" alt="" className="size-7" />
            Hobby Jogger
          </h1>
          {ready ? <ConnectionBar /> : <span className="text-sm text-muted-foreground">Starting…</span>}
          <div className="flex-1" />
          <SettingsDialog />
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button
                variant="outline"
                className="h-9"
                disabled={!connected}
                title="Reboot the motion controller"
              >
                <Power /> Reset
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Reset the controller?</AlertDialogTitle>
                <AlertDialogDescription>
                  Reboots the motion controller, aborting any job or move and stopping the spindle.
                  Use it to recover from halts that Unlock can't clear.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={() => act({ type: "reset" })}>
                  Reset
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
          <Button
            variant="destructive"
            className="h-9 bg-red-600 px-4 font-semibold text-white hover:bg-red-700"
            disabled={!connected}
            onClick={() => act({ type: "halt" })}
            title="Stop everything immediately (Ctrl-X). Clear with Unlock."
          >
            <OctagonX /> HALT
          </Button>
        </header>

        <ErrorBoundary>
          <ResizablePanelGroup
            orientation="vertical"
            className="min-h-0 flex-1"
            defaultLayout={mainLayout.defaultLayout}
            onLayoutChanged={mainLayout.onLayoutChanged}
          >
            <ResizablePanel id="top" defaultSize="62" minSize={300}>
              <main className="grid h-full min-h-0 grid-cols-[22rem_1fr_22rem] gap-3 p-3">
                <ScrollColumn>
                  <Panel name="Position">
                    <Dro />
                  </Panel>
                  <Panel name="Job">
                    <JobPanel />
                  </Panel>
                  <Panel name="Machine">
                    <MachineControls />
                  </Panel>
                  <Panel name="Markers">
                    <MarkersPanel />
                  </Panel>
                  {laserIn && (
                    <Panel name="Laser">
                      <LaserPanel />
                    </Panel>
                  )}
                </ScrollColumn>
                <div className="min-h-0 rounded-xl ring-1 ring-foreground/10">
                  <BedView />
                </div>
                <ScrollColumn>
                  <Panel name="Jog">
                    <JogPanel />
                  </Panel>
                  <Panel name="Probe">
                    <ProbePanel />
                  </Panel>
                </ScrollColumn>
              </main>
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel id="bottom" defaultSize="38" minSize={160}>
              <ResizablePanelGroup
                orientation="horizontal"
                defaultLayout={bottomLayout.defaultLayout}
                onLayoutChanged={bottomLayout.onLayoutChanged}
              >
                <ResizablePanel id="editor" defaultSize="60" minSize={320}>
                  <ProgramEditor />
                </ResizablePanel>
                <ResizableHandle />
                <ResizablePanel id="console" defaultSize="40" minSize={240}>
                  <ConsolePanel />
                </ResizablePanel>
              </ResizablePanelGroup>
            </ResizablePanel>
          </ResizablePanelGroup>
        </ErrorBoundary>

        {dragging !== null && (
          <div className="pointer-events-none fixed inset-3 z-50 flex items-center justify-center rounded-2xl border-2 border-dashed border-sky-400 bg-background/80">
            <div className="flex flex-col items-center gap-2 text-lg">
              <FileDown className="size-10 text-sky-400" />
              {dragging ? `Open ${dragging}` : "Not a G-code file"}
            </div>
          </div>
        )}
      </div>
      <ToolChangeDialog />
      <AboutDialog />
      <TitleTooltips />
      {/* Error messages are worth copying. */}
      <Toaster position="bottom-right" theme="dark" toastOptions={{ className: "select-text" }} />
    </TooltipProvider>
  );
}
