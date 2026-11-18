import { Check, Move3d, RotateCcwSquare, TriangleAlert } from "lucide-react";
import { useState, type ReactNode } from "react";

import type { Problem } from "@/bindings/Problem";
import { Field } from "@/components/Field";
import { Panel, ScrollColumn } from "@/components/ScrollColumn";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { rotateGcode, translateGcode } from "@/lib/backend";
import { currentText, replaceText } from "@/lib/program";
import { setRotate, setTranslate, type Transforms } from "@/lib/transform";
import { useUi } from "@/lib/ui";

/** Lines up a column of fields whose units differ. */
const ROW = { className: "grid grid-cols-[1fr_auto_auto]", unitClassName: "w-7 shrink-0" };

/**
 * Transforms of the program in the editor, in a sidebar at its right edge while open. The card
 * changed last shows on the maps (dashed) as it would come out; applying rewrites the text (one
 * undo step), and if any line can't be transformed, nothing changes and those lines are listed.
 */
export function TransformSidebar() {
  const transforms = useUi((s) => s.transforms);
  if (!transforms) return null;
  return (
    <aside className="flex w-60 shrink-0 flex-col border-l p-2 pr-1">
      <ScrollColumn className="h-full">
        <Panel name="Rotate">
          <RotateCard rotate={transforms.rotate} />
        </Panel>
        <Panel name="Translate">
          <TranslateCard translate={transforms.translate} />
        </Panel>
      </ScrollColumn>
    </aside>
  );
}

/** A card with its fields, an Apply button, and what kept it from applying. */
function TransformCard({
  icon,
  title,
  verb,
  disabled,
  run,
  done,
  children,
}: {
  icon: ReactNode;
  title: string;
  verb: string;
  disabled: boolean;
  /** The transformed text, or a rejection with `Problem[]`. */
  run: (text: string) => Promise<string>;
  /** After applying: back to doing nothing, ready for another. */
  done: () => void;
  children: ReactNode;
}) {
  const [problems, setProblems] = useState<Problem[] | null>(null);
  const apply = async () => {
    try {
      replaceText(await run(currentText()));
      setProblems(null);
      done();
    } catch (e) {
      setProblems(Array.isArray(e) ? (e as Problem[]) : [{ line: 0, message: String(e) }]);
    }
  };
  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle className="flex items-center gap-1.5">
          {icon} {title}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3" onChange={() => setProblems(null)}>
        <div className="space-y-1.5">{children}</div>
        <Button size="sm" className="w-full" onClick={() => void apply()} disabled={disabled} title="Rewrites the program (one undo step)">
          <Check /> Apply
        </Button>
        {problems && (
          <div className="max-h-40 overflow-y-auto rounded-md border border-red-500/40 bg-red-500/10 p-2 text-xs select-text">
            <div className="mb-1 flex items-center gap-1.5 font-medium">
              <TriangleAlert className="size-3.5 shrink-0 text-red-400" /> Not {verb}: the program is unchanged.
            </div>
            {problems.map((p, i) => (
              <div key={i}>
                {p.line > 0 && <span className="font-mono text-muted-foreground">line {p.line}: </span>}
                {p.message}
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function RotateCard({ rotate }: { rotate: Transforms["rotate"] }) {
  return (
    <TransformCard
      icon={<RotateCcwSquare className="size-4" />}
      title="Rotate"
      verb="rotated"
      disabled={rotate.degrees === 0}
      run={(text) => rotateGcode(text, rotate.degrees, rotate.pivot)}
      done={() => setRotate({ degrees: 0 })}
    >
      <Field {...ROW} label="Angle" signed unit="° ⟲" value={rotate.degrees} onChange={(degrees) => setRotate({ degrees })} title="Degrees counter-clockwise, seen from above; negative turns it clockwise" />
      <Field {...ROW} label="About X" signed unit="" value={rotate.pivot[0]} onChange={(x) => setRotate({ pivot: [x, rotate.pivot[1]] })} title="The point it turns about, in the program's coordinates (and units)" />
      <Field {...ROW} label="About Y" signed unit="" value={rotate.pivot[1]} onChange={(y) => setRotate({ pivot: [rotate.pivot[0], y] })} title="The point it turns about, in the program's coordinates (and units)" />
    </TransformCard>
  );
}

function TranslateCard({ translate }: { translate: Transforms["translate"] }) {
  const set = (axis: number, v: number) => setTranslate(translate.map((d, i) => (i === axis ? v : d)) as [number, number, number]);
  return (
    <TransformCard
      icon={<Move3d className="size-4" />}
      title="Translate"
      verb="moved"
      disabled={translate.every((d) => d === 0)}
      run={(text) => translateGcode(text, translate)}
      done={() => setTranslate([0, 0, 0])}
    >
      {(["X", "Y", "Z"] as const).map((axis, i) => (
        <Field
          key={axis}
          {...ROW}
          label={axis}
          signed
          unit=""
          value={translate[i]}
          onChange={(v) => set(i, v)}
          title={`How far to move the program along ${axis}, in its own units${axis === "Z" ? ": for one whose Z0 is the stock's bottom, minus the stock's thickness puts it on the top" : ""}`}
        />
      ))}
    </TransformCard>
  );
}
