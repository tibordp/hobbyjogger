import { LoaderCircle, RefreshCw, Settings, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { Setting } from "@/bindings/Setting";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { act } from "@/lib/actions";
import { useConnected } from "@/lib/machine";
import { useStore } from "@/lib/store";

/** The machine's configuration, read-only, grouped and explained where we know the keys. */
export function SettingsDialog() {
  const connected = useConnected();
  const settings = useStore((s) => s.state?.settings ?? null);
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");

  // Read them the first time the dialog opens.
  useEffect(() => {
    if (open && connected && settings && !settings.entries && !settings.loading && !settings.error) {
      void act({ type: "loadSettings" });
    }
  }, [open, connected, settings]);

  const groups = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const map = new Map<string, Setting[]>();
    for (const s of settings?.entries ?? []) {
      const text = `${s.key} ${s.value} ${s.title ?? ""} ${s.detail ?? ""}`.toLowerCase();
      if (f && !text.includes(f)) continue;
      map.set(s.group, [...(map.get(s.group) ?? []), s]);
    }
    return [...map];
  }, [settings, filter]);

  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      <AlertDialogTrigger asChild>
        <Button variant="outline" className="h-9" disabled={!connected} title="The machine's configuration (config.txt)">
          <Settings /> Machine settings
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent className="flex max-h-[85vh] flex-col data-[size=default]:max-w-2xl data-[size=default]:sm:max-w-2xl">
        <AlertDialogHeader>
          <div className="flex w-full items-center gap-2">
            <AlertDialogTitle className="flex-1 text-left">Machine settings</AlertDialogTitle>
            <Button
              size="icon-sm"
              variant="ghost"
              title="Read again"
              disabled={!connected || settings?.loading}
              onClick={() => act({ type: "loadSettings" })}
            >
              <RefreshCw />
            </Button>
            <Button size="icon-sm" variant="ghost" title="Close" onClick={() => setOpen(false)}>
              <X />
            </Button>
          </div>
          <AlertDialogDescription className="text-left">
            From the machine's <code>config.txt</code>, read-only. Change one from the console with{" "}
            <code>config-set sd &lt;key&gt; &lt;value&gt;</code>; it takes effect after a reset.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <Input placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <div className="-mx-4 min-h-0 flex-1 overflow-y-auto px-4">
          {settings?.loading && (
            <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
              <LoaderCircle className="size-4 animate-spin" /> Reading settings…
            </div>
          )}
          {settings?.error && <p className="py-6 text-sm text-red-400">{settings.error}</p>}
          {!settings?.loading &&
            groups.map(([group, items]) => (
              <section key={group} className="mb-4">
                <h3 className="sticky top-0 bg-popover py-1 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                  {group}
                </h3>
                <dl className="divide-y divide-border/60">
                  {items.map((s) => (
                    <div key={s.key} className="grid grid-cols-[1fr_auto] gap-x-4 py-1.5 text-sm">
                      <dt className="min-w-0">
                        <div className={s.title ? "" : "font-mono text-xs select-text"}>{s.title ?? s.key}</div>
                        {s.detail && <div className="text-xs text-muted-foreground">{s.detail}</div>}
                        {s.title && <div className="font-mono text-[11px] text-muted-foreground/70 select-text">{s.key}</div>}
                      </dt>
                      <dd className="max-w-56 truncate text-right font-mono text-xs select-text" title={s.value}>
                        {s.value}
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          {!settings?.loading && settings?.entries && groups.length === 0 && (
            <p className="py-6 text-sm text-muted-foreground">Nothing matches.</p>
          )}
        </div>
      </AlertDialogContent>
    </AlertDialog>
  );
}
