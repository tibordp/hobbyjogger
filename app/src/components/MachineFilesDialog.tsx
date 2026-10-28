import { ArrowUp, FileCode, Folder, HardDrive, LoaderCircle, RefreshCw, Trash2, X } from "lucide-react";
import { useState } from "react";

import type { FileEntry } from "@/bindings/FileEntry";
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
import { act } from "@/lib/actions";
import { useConnected, useJobRunning } from "@/lib/machine";
import { openRemote } from "@/lib/program";
import { useStore } from "@/lib/store";

function size(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** Browser for the machine's SD card, opened from the editor's toolbar. Opening a file loads it in
 * the editor. */
export function MachineFilesDialog() {
  const connected = useConnected();
  const jobRunning = useJobRunning();
  const files = useStore((s) => s.state?.files);
  const transferring = useStore((s) => s.state?.transfer != null);
  const [remove, setRemove] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  const dir = files?.dir ?? "/sd/gcodes";
  const canOpen = connected && !jobRunning && !transferring;
  const list = (d?: string) => act({ type: "listFiles", dir: d });
  const parent = dir.slice(0, dir.lastIndexOf("/"));

  const row = (e: FileEntry) => {
    const path = `${dir}/${e.name}`;
    if (e.dir) {
      return (
        <button
          key={e.name}
          type="button"
          className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-muted"
          onClick={() => list(path)}
        >
          <Folder className="size-4 shrink-0 text-sky-400" />
          <span className="truncate">{e.name}</span>
        </button>
      );
    }
    return (
      <div key={e.name} className="group flex items-center gap-2 rounded-md px-2 py-1 text-sm hover:bg-muted">
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-2 text-left disabled:cursor-default"
          disabled={!canOpen}
          title={`Open ${e.name} in the editor`}
          onClick={() => {
            setOpen(false);
            void openRemote(path);
          }}
        >
          <FileCode className="size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="truncate">{e.name}</div>
            <div className="text-xs text-muted-foreground">
              {size(e.size)}
              {e.modified && ` · ${e.modified}`}
            </div>
          </div>
        </button>
        <Button
          size="icon-sm"
          variant="ghost"
          className="opacity-0 group-hover:opacity-100"
          disabled={!connected || files?.busy}
          title="Delete"
          onClick={() => setRemove(path)}
        >
          <Trash2 />
        </Button>
      </div>
    );
  };

  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o && connected) void list();
      }}
    >
      <AlertDialogTrigger asChild>
        <Button size="sm" variant="ghost" disabled={!connected} title="Open a file from the machine's SD card">
          <HardDrive />
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent className="flex max-h-[80vh] flex-col data-[size=default]:max-w-lg data-[size=default]:sm:max-w-lg">
        <AlertDialogHeader>
          <div className="flex w-full items-center gap-1">
            <AlertDialogTitle className="flex-1 text-left">Machine files</AlertDialogTitle>
            {files?.busy && <LoaderCircle className="size-4 animate-spin text-muted-foreground" />}
            <Button size="icon-sm" variant="ghost" disabled={!connected || dir === "/sd"} title="Up" onClick={() => list(parent)}>
              <ArrowUp />
            </Button>
            <Button size="icon-sm" variant="ghost" disabled={!connected || files?.busy} title="Refresh" onClick={() => list()}>
              <RefreshCw />
            </Button>
            <Button size="icon-sm" variant="ghost" title="Close" onClick={() => setOpen(false)}>
              <X />
            </Button>
          </div>
          <AlertDialogDescription className="truncate text-left font-mono text-xs">{dir}</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="-mx-2 min-h-24 flex-1 overflow-y-auto">
          {files?.entries == null ? (
            <p className="px-2 text-sm text-muted-foreground">Loading…</p>
          ) : files.entries.length === 0 ? (
            <p className="px-2 text-sm text-muted-foreground">Empty.</p>
          ) : (
            files.entries.map(row)
          )}
        </div>

        <AlertDialog open={remove !== null} onOpenChange={(o) => !o && setRemove(null)}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Delete {remove?.split("/").pop()}?</AlertDialogTitle>
              <AlertDialogDescription>It's removed from the machine's SD card for good.</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                onClick={() => remove && void act({ type: "deleteFile", path: remove })}
              >
                Delete
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </AlertDialogContent>
    </AlertDialog>
  );
}
