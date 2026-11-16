// The bed in 3D, view only: the program as lines in space, with the travel limits, the work origin
// and where the head is. Machine coordinates throughout, Z up, as on the 2D map. Loaded on first
// use, so the 2D map doesn't pay for three.js. It draws when something changes, not continuously.
import { Box, Focus } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

import type { Analysis } from "@/bindings/Analysis";
import { Button } from "@/components/ui/button";
import { useConnected, useJobRunning } from "@/lib/machine";
import { useFollowing, useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

/** Used until the machine reports its limits (Z1 values). */
const DEFAULT_MIN = [-207, -206, -102];

const JUMP = 0;
const RAPID = 1;

type Vec3 = [number, number, number];
type Box3 = { min: Vec3; max: Vec3 };

/** Where to look from, toward the target. +Y is the back of the machine. */
const VIEWS = {
  top: [0, -1e-4, 1],
  front: [0, -1, 0],
  iso: [0.65, -1, 0.75],
} satisfies Record<string, Vec3>;
type View = keyof typeof VIEWS;

/** A CSS colour (custom properties and oklch included) as three.js understands it. */
function cssColor(value: string): THREE.Color {
  const c = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  if (!c) return new THREE.Color(0x888888);
  c.fillStyle = value;
  c.fillRect(0, 0, 1, 1);
  const [r, g, b] = c.getImageData(0, 0, 1, 1).data;
  return new THREE.Color(r / 255, g / 255, b / 255).convertSRGBToLinear();
}

function themeColors() {
  const v = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return {
    border: cssColor(v("--border")),
    muted: cssColor(v("--muted-foreground")),
    feed: cssColor("#e879f9"), // fuchsia-400, as in 2D
    highlight: cssColor("#fde047"), // yellow-300
    tool: cssColor("#38bdf8"), // sky-400
    contact: cssColor("#7dd3fc"), // sky-300
    x: cssColor("#f87171"),
    y: cssColor("#4ade80"),
    z: cssColor("#60a5fa"),
  };
}

/** First vertex index whose source line is after `line` (lines never decrease). */
function vertexAfterLine(a: Analysis, line: number) {
  let lo = 0;
  let hi = a.lines.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (a.lines[mid] <= line) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** How many of `ends` (ascending) are below `v`. */
function countBelow(ends: Uint32Array, v: number) {
  let lo = 0;
  let hi = ends.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (ends[mid] < v) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Line segments of one kind (rapid or feed), placed at `offset`, and the vertex each ends at. */
function segments(a: Analysis, rapid: boolean, offset: Vec3) {
  const n = a.kinds.length;
  const pos: number[] = [];
  const ends: number[] = [];
  for (let i = 1; i < n; i++) {
    const k = a.kinds[i];
    if (k === JUMP || (k === RAPID) !== rapid) continue;
    for (const j of [i - 1, i]) {
      pos.push(a.points[j * 3] + offset[0], a.points[j * 3 + 1] + offset[1], a.points[j * 3 + 2] + offset[2]);
    }
    ends.push(i);
  }
  return { pos: new Float32Array(pos), ends: new Uint32Array(ends) };
}

/** A set of segments drawn as done (dim) and still to go, split at a segment count. */
class Path {
  readonly group = new THREE.Group();
  private readonly done: THREE.LineSegments;
  private readonly rest: THREE.LineSegments;
  readonly ends: Uint32Array;

  constructor(pos: Float32Array, ends: Uint32Array, done: THREE.LineBasicMaterial, rest: THREE.LineBasicMaterial) {
    this.ends = ends;
    const attr = new THREE.BufferAttribute(pos, 3);
    const make = (m: THREE.LineBasicMaterial) => {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", attr);
      const line = new THREE.LineSegments(g, m);
      if (m instanceof THREE.LineDashedMaterial) line.computeLineDistances();
      line.frustumCulled = false;
      return line;
    };
    this.done = make(done);
    this.rest = make(rest);
    this.group.add(this.done, this.rest);
    this.split(0);
  }

  /** Vertices before `vertex` are done. */
  split(vertex: number) {
    const k = countBelow(this.ends, vertex) * 2;
    this.done.geometry.setDrawRange(0, k);
    this.rest.geometry.setDrawRange(k, Infinity);
  }

  dispose() {
    this.done.geometry.dispose();
    this.rest.geometry.dispose();
  }
}

export default function Bed3D() {
  const host = useRef<HTMLDivElement>(null);
  const [api, setApi] = useState<ReturnType<typeof setup> | null>(null);
  const [perspective, setPerspective] = useState(false);

  const connected = useConnected();
  const t = useStore((s) => s.telemetry);
  const status = useStore((s) => s.state?.status ?? null);
  const travel = useStore((s) => s.state?.machine?.travel);
  const probe = useStore((s) => s.state?.probe ?? null);
  const analysis = useProgram((s) => s.analysis);
  const cursorLine = useProgram((s) => s.cursorLine);
  const following = useFollowing();
  const job = useJobRunning();
  const doneLine = following && status?.job ? status.job.lines : 0;

  // Machine − work, tool length included (so Z matches the DRO); rounded so it doesn't rebuild
  // the path on every report.
  const round = (v: number) => Math.round(v * 1e4) / 1e4;
  const live = connected && t;
  const ox = live ? round(t.machine[0] - t.work[0]) : status?.workOffset[0];
  const oy = live ? round(t.machine[1] - t.work[1]) : status?.workOffset[1];
  const oz = live ? round(t.machine[2] - t.work[2]) : status?.workOffset[2];
  const offset = useMemo<Vec3 | null>(
    () => (ox === undefined || oy === undefined || oz === undefined ? null : [ox, oy, oz]),
    [ox, oy, oz],
  );

  const bed = useMemo<Box3>(
    () => ({
      min: [0, 1, 2].map((i) => travel?.min[i] ?? DEFAULT_MIN[i]) as Vec3,
      max: [0, 1, 2].map((i) => travel?.max[i] ?? -1) as Vec3,
    }),
    [travel],
  );

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const a = setup(el);
    setApi(a);
    return () => {
      a.dispose();
      setApi(null);
    };
  }, []);

  useEffect(() => api?.setBed(bed), [api, bed]);
  useEffect(() => api?.setOrigin(offset), [api, offset]);
  useEffect(() => api?.setProgram(analysis, offset), [api, analysis, offset]);
  useEffect(() => api?.setDone(analysis && doneLine > 0 ? vertexAfterLine(analysis, doneLine) : 0), [api, analysis, doneLine, offset]);
  useEffect(() => {
    if (!api) return;
    if (!analysis || !offset || cursorLine === null) return api.setHighlight(null);
    const from = vertexAfterLine(analysis, cursorLine - 1);
    const to = vertexAfterLine(analysis, cursorLine);
    const pos: number[] = [];
    for (let i = Math.max(from, 1); i < to; i++) {
      if (analysis.kinds[i] === JUMP) continue;
      for (const j of [i - 1, i]) {
        pos.push(...[0, 1, 2].map((k) => analysis.points[j * 3 + k] + offset[k]));
      }
    }
    api.setHighlight(pos.length ? new Float32Array(pos) : null);
  }, [api, analysis, offset, cursorLine]);
  const mx = live ? t.machine[0] : null;
  const my = live ? t.machine[1] : null;
  const mz = live ? t.machine[2] : null;
  useEffect(() => api?.setTool(mx === null || my === null || mz === null ? null : [mx, my, mz], bed.min[2]), [api, mx, my, mz, bed]);
  const contacts = probe?.contacts;
  useEffect(() => api?.setContacts(contacts ?? []), [api, contacts]);
  useEffect(() => api?.setPerspective(perspective), [api, perspective]);

  // Frame the program when one appears (or the bed, without), from the angle.
  const framed = useRef(false);
  const hasProgram = analysis !== null && offset !== null;
  useEffect(() => {
    if (!api) return;
    if (!framed.current || hasProgram) api.look("iso", fitBox(analysis, offset, bed));
    framed.current = true;
    // Only when a program comes or goes, not as it's edited or the offset moves.
  }, [api, hasProgram]);

  const look = (view: View) => api?.look(view, fitBox(analysis, offset, bed));

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={host} className="absolute inset-0 cursor-grab active:cursor-grabbing" />
      <div className="absolute top-2 left-2 flex gap-1">
        {(Object.keys(VIEWS) as View[]).map((v) => (
          <Button key={v} size="xs" variant="outline" className="bg-background/70 capitalize" onClick={() => look(v)}>
            {v}
          </Button>
        ))}
        <Button
          size="xs"
          variant="outline"
          className="bg-background/70"
          title={analysis ? "Fit the program" : "Fit the bed"}
          onClick={() => api?.fit(fitBox(analysis, offset, bed))}
        >
          <Focus /> Fit
        </Button>
        <Button
          size="xs"
          variant="outline"
          className={cn("bg-background/70", perspective && "border-ring")}
          title="Perspective, or orthographic (true to scale)"
          onClick={() => setPerspective((p) => !p)}
        >
          <Box /> {perspective ? "Perspective" : "Orthographic"}
        </Button>
      </div>
      <div className="pointer-events-none absolute bottom-2 left-2 text-[11px] text-muted-foreground">
        Drag to orbit · middle- or right-drag to pan · scroll to zoom{job && following ? " · done shown dim" : ""}
      </div>
    </div>
  );
}

