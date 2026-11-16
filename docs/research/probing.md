# Probing on the Z1

Notes on using the 3D probe with plain G-code, beyond what Makera Studio offers. The routines below
are the user's own, verified on their Z1 (stock firmware). They assume a probe with a 2 mm ball
(1 mm radius) and that the probe is the active tool.

## Building blocks

- `G38.2` — probe toward the workpiece, error if no contact. `G38.4` — move away until the probe
  *releases*. Both work on the Z1.
- Each probe move reports `[PRB:x,y,z:1]` in machine coordinates (`1` = triggered;
  `SimpleShell.cpp:1481` prints `%1.4f`). A `G38.2` without contact reports `ALARM: Probe fail`
  and halts (`H:3`).
- `G10 L20 P0 <axis><value>` sets the *current* WCS (`P0`) so that the current position reads
  `<value>` (`Robot.cpp:568`).
- Setting Z through `G10` makes the current tool the length reference: `ref_tool_mz` becomes the
  current tool's last tool-sensor reading and the TLO is zeroed (`ATCHandler.cpp:3208`,
  `Robot::clearToolOffset`). Since every tool change (probe included) is measured on the sensor,
  Z0 stays valid across tool changes regardless of which tool set it. Consequences for the app:
  the `T:` offset is relative to the reference tool (it reads 0 right after zeroing Z), not an
  absolute tool length; and zeroing Z with a tool that wasn't measured since it was inserted
  (e.g. set with `M493.2` only) would give a wrong reference.

## Shoulder touch-off in -X, setting the edge to X0

Two-pass: fast touch, back off until release, 1 mm clearance, slow touch. At contact the ball
centre is one radius (+1 mm) from the shoulder, hence `X1.000`.

```gcode
G21
G91

; First touch, moving toward -X
G38.2 X-10 F100

; Retract until probe releases
G38.4 X2 F50

; Add 1 mm clearance
G0 X1

; Slow second touch
G38.2 X-2 F20

; At contact, probe centre is +1 mm from the shoulder
G90
G10 L20 P0 X1.000

; Safely release probe
G91
G38.4 X2 F20

; Extra clearance
G0 X1

G90
```

## Top surface touch-off, setting it to Z0

```gcode
G21
G91

; First touch downward
G38.2 Z-10 F100

; Retract until probe releases
G38.4 Z2 F50

; Add 1 mm clearance
G0 Z1

; Slow second touch
G38.2 Z-2 F20

; Make the touched top surface Z0
G90
G10 L20 P0 Z0.000

; Release probe safely
G91
G38.4 Z2 F20

; Extra clearance
G0 Z1

G90
```

## Verified on hardware: tailstock centre in Y (2026-10-27)

`hj probe-cylinder-y` (host-orchestrated, read-only) on the 4th-axis tailstock, wired probe
**T9999** (TLO -0.023), 2 mm ball assumed. Capture:
[captures/z1-probe-tailstock-y.jsonl](captures/z1-probe-tailstock-y.jsonl).

- **`G38.x` is blocking:** the firmware waits for the queue, runs the probe move, waits again,
  prints `[PRB:…]` and only then `ok` (`ZProbe::probe_XYZ`). The `ok` arrives right after the PRB
  line, e.g. 12.5 s for a 21 mm approach at F100. `G38.4` reports `[PRB:…:1]` when the probe
  *releases*.
- **`G38.x` distances are always relative**, even under `G90` (delta move), so routines don't need
  `G91` at all. Positioning with `G53 G1 … F…` (absolute machine) worked as expected.
- PRB coordinates have 3 decimals (`%1.3f`), unlike the 4 in status reports.
- Results: contacts at MY -106.685 and -143.491 → **centre MY -125.088** (WY -0.131), width at
  8 mm below the top 34.806 mm (ball-compensated). Top MZ -17.078 at the centre, -17.072 at the
  start 4 mm off-centre, so the top is flat (within ~6 µm), not round; the routine's "if round"
  diameter/axis numbers don't apply to this part.
- Repeatability: fast vs slow touch differed by 1–3 µm (-17.075/-17.072, -106.686/-106.685,
  -143.489/-143.491).
- Time: ~84 s for 4 two-pass touches with 35 mm side offsets.

## Switching to the probe with `M6 T9999` (2026-10-27)

Question: does the manual tool change alone make the wired 3D probe usable, or does Makera Studio
do something extra? Tested by changing T1 → T9999 with `M6` from `hj`, then touching the ball by
hand while polling `M119` (`Probe: 0/1`) twice a second.

- **Firmware 1.1.2: no.** The tool change measured the probe like any other tool (no probe-specific
  steps) and ended with `M493.2 T9999` + `M494.1`. The probe LED turned blue, diagnose `T:` went to
  1, but hand touches never registered: no LED reaction, `Probe: 0` and diagnose `P:0,0` throughout
  (capture: [captures/z1-probe-tool-change-1.1.2.jsonl](captures/z1-probe-tool-change-1.1.2.jsonl)).
  The 1.1.2 source only treats T0 as a probe in `fill_cali_scripts`.
