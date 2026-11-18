import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useUi } from "@/lib/ui";

/** Help ▸ About (the app menu on macOS). */
export function AboutDialog() {
  const open = useUi((s) => s.aboutOpen);
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    if (open && version === null && "__TAURI_INTERNALS__" in window) void getVersion().then(setVersion);
  }, [open, version]);
  return (
    <AlertDialog open={open} onOpenChange={(aboutOpen) => useUi.setState({ aboutOpen })}>
      <AlertDialogContent className="sm:max-w-sm">
        <AlertDialogHeader className="items-center text-center sm:items-center sm:text-center">
          <img src="/logo.svg" alt="" className="size-16" />
          <AlertDialogTitle>Hobby Jogger</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="space-y-2">
              {version && <div className="font-mono text-xs">Version {version}</div>}
              <p>A controller for the Makera Z1, over Wi-Fi or USB, with the stock firmware (tested with 1.1.3).</p>
              <p className="text-xs">Free software under the GNU General Public License, version 3.</p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="sm:justify-center">
          <AlertDialogAction>OK</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