/** The program's extent where it is, or the bed's. */
function fitBox(analysis: Analysis | null, offset: Vec3 | null, bed: Box3): Box3 {
  const b = analysis?.bounds;
  if (!b || !offset) return bed;
  return {
    min: [0, 1, 2].map((i) => b.min[i] + offset[i]) as Vec3,
    max: [0, 1, 2].map((i) => b.max[i] + offset[i]) as Vec3,
  };
}

/** The scene and its controls; the component feeds it state. */
function setup(el: HTMLDivElement) {
  const colors = themeColors();
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(window.devicePixelRatio);
  el.appendChild(renderer.domElement);
  renderer.domElement.style.display = "block";

  const scene = new THREE.Scene();
  const ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, -10000, 10000);
  const persp = new THREE.PerspectiveCamera(35, 1, 1, 10000);
  for (const c of [ortho, persp]) c.up.set(0, 0, 1);
  let camera: THREE.OrthographicCamera | THREE.PerspectiveCamera = ortho;
  /** Half the ortho camera's view height at zoom 1, mm. */
  const HALF = 100;

  let controls = makeControls();
  function makeControls() {
    const c = new OrbitControls(camera, renderer.domElement);
    c.enableDamping = false;
    c.zoomToCursor = true;
    // Middle drag pans, as in CAD tools (three.js zooms with it by default); the wheel zooms.
    c.mouseButtons = { LEFT: THREE.MOUSE.ROTATE, MIDDLE: THREE.MOUSE.PAN, RIGHT: THREE.MOUSE.PAN };
    c.addEventListener("change", render);
    return c;
  }

  let frame = 0;
  function render() {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      renderer.render(scene, camera);
    });
  }

  function resize() {
    const w = Math.max(el.clientWidth, 1);
    const h = Math.max(el.clientHeight, 1);
    renderer.setSize(w, h);
    const aspect = w / h;
    ortho.left = -HALF * aspect;
    ortho.right = HALF * aspect;
    ortho.top = HALF;
    ortho.bottom = -HALF;
    ortho.updateProjectionMatrix();
    persp.aspect = aspect;
    persp.updateProjectionMatrix();
    render();
  }
  const ro = new ResizeObserver(resize);
  ro.observe(el);
  resize();

  // Materials, shared by what's rebuilt.
  const feed = new THREE.LineBasicMaterial({ color: colors.feed });
  const feedDone = new THREE.LineBasicMaterial({ color: colors.feed, transparent: true, opacity: 0.25 });
  const rapid = new THREE.LineDashedMaterial({ color: colors.muted, dashSize: 1, gapSize: 1, transparent: true, opacity: 0.6 });
  const rapidDone = new THREE.LineDashedMaterial({ color: colors.muted, dashSize: 1, gapSize: 1, transparent: true, opacity: 0.2 });
  const highlight = new THREE.LineBasicMaterial({ color: colors.highlight, depthTest: false });

  const bedGroup = new THREE.Group();
  const origin = new THREE.Group();
  const program = new THREE.Group();
  const marks = new THREE.Group();
  scene.add(bedGroup, origin, program, marks);
  let paths: Path[] = [];
  let highlightLine: THREE.LineSegments | null = null;
  let tool: THREE.Group | null = null;
  let contacts: THREE.Points | null = null;

  const clear = (g: THREE.Object3D) => {
    for (const c of [...g.children]) {
      g.remove(c);
      c.traverse((o) => {
        if (o instanceof THREE.Mesh || o instanceof THREE.Line || o instanceof THREE.Points) {
          o.geometry.dispose();
          // Shared materials are disposed at the end; per-object ones here.
          if (![feed, feedDone, rapid, rapidDone, highlight].includes(o.material)) (o.material as THREE.Material).dispose();
        }
      });
    }
  };
  const lines = (pos: number[], material: THREE.Material) => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
    return new THREE.LineSegments(g, material);
  };

  return {
    /** The travel limits as a box, with a 10 mm grid on its floor. */
    setBed(b: Box3) {
      clear(bedGroup);
      const [x0, y0, z0] = b.min;
      const [x1, y1, z1] = b.max;
      const box: number[] = [];
      const corners = (z: number) => [
        [x0, y0, z],
        [x1, y0, z],
        [x1, y1, z],
        [x0, y1, z],
      ];
      for (const z of [z0, z1]) {
        const c = corners(z);
        for (let i = 0; i < 4; i++) box.push(...c[i], ...c[(i + 1) % 4]);
      }
      for (const [x, y] of corners(0)) box.push(x, y, z0, x, y, z1);
      bedGroup.add(lines(box, new THREE.LineBasicMaterial({ color: colors.border })));
      const minor: number[] = [];
      const major: number[] = [];
      for (let x = Math.ceil(x0 / 10) * 10; x <= x1; x += 10) (x % 50 === 0 ? major : minor).push(x, y0, z0, x, y1, z0);
      for (let y = Math.ceil(y0 / 10) * 10; y <= y1; y += 10) (y % 50 === 0 ? major : minor).push(x0, y, z0, x1, y, z0);
      bedGroup.add(lines(minor, new THREE.LineBasicMaterial({ color: colors.border, transparent: true, opacity: 0.35 })));
      bedGroup.add(lines(major, new THREE.LineBasicMaterial({ color: colors.border })));
      render();
    },
    /** X/Y/Z arrows at the work origin. */
    setOrigin(o: Vec3 | null) {
      clear(origin);
      if (o) {
        const axis = (d: Vec3, color: THREE.Color) =>
          lines([...o, o[0] + d[0], o[1] + d[1], o[2] + d[2]], new THREE.LineBasicMaterial({ color, depthTest: false }));
        origin.add(axis([15, 0, 0], colors.x), axis([0, 15, 0], colors.y), axis([0, 0, 15], colors.z));
        origin.renderOrder = 2;
      }
      render();
    },
    setProgram(a: Analysis | null, o: Vec3 | null) {
      for (const p of paths) p.dispose();
      paths = [];
      clear(program);
      if (a && o) {
        const r = segments(a, true, o);
        const f = segments(a, false, o);
        paths = [new Path(r.pos, r.ends, rapidDone, rapid), new Path(f.pos, f.ends, feedDone, feed)];
        for (const p of paths) program.add(p.group);
      }
      render();
    },
    setDone(vertex: number) {
      for (const p of paths) p.split(vertex);
      render();
    },
    /** The editor's current line, drawn over everything. */
    setHighlight(pos: Float32Array | null) {
      if (highlightLine) {
        marks.remove(highlightLine);
        highlightLine.geometry.dispose();
        highlightLine = null;
      }
      if (pos) {
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.BufferAttribute(pos, 3));
        highlightLine = new THREE.LineSegments(g, highlight);
        highlightLine.renderOrder = 3;
        marks.add(highlightLine);
      }
      render();
    },
    /** Where the head is: a dot, with a line down to the floor for depth. No tool model. */
    setTool(p: Vec3 | null, floor: number) {
      if (tool) {
        clear(tool);
        marks.remove(tool);
        tool = null;
      }
      if (p) {
        tool = new THREE.Group();
        const dot = new THREE.BufferGeometry();
        dot.setAttribute("position", new THREE.Float32BufferAttribute(p, 3));
        tool.add(new THREE.Points(dot, new THREE.PointsMaterial({ color: colors.tool, size: 9, sizeAttenuation: false, depthTest: false })));
        tool.add(
          lines([...p, p[0], p[1], floor], new THREE.LineDashedMaterial({ color: colors.tool, dashSize: 2, gapSize: 2, transparent: true, opacity: 0.5 })),
        );
        (tool.children[1] as THREE.LineSegments).computeLineDistances();
        tool.renderOrder = 4;
        marks.add(tool);
      }
      render();
    },
    setContacts(points: [number, number, number][]) {
      if (contacts) {
        marks.remove(contacts);
        contacts.geometry.dispose();
        (contacts.material as THREE.Material).dispose();
        contacts = null;
      }
      if (points.length) {
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.Float32BufferAttribute(points.flat(), 3));
        contacts = new THREE.Points(g, new THREE.PointsMaterial({ color: colors.contact, size: 6, sizeAttenuation: false }));
        marks.add(contacts);
      }
      render();
    },
    setPerspective(on: boolean) {
      const next = on ? persp : ortho;
      if (next === camera) return;
      // Keep the view: same direction and target, at a distance that shows about as much.
      const target = controls.target.clone();
      const dir = camera.position.clone().sub(target).normalize();
      const shown = camera === ortho ? (2 * HALF) / ortho.zoom : 2 * camera.position.distanceTo(target) * Math.tan(THREE.MathUtils.degToRad(persp.fov / 2));
      if (next === persp) {
        persp.position.copy(target).addScaledVector(dir, shown / 2 / Math.tan(THREE.MathUtils.degToRad(persp.fov / 2)));
      } else {
        ortho.position.copy(target).addScaledVector(dir, 1000);
        ortho.zoom = (2 * HALF) / shown;
        ortho.updateProjectionMatrix();
      }
      controls.dispose();
      camera = next;
      controls = makeControls();
      controls.target.copy(target);
      controls.update();
      render();
    },
    /** Frame `box` without changing the angle. */
    fit(box: Box3) {
      const dir = camera.position.clone().sub(controls.target).normalize();
      frame3(box, dir);
    },
    look(view: View, box: Box3) {
      frame3(box, new THREE.Vector3(...VIEWS[view]).normalize());
    },
    dispose() {
      cancelAnimationFrame(frame);
      ro.disconnect();
      controls.dispose();
      for (const p of paths) p.dispose();
      for (const g of [bedGroup, origin, program, marks]) clear(g);
      for (const m of [feed, feedDone, rapid, rapidDone, highlight]) m.dispose();
      renderer.dispose();
      renderer.domElement.remove();
    },
  };

  /** Look along `dir` at the centre of `box`, close enough that it fills most of the view. */
  function frame3(box: Box3, dir: THREE.Vector3) {
    const b = new THREE.Box3(new THREE.Vector3(...box.min), new THREE.Vector3(...box.max));
    const center = b.getCenter(new THREE.Vector3());
    // Room for small programs too.
    const radius = Math.max(b.getSize(new THREE.Vector3()).length() / 2, 10);
    const aspect = Math.max(el.clientWidth, 1) / Math.max(el.clientHeight, 1);
    const half = radius * 1.1 * Math.max(1, 1 / aspect);
    controls.target.copy(center);
    if (camera === ortho) {
      ortho.position.copy(center).addScaledVector(dir, 1000);
      ortho.zoom = HALF / half;
      ortho.updateProjectionMatrix();
    } else {
      persp.position.copy(center).addScaledVector(dir, half / Math.tan(THREE.MathUtils.degToRad(persp.fov / 2)));
    }
    controls.update();
    render();
  }
}
