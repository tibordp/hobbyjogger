# Open-source prior art survey

Survey of general open-source CNC senders, probing routines, conversational CAM and the Rust/React
ecosystem, as reference material for Hobby Jogger. Licences and activity were checked via the GitHub
API on 2026-10-26; anything marked *unverified* was not confirmed.

**Licence rule of thumb:** most mature senders are GPL — read them for algorithms and re-derive the
math, but don't copy code. Permissive (MIT/BSD/Apache) and safe to port: cncjs, ioSender, Kiri:Moto,
cavalier_contours, clipper2 ports, tauri-specta, ts-rs, json-patch, mdns-sd, tokio-serial.

## 1. G-code senders / controllers

| Project | Lang | Licence | Notable | Borrow |
|---|---|---|---|---|
| [cncjs](https://github.com/cncjs/cncjs) | JS (Node + React) | MIT | Server owns controller state, browser is a thin client (similar split to ours). Macros with safe expression evaluation (`src/server/lib/evaluate-expression.js`, `translate-expression.js`: esprima AST walk, `[expr]` interpolation, `%var=...`, machine vars `posx`/`mposx`). three.js visualizer, pendant/widget API. | Snippet variable design: whitelisted expression evaluator over machine state + user vars. |
| [cncjs-kt-ext](https://github.com/kreso-t/cncjs-kt-ext) | JS | none detected | Popular autolevel extension (probe grid → modify G-code). | Learn only; no licence. |
| [bCNC](https://github.com/vlachoudis/bCNC) | Python/Tk | GPL-2.0 | Probe tab (center, angle, tool change, autolevel w/ bilinear interpolation + segment splitting). ~45 CAM plugins in `bCNC/plugins/` (`drill.py`, `pocket.py`, `Helical_Descent.py`, `trochoidal.py`, `spiral.py`, …). | Autolevel splitting approach; plugin parameter-schema idea. |
| [Universal Gcode Sender](https://github.com/winder/Universal-G-Code-Sender) | Java | GPL-3.0 | `ProbeModule` (Z, XYZ corner, outside XY, hole center) each with a 3D *path preview* (`*PathPreview.java`). AutoLeveler with save/load mesh. Designer with drill/pocket/surface ops. | Preview probe path before running; persisted surface meshes. |
| [gSender](https://github.com/Sienci-Labs/gsender) | TS/React + Node | GPL-3.0 | Modern React UI. `features/Surfacing`, `features/Rotary` (`RotarySurfacing`, `mountingSetupMacros.ts`, `probeCommands.ts`). Native bore/boss probing state *unverified*. | Rotary setup + surfacing UX flow. |
| [ioSender](https://github.com/terjeio/ioSender) | C#/WPF | BSD-3 | Separate "CNC Controls Probing" library: TLO, edge finder, center finder, rotation, height map (feature list *unverified*). | Best permissive probing reference — math can be ported. |
| [Candle](https://github.com/Denvi/Candle) | C++/Qt | GPL-3.0 | Simple GRBL sender, heightmap, OpenGL viewer. | Minimal heightmap UX. |
| [OpenBuilds CONTROL](https://github.com/OpenBuilds/OpenBuilds-CONTROL) | JS/Electron | GPL-3.0 | Probe wizards for XYZ touch block; three.js viewer. Slow-moving. | Touch-plate wizard UX. |
| [GRBL-Plotter](https://github.com/svenhb/GRBL-Plotter) | C# | GPL-3.0 | Sender + SVG/DXF import + probing. | Optional. |
| [Carvera Community Controller](https://github.com/Carvera-Community/Carvera_Controller) | Python/Kivy | GPL-2.0 | Closest reference for Makera WiFi discovery/upload (Carvera; Z1 unverified). | Protocol reading — see `carvera-reference.md`. |
| [GCodeKit5](https://github.com/thawkins/gcodekit5) | Rust | unclear | Rust workspace sender: `-communication`, `-core`, `-camtools`, `-visualizer`, `-devicedb`. Small. | Closest Rust architecture analog; check licence first. |
| [LinuxCNC / QtDragon](https://github.com/LinuxCNC/linuxcnc) | Python/C | GPL-2.0 | `lib/python/qtvcp/widgets/probe_routines.py`, `versa_probe.py`, `basic_probe.py`; ~40 NGC macros in `nc_files/probe/basic_probe/macros/` incl. calibration (`probe_cal_round_pocket` …). | Most complete routine catalogue + parameter set; don't forget calibration. |
| [Probe Basic](https://github.com/kcjengr/probe_basic) / [QtPyVCP](https://github.com/kcjengr/qtpyvcp) | Python | GPL-3.0 / mixed | Probe screens: corners, edge angle, boss/pocket, ridge/valley. | UI layout: icon grid of routines + shared params (feed, latch, clearance, step-off). |

## 2. Probing routines & math

Routine set (union of LinuxCNC basic_probe, Probe Basic, UGS):

- Z surface; edges ±X/±Y; inside/outside corners.
- Edge angle: two touches on one edge, `atan2(dy, dx)` → WCS rotation.
- Bore/pocket and boss center: 4-point (X midpoint then Y) or 3-point circumcircle (true center + diameter).
- Ridge/valley (web/slot) center.
- Calibration on ring gauge / gauge block → effective stylus radius, XY offset/runout.
- 4th-axis centerline: probe top Z at A=0/A=180 or both sides in Y; axis at midpoint
  ([forum macro reference](https://forum.sienci.com/t/macros-for-3d-probing/13806), forum quality).
- Haas/Renishaw (G65 P9810/P9811/…) manuals: good naming/parameter conventions (not fetched).

Surface mapping (bCNC, UGS, Candle, ioSender, cncjs-kt-ext all do this): probe N×M grid → bilinear
interpolation → split G1 to ≤ grid spacing (linearize arcs) → add Z offset. Implement as a pure
transform over the parsed program in Rust; persist meshes.

## 3. Parametric / conversational CAM

| Item | Licence | Borrow |
|---|---|---|
| LinuxCNC NGCGUI | GPL-2.0 | Subroutine + declared param list (`#<name> = #1 (=default)`) → auto form. Maps onto snippets-with-variables. |
| bCNC plugins | GPL-2.0 | Drill/peck, pocket, helical descent, trochoidal. |
| UGS Designer | GPL-3.0 | Pocket / facing implementations. |
| gSender Surfacing/Rotary | GPL-3.0 | Facing + 4th-axis surfacing UX. |
| [FreeCAD CAM](https://github.com/FreeCAD/FreeCAD) (`src/Mod/CAM`) | LGPL-2.1 | ThreadMilling + Helix ops (climb/conventional, int/ext, multi-pass, lead-in arcs). Most rigorous thread-milling reference. |
| [Kiri:Moto](https://github.com/GridSpace/grid-apps) | MIT | Browser CAM incl. drilling, 4th-axis "indexed". Portable. |
| [OpenCAMLib](https://github.com/aewallin/opencamlib) | LGPL-2.1 | Drop/push cutter, waterline — likely beyond "lite". |
| [hronn](https://docs.rs/hronn) | AGPL-3.0 | Avoid copying. |
| hsm-nibble (PyPI) | unchecked | Adaptive pocketing reference, later. |
| Estlcam | proprietary | UX only. |

**Rust geometry**
- [cavalier_contours](https://github.com/jbuckmccready/cavalier_contours) (Apache-2.0) — arc-preserving polyline offsets → G2/G3 pockets. Top pick.
- clipper2: [clipper2-rust](https://github.com/larsbrubaker/clipper2-rust) (BSL-1.0, pure Rust, very new) or [clipper2](https://github.com/tirithen/clipper2) (Apache-2.0, C++ wrapper) — polygon booleans.
- [geo](https://github.com/georust/geo) (MIT/Apache) — general 2D.

**G-code crates**
- `gcode` (Michael-F-Bryan/gcode-rs, Apache-2.0) — zero-alloc parser.
- `g-code` (sameer/g-code, MIT/Apache) — PEG parser + emitter; limited coverage.
- Likely: `gcode` for parsing, own typed emitter for generator output.

**3D preview** — no mature r3f CNC viewer exists. References: cncjs visualizer (MIT), [gcode-preview](https://github.com/xyz-tools/gcode-preview) (MIT, printer-oriented), CAMotics (GPL, licence version unverified). Plan: r3f `BufferGeometry` line segments coloured by motion type, from points tessellated in Rust.

## 4. Rust ecosystem

- [serialport](https://github.com/serialport/serialport-rs) (MPL-2.0, fine as dep) + tokio-serial (MIT).
- [mdns-sd](https://github.com/keepsimple1/mdns-sd) (Apache-2.0); Makera-style UDP broadcast needs only `tokio::net::UdpSocket` + `set_broadcast(true)`.
- [tauri-specta](https://github.com/specta-rs/tauri-specta) (MIT) — commands + events; still `2.0.0-rc.25` (May 2026), pin it. [ts-rs](https://github.com/Aleph-Alpha/ts-rs) (MIT) — types only.
- [json-patch](https://github.com/idubrov/json-patch) (Apache-2.0) — `diff(&Value, &Value) -> Patch` (RFC 6902). Needs a small converter to immer patches (JSON Pointer strings → path arrays; immer ops are add/replace/remove only). Array diffs are index-based, so keep high-frequency status out of big arrays / on a separate event channel.

## Unverified

ioSender's exact probing feature list; versa_probe lineage; CAMotics licence version; gSender native
probing set today.
