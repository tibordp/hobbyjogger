import { OctagonX, Power } from "lucide-react";

import { BedView } from "@/components/BedView";
import { ConnectionBar } from "@/components/ConnectionBar";
import { ConsolePanel } from "@/components/ConsolePanel";
import { Dro } from "@/components/Dro";
import { JogPanel } from "@/components/JogPanel";
import { MachineControls } from "@/components/MachineControls";
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
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { act } from "@/lib/actions";
import { useConnected } from "@/lib/machine";
import { useStore } from "@/lib/store";

export default function App() {
  const ready = useStore((s) => s.state !== null);
  const connected = useConnected();

  return (
    <TooltipProvider>
      <div className="grid h-screen grid-rows-[auto_1fr_14rem] bg-background text-foreground">
        <header className="flex items-center gap-4 border-b px-4 py-2">
          <h1 className="flex items-center gap-2 font-heading text-sm font-semibold tracking-wide">
            <img src="/logo.svg" alt="" className="size-7" />
            Hobby Jogger
          </h1>
          {ready ? <ConnectionBar /> : <span className="text-sm text-muted-foreground">Starting…</span>}
          <div className="flex-1" />
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

        <main className="grid min-h-0 grid-cols-[22rem_1fr_22rem] gap-3 p-3">
          <div className="flex min-h-0 flex-col gap-3 overflow-y-auto">
            <Dro />
            <MachineControls />
          </div>
          <div className="min-h-0 rounded-xl ring-1 ring-foreground/10">
            <BedView />
          </div>
          <div className="min-h-0 overflow-y-auto">
            <JogPanel />
          </div>
        </main>

        <section className="min-h-0 border-t">
          <ConsolePanel />
        </section>
      </div>
      <Toaster position="bottom-right" theme="dark" />
    </TooltipProvider>
  );
}