- **Firmware 1.1.3: yes.** The same change runs the probe sequence: `M494.1` (probe on) before
  calibrating, the slow sensor touch as `G38.6 Z-2 F100 P1`, then `M492.3`, `M494.2`,
  `M493.2 T9999`, `M494.1`, `Done ATC`. Afterwards hand touches show up as `Probe: 1` (12 of 12
  in the test) and the LED reacts (capture:
  [captures/z1-probe-tool-change-1.1.3.jsonl](captures/z1-probe-tool-change-1.1.3.jsonl)).
- After the upgrade and a power cycle with T9999 still active (it persists in EEPROM), the probe
  also worked without any tool change.
- The 1.1.3 probe calibration reads ~0.026 mm higher on the tool sensor than 1.1.2 did
  (MZ -69.550 vs -69.576), so the probe's TLO changed from -0.025 to +0.002. Offsets zeroed with the
  probe under 1.1.2 are off by that much.
- For the app: require firmware ≥ 1.1.3 for probing, switch to the probe with a plain
  `M6 T9999`, and check `M119` (or a known contact) before trusting it.

## Setup tools in Makera Studio

Studio's 3D probing dialog offers three "setup tools":

- **3D Probe** — the touch-trigger probe (ball stylus) used above; tool T9999.
- **3D Probe Rod** — a solid 1/8"-ish steel rod through a moulded block with a 2-wire cable
  (4-pin connector, 2 pins populated). **Conductive**: Studio warns that "the base plate must be
  made of a conductive material". No moving parts, so it only works on conductive stock/fixtures
  connected to the machine. Tool number not yet known (candidates in the firmware: T0, T8888).
- **Manual Tool Setting** — no probe; touch-off by hand.

Hobby Jogger needs to know which of these is active: the rod has no ball radius to compensate in
the same way (rod radius instead), can't probe non-conductive parts, and needs its own guidance.

## How this maps onto Hobby Jogger

- The same sequence generalises to all six directions and to corners, bores, bosses and edge angle.
  The Rust side can run it line by line, read each `[PRB:…]` and compute offsets itself
  (radius compensation, centre finding, rotation) instead of relying on `G10` at the machine.
- Parameters to expose: approach distance, fast/slow feed, retract, clearance, ball radius
  (ideally calibrated on a ring gauge or known-width block).
- Every routine must end in `G90` even when a step fails; on `ALARM: Probe fail` the app has to
  clear the halt and restore modal state (`G90`), since the program stops mid-way in `G91`.
- The 3D probe is tool T9999 on this machine (selected via Makera Studio's probe switch).
- Ball radius is still nominal (2 mm ball); a calibration routine should measure it.

## In the app (2026-10-27)

Hobby Jogger's probing routines (surface, edge, corner, bore, boss; `crates/hj-core/src/probe.rs`)
use only probe moves: `G38.3` for searches and positioning (checking the `[PRB:…:flag]`), `G38.5`
for releases. Developed against the simulator, which models a block with a bore and a boss.

On the Z1 (1.1.3, 2026-10-27): **`G38.3` reports `[PRB:…:0]` then `ok` when it ends without
contact, and `[PRB:…:1]` then `ok` when it stops on one** (e.g. an edge routine coming down onto
the top because the edge was further than its reach). After a contact the probe's LED stays green
for a few seconds even once it's clear; neither `$X` nor switching the probe off and on
(`M494.2`/`M494.1`, which is all `M494` does on manual-change machines) changes that, and it
clears by itself. Routines now back out to where the failing move started.

The firmware also has a **motion probe guard** (`ZProbe::read_probe`): with T9999 (or T0) active,
if the probe reads triggered during a non-probe move in X/Y or up in Z, it stops the motors and
sets a halt.

## Depth probe, T0 (firmware source, not yet verified on the Z1)

The firmware treats T0 as a probe alongside T9999: a manual change to it runs the probe
calibration (`fill_cali_scripts(is_probe)`, slow touch with `G38.6 … P1`), the motion probe guard
covers it, and both read the same probe input. Unlike T9999, the change ends with `M494.2`
(detector switch off), and the firmware's own Z probing (`M495 … O`, `fill_zprobe_scripts`) wraps
its `G38.2` touches in `M494.1` / `M494.2`. It's a mechanical switch that only touches down, so
the app offers Surface with it and switches it on the same way. To check on the machine: that the
depth probe really is T0, and that it triggers with `M494.1` sent (and not without).

## Probe state, chatter and `G38.4`/`G38.5` (firmware source, confirmed on the machine 2026-11)

- `G38.x` checks the probe before moving (`ZProbe::probe_XYZ`): `G38.2`/`G38.3` need it free and
  `G38.4`/`G38.5` need it triggered. Otherwise it prints `Error:ZProbe triggered before move,
  aborting command.` (the same text either way, so misleading for `.4`/`.5`) and **halts**
  (`PROBE_FAIL`).
- The probe input isn't debounced by default (`zprobe.debounce_ms` 0). Right where the probe lets
  go after `G38.5`, its contact chatters: a guarded `G38.3` lift started there stopped after 8 µm
  as triggered. Hobby Jogger moves a further 0.5 mm with a plain move after every release, back
  the way the probe came.
- `M119` ends its line with `Probe: 0|1`: the probe's state now. Hobby Jogger reads it before each
  `G38.5`, and Recover does nothing more when the probe has already let go.
