import { Cable, Cpu, Plug, Unplug, Wifi } from "lucide-react";
import { useState } from "react";

import type { ConnectTarget } from "@/bindings/ConnectTarget";
import type { DiscoveredMachine } from "@/bindings/DiscoveredMachine";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { act } from "@/lib/actions";
import { useStore } from "@/lib/store";

function parseHost(text: string): ConnectTarget | null {
  const t = text.trim();
  if (!t) return null;
  const [host, port] = t.split(":");
  const p = port ? Number(port) : undefined;
  if (p !== undefined && !(Number.isInteger(p) && p > 0 && p < 65536)) return null;
  return { kind: "tcp", host, port: p };
}

/** A stable empty list: a selector returning a fresh `[]` would re-render forever. */
const NONE: DiscoveredMachine[] = [];

export function ConnectionBar() {
  const connection = useStore((s) => s.state?.connection);
  const machine = useStore((s) => s.state?.machine);
  const discovered = useStore((s) => s.state?.discovered ?? NONE);
  const [choice, setChoice] = useState<string>("");
  const [host, setHost] = useState("");

  if (!connection) return null;

  if (connection.kind === "connected") {
    const Icon =
      connection.transport === "usb" ? Cable : connection.transport === "simulator" ? Cpu : Wifi;
    return (
      <div className="flex items-center gap-3">
        <Badge variant="secondary" className="gap-1.5">
          <Icon className="size-3.5" />
          {connection.target}
        </Badge>
        <span className="text-sm text-muted-foreground">
          {machine?.model ?? "…"}
          {machine?.firmware && <> · fw {machine.firmware}</>}
        </span>
        <Button variant="ghost" size="sm" onClick={() => act({ type: "disconnect" })}>
          <Unplug /> Disconnect
        </Button>
      </div>
    );
  }

  const connecting = connection.kind === "connecting";
  const target = (): ConnectTarget | null => {
    if (choice === "usb") return { kind: "usb" };
    if (choice === "sim") return { kind: "simulator" };
    if (choice === "manual") return parseHost(host);
    const m = discovered.find((d) => `${d.host}:${d.port}` === choice);
    return m ? { kind: "tcp", host: m.host, port: m.port } : null;
  };
  const t = target();

  return (
    <div className="flex items-center gap-2">
      <Select value={choice} onValueChange={setChoice}>
        <SelectTrigger className="w-64" size="sm">
          <SelectValue placeholder="Choose a machine" />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectLabel>On the network</SelectLabel>
            {discovered.length === 0 && (
              <SelectItem value="none" disabled>
                Searching…
              </SelectItem>
            )}
            {discovered.map((d) => (
              <SelectItem key={`${d.host}:${d.port}`} value={`${d.host}:${d.port}`}>
                {d.name} <span className="text-muted-foreground">{d.host}</span>
                {d.busy && <span className="text-amber-500">busy</span>}
              </SelectItem>
            ))}
          </SelectGroup>
          <SelectSeparator />
          <SelectItem value="manual">Address…</SelectItem>
          <SelectItem value="usb">USB</SelectItem>
          <SelectItem value="sim">Simulator</SelectItem>
        </SelectContent>
      </Select>
      {choice === "manual" && (
        <Input
          className="h-8 w-44"
          placeholder="192.168.0.55[:2222]"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && t && act({ type: "connect", target: t })}
        />
      )}
      <Button
        size="sm"
        disabled={!t || connecting}
        onClick={() => t && act({ type: "connect", target: t })}
      >
        <Plug /> {connecting ? "Connecting…" : "Connect"}
      </Button>
      {connection.kind === "disconnected" && connection.error && (
        <span className="max-w-md truncate text-sm text-destructive" title={connection.error}>
          {connection.error}
        </span>
      )}
    </div>
  );
}
