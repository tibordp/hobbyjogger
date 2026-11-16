import { useState } from "react";

import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/** A labelled number, committed on blur or Enter; blank while `value` is null. Positive only unless
 * `signed`. */
export function Field({
  label,
  value,
  onChange,
  title,
  signed = false,
  unit = "mm",
  className,
}: {
  label: string;
  value: number | null;
  onChange: (v: number) => void;
  title?: string;
  signed?: boolean;
  unit?: string;
  className?: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <label className={cn("flex items-center gap-1.5 text-xs text-muted-foreground", className)} title={title}>
      {label}
      <Input
        className="h-7 w-16 px-1.5 text-right font-mono text-xs"
        inputMode="decimal"
        value={draft ?? (value === null ? "" : String(value))}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          const v = Number((draft ?? "").replace(",", "."));
          if (draft !== null && draft.trim() !== "" && Number.isFinite(v) && (signed || v > 0)) onChange(v);
          setDraft(null);
        }}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      {unit}
    </label>
  );
}
