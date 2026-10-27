# Makera Z1: how software talks to it

Research snapshot as of 2026-10-26. Sections are split into **confirmed** (with source),
**strongly implied**, and **unknown**. Nothing here has been verified against a real machine yet —
see "Open questions" at the bottom.

## Architecture summary

The Z1 is two boards:

- **Motion board** — LPC1768 running a Smoothieware fork (open source, GPL-3.0). Shares a codebase
  with Carvera / Carvera Air firmware.
- **Mainboard** — ESP32 with **closed-source** firmware. Owns WiFi, USB, the TF/SD card,
  `config.txt`, and the camera. The host app talks to the ESP32, which relays to the LPC over
  UART2 @ 230400 (DMA).

Practical consequence: the protocol we speak is whatever the ESP32 exposes, which in turn mostly
mirrors the Carvera framed protocol. The firmware being open doesn't make the ESP32 behaviour a
stable contract.

## Confirmed

### Firmware (motion board)

- [MakeraInc/MakeraZ1Firmware](https://github.com/MakeraInc/MakeraZ1Firmware), GPL-3.0. README:
  "a branch of Smoothieware … for the Makera Z1/Z1Pro Motion Control Board".
- MCU LPC1768 (`build/common.mk`). `Kernel.h` model IDs: `CARVERA=1, CARVERA_AIR=2, MAKERA_Z1=3, MAKERA_Z1Pro=4`.
- Releases [v1.1.2 (2026-08-11), v1.1.3 (2026-10-21)](https://github.com/MakeraInc/MakeraZ1Firmware/releases).
  Changelogs in `version.txt` (probe calibration, 3D-probe crash protection, manual tool-change
  interrupt handling, fan control).
- Update: copy `lpc1768.bin` to TF card root, power cycle (~2 min).
- In `src/main.cpp`, LPC USB serial / MSD / WifiProvider are commented out; only UART2 to the
  "Mainboard" is used (`Kernel.cpp:147`, `SerialConsole_DMA.h`).
- ESP32 firmware is closed ([issue #2](https://github.com/MakeraInc/MakeraZ1Firmware/issues/2)).
  `config.txt` lives on the ESP32's SD and is streamed to the LPC at boot
  ([issue #1](https://github.com/MakeraInc/MakeraZ1Firmware/issues/1)).
- Job playback: the LPC receives G-code lines as `PTYPE_PLAY_DATA` packets (`SerialConsole.cpp`,
  [issue #8](https://github.com/MakeraInc/MakeraZ1Firmware/issues/8)) — most likely fed by the ESP32
  from its SD card.

### Wire protocol (host ↔ ESP32)

Sources: official `CarveraController/src/Controller.py`, `XMODEM.py`, `WIFIStream.py`; community
`carveracontroller/protocols/framing.py`, `protocols/detector.py`.

- **Frame:** `0x8668 | len:u16be | type:u8 | data | crc16-ccitt:u16be | 0x55AA`.
  `len` = type + data + CRC; CRC covers len + type + data.
- **Frame types:**
  - `0xA1` realtime char (`?`, `!`, `~`, Ctrl-X)
  - `0xA2` command line
  - `0xB0`–`0xB6` file transfer: start, md5, view, data, end, cancel, retry
  - `0x81` status response, `0x82` diagnostics, `0x83`/`0x84`/`0x85` load info/finish/error,
    `0x90` info text, `0x71` firmware version
- **Protocol detection:** send plain `echo echo\n` — plaintext echo back ⇒ legacy Smoothie text
  protocol; silence ⇒ framed.
- **WiFi:** TCP **2222** for commands. Discovery: listen for UDP broadcast on **3333**, payload
  `name,ip,port,busy`. Community reports Z1 protocol "seems to be the same"
  ([Carvera_Controller#692](https://github.com/Carvera-Community/Carvera_Controller/issues/692)).
- **USB:** on the Z1 this is **not a serial port** — it's a vendor-class bulk device,
  VID `0x303A` / PID `0x4002` (Espressif), driven via libusb (community controller
  `develop:carveracontroller/USBBulkStream.py`, PR #754). Windows needs WinUSB, Linux a udev rule.
  (C1/CA1 use FTDI serial @ 115200.) See `carvera-reference.md`.
- **Camera:** WebSocket `ws://<ip>:82/ws_video`; send `start_stream`; each binary message is one
  JPEG (640×480 default). Resolution: `POST http://<ip>:80/api/camera/resolution {"resolution":10..15}`
  (Espressif framesize). No exposure/gain (`addons/camera/Z1Camera.py` in community controller).
- **Files:** `upload`, `play <file>` (spaces encoded as `\x01`), `ls`, `cat`, `rm`, `mv`, `mkdir`.
  Stock Z1 answers `md5sum` with placeholder `default_md5_hash_value_32_bytes_`.
- **Status line** (`Kernel::get_query_string`):
  `<State|MPos:x,y,z,a,b|WPos:…|F:…|S:rpm,target,factor,vac|T:tool,offset[,target]|W:…|L:mode,state,testing,power,scale|P:lines,pct,secs|A:atc|O:…|H:halt|C:model,func,inch,abs|RSSI:…>`.
  `C:` model = 3 (Z1) / 4 (Z1 Pro).

### G/M-codes

Community reference: [gggcodes.com/z1/gcodes.html](https://www.gggcodes.com/z1/gcodes.html),
partially cross-checked against source.

| Area | Codes | Source |
|---|---|---|
| Probing | G38.2–G38.5, G38.6 (Z calibration), G30, M670 | `ZProbe.cpp` |
| Leveling | G29/G31/G32; M370/M374/M375/M561 grid mgmt | |
| Tool change | M6 Tn (manual QTC flow), M493.2 set tool w/o motion, M491 TLO recalibrate, M480.x corner/pocket probing, M490–M499 | `ATCHandler.cpp` |
| Laser | M321/M322 enter/exit, M323/M324 test fire, M325 power scale | |
| 4th axis | A in normal moves/offsets; G92.4 A homing / rotation shrink | |
| Misc | M115 fw info; `M1000 <shell cmd>` | |

### Ecosystem

- [MakeraInc/CarveraController](https://github.com/MakeraInc/CarveraController) (GPL-3.0, Python/Kivy) — maps model 3 → "Z1".
- [Carvera-Community/Carvera_Controller](https://github.com/Carvera-Community/Carvera_Controller) — supports Z1 incl. camera.
- [Community firmware PR #437](https://github.com/Carvera-Community/Carvera_Community_Firmware/pull/437) (merged 2026-09-15) adds Z1/Z1 Pro builds.
- [MakeraInc/MakeraCAM](https://github.com/MakeraInc/MakeraCAM) — binaries only, no licence.
- Makera Studio (CAM + control, Windows/macOS) launched 2026-08-06; Makera App (iOS/Android) for
  status + camera. WiFi provisioning requires USB-C.

## Strongly implied

- Makera Studio uses the same framed protocol on TCP 2222; the ESP32 translates/forwards rather
  than exposing the LPC raw. ESP32 upload responses differ from LPC ones
  ([Carvera_Controller#811](https://github.com/Carvera-Community/Carvera_Controller/issues/811)).
- Grbl mode off on stock; Smoothie dialect, IJK-only arcs.
- ESP32 runs an HTTP server on :80 (web UI / API).

## Unknown

- Makera Studio's tech stack and exact traffic.
- ESP32 variant, USB VID/PID, CDC-ACM vs vendor bulk on macOS/Linux.
- Full HTTP API on :80; auth.
- BLE/blufi provisioning.
- Any GRBL-style sender (LightBurn, gSender, cncjs, UGS) working with Z1 — unlikely given framing.

## Verified on hardware (2026-10-27)

Z1 `Makera_Z1_010967`, firmware `version = 1.1.2.0.1.13`, macOS host, using `hj`. Raw captures:
[captures/z1-wifi.jsonl](captures/z1-wifi.jsonl), [captures/z1-usb.jsonl](captures/z1-usb.jsonl).

- **Framed protocol confirmed on both transports.** Same frames, same replies over TCP 2222 and USB.
- **Discovery** broadcasts roughly once a second, `name,ip,port,busy,state` — the fifth field is the
  **machine state** (`Idle`, `Home`, ...), not a version.
- **USB**: `303a:4002`, manufacturer "Espressif", product "MakeraZ1 (USB)", serial `123456`,
  full speed. One configuration, interface 0 class `0xff` (vendor) with bulk OUT `0x01` and bulk
  IN `0x81`, 64-byte max packet. **macOS opens it without any driver or prior wake-up bytes.**
- **Latency** (first reply to `?`): ~17 ms over USB, ~107 ms over WiFi.
- **No `ok` after console commands.** `version`, `model`, `ftype` and `diagnose` reply with their
  output only. Command completion can't be detected by waiting for `ok` in general.
- `version = 1.1.2.0.1.13` — the LPC firmware's own `version` command is commented out, so this is
  answered by the ESP32: **LPC firmware version + ESP version** (confirmed: after flashing LPC
  1.1.3 it reads `1.1.3.0.1.13`). The machine was upgraded to 1.1.3 on 2026-10-27; the
  observations above were made on 1.1.2 unless noted. 1.1.3 changes probe handling — see
  `probing.md`.
- `ftype = nc` — **no QuickLZ (`lz`) upload compression** on stock Z1.
- `model = Z1, 3, 1, 0, Idle` — `model, MachineModel, FuncSetting, probe_addr, state`
  (`SimpleShell.cpp:1030`). FuncSetting = 1 (bit 2 / ATC clear → manual tool change).
  `probe_addr` 0.
- **Status** (idle, homed): `T:3,-18.211,-1` (tool 3, TLO, target -1 = no change pending); no `W:`
  field (probe voltage) in this capture; `C:3,1,0,1`; Z1 extras `E:0,0,0,25,7613` and `OTA:0,0`.
  After homing MPos is `-1,-1,-1`.
- **Diagnose**: `{S:0,10000,0,0,21,21|L:0,0|V:1,24|F:0,0|G:1,0,0,0,0|T:0|C:1|E:0,0,0,0,0,1,1,0|P:0,0|I:0|RSSI:-27}`
  — `RSSI` is present even over USB.

### Motion, hold and halt (2026-10-27, WiFi, Z untouched, A axis attached)

- **`$J` is a relative move.** `$J X-10 [Y..] [A..] [F0.1]`. `F` is a *fraction of the axis max
  rate* (default 1.0 = full speed), not a feed rate (`SimpleShell::jog`). F0.1 moved X/Y at
  120 mm/min, so the X/Y jog max is ~1200 mm/min. A10 at F0.1 took ~3.4 s (~1800°/min at full rate).
- **`$J` gets no reply at all** — no `ok`, no error. Jogs sent while moving are queued and run
  back to back. There is no way to cancel just the jog in progress.
- **G-code lines get `ok` as soon as they're queued** (~80–100 ms), long before the move finishes.
  Completion has to come from the status going back to `Idle`.
- **Feed hold (`!`) does not stop a move in progress.** State changes to `Hold`, but the firmware
  only blocks *new* blocks from being queued (`Robot.cpp:1715`). A single 20 mm jog ran to
  completion. Worse, in `Hold` the status reports the *planned end position*, not the actual one
  (`get_query_string` only reports live positions in `Run`/`Home`). `~` returns to `Run`.
  → Not usable as a stop. Pausing jobs should use `suspend`/`resume`.
- **Halt (Ctrl-X) stops immediately**, mid-move included, and keeps the position (X stopped at
  -6.905 out of a -21 move, consistent with the timing). Reply `ALARM: Abort during cycle`, state
  `Alarm` with `H:1` (manual). `$X` replies `[Caution: Unlocked]` + `ok` and returns to `Idle`.
  No re-homing needed.
- **Status positions update about every 300 ms** while moving, regardless of poll rate: polling
  at 100 ms returns each position ~3 times.
- Side effect of testing: the modal feed rate is now 600 (`F:…,600.0,…`, was 2000) after a
  `G1 … F600`. Harmless for jobs that set F.

### Z, spindle, upload and job control (2026-10-27, WiFi)

- **Z jog** max rate ~600 mm/min (F0.1 → 60 mm/min).
- **Travel / soft limits** (config `soft_endstop.*_min`): X ≥ -207, Y ≥ -206, Z ≥ -102; the max is
  -1 on all three (hard-coded in `Robot.cpp`). **With the 4th axis on, the Z1 firmware uses Y ≥ -160
  instead** — hard-coded in the soft-limit check (`Robot.cpp:1497`), swapped in only during the
  check, so neither `config-get` nor `M211` shows it. Whether A is "on" is decided during homing
  (`Endstops::home` probes the A endstop) and isn't reported; the closest host-visible signal is
  FuncSetting bit 0 (A homing enabled) in the status `C:` field — but that bit **stays set with the
  4th axis unplugged** (verified 2026-10-27), so it can't tell. Hobby Jogger uses the configured
  limits and lets the firmware halt if Y goes below -160 with the axis attached. Homing also does Y
  before X when A is on (`Endstops::home_xy`).
- **Soft limits sit exactly at the homed position: MPos -1.0 on X, Y and Z** (max side). A move
  targeting -0.9997 trips `error:Soft Endstop Z was exceeded - reset or $X or M999 required`, and
  even `$J Z2.9997` from -3.9997 tripped it through float rounding. The check runs when a line is
  *queued*, so the halt cuts off motion that is still queued ahead of it. → Never target the
  limit exactly; keep a margin (we use 0.01 mm).
- **Spindle `M3 S5000`**: takes ~5 s to reach speed and overshoots (~5700 rpm, then settles at 5000).
  The machine reports `Run` while the spindle is on, even with no motion. M3 needs an active tool
  (`T>0`), otherwise it halts with `ERROR: No tool or probe tool!`. The target rpm stays in `S:`
  after `M5`.
- **⚠ An `M5` sent while `M3` is still spinning up is silently ignored** and still gets `ok`.
  `SpindleControl.cpp` drops M3/M5 while another is in progress (`b_on_offing`), and incoming
  commands are processed re-entrantly during M3's wait. The spindle kept running until a second
  `M5`. → After `M5`, verify `S:` current rpm / diagnose `S:0,…` and resend (or halt).
- **Upload** works as documented (`FILE_START` + `FILE_MD5`, machine-pulled `FILE_VIEW`/`FILE_DATA`,
  `FILE_END`). A 3.6 KB file took 0.27 s over WiFi.
- **`md5sum <file>` returns the real digest** on 1.1.2 (not the placeholder), formatted
  `<digest><path>` with no separator.
- **`ls -s <dir>` replies in `LOAD_INFO` (0x83) frames**, one entry per frame: `name size YYYYMMDDhhmmss`.
  Directories end in `/`. New files are dated 1980-01-01 (clock not set; there is a `time <unix>`
  command).
- **`play <file>`** → `  File size N`. Status gets `P:lines,percent,secs`. The job's `G4 S2` plus
  M3 spin-up took ~10 s before motion started.
- **`suspend` waits for the planner queue to drain** (~3.2 s with 1 mm segments at 600 mm/min), in
  state `Wait`, then `Pause`. While in `Wait` the status reports the end of the queue, not the live
  position. Messages: `Suspending , waiting for queue to empty...`, `now save current pos...`,
  `Suspended, resume to continue playing`. The spindle kept running while paused.
- **`resume`** → `Resuming playing...`, `Restoring saved XYZ positions and state...`,
  `Playing file resumed`. It restores the position with `G1 X%.3f Y%.3f Z%.3f F1000` in WCS
  (`Player::resume_command`), i.e. **rounded to 0.001 mm**. With a 4-decimal WCS offset that left Z
  0.0003 mm off, which is what then tripped the soft limit above.
- **`abort` also waits for the queue to drain** (~3.8 s of motion kept running), then runs M5 and
  replies `ok` + `Aborted playing or paused file.` about 7 s after the command. **Modal state is
  not reset**: a job that used `G91` leaves the machine in `G91`.
- Halting a job (soft-limit trip) stopped the spindle and printed `Aborted by halt`.

### Manual tool change (2026-10-27, `M6 T2` from T3)

Capture: [captures/z1-manual-tool-change.jsonl](captures/z1-manual-tool-change.jsonl). Flow from
`ATCHandler.cpp` (`fill_change_scripts`, `fill_cali_scripts`) matched what the machine did.

- `M6 T2` → `Please change the tool to: T2`. **The firmware echoes every line of its internal
  script as a console line, each followed by its own `ok`**: `G53 G0 Z-3.000` (clearance_z),
  `G53 G0 X-9.600 Y-13.300` (anchor1 + toolrack offset + 132), `M497.2`, `M490.1`.
- `M490.1` enters the **`Tool` state** (beeps). The status has `T:3,-18.211,2`, i.e. the target
  tool in the third field. Reached ~6 s after `M6`.
- **Confirmation is `M490.2`** (exit tool-waiting). Then it runs `M493.2 T-1` (tool = none until
  calibrated), `M497.3`, `G53 G0 Z-3`, `G53 G0 X-9.400 Y-13.300` (tool sensor, anchor1 + 181),
  `G38.6 Z-108 F500` → `[PRB:-9.400,-13.300,-82.154:1]`, `G91 G0 Z1`, `G38.6 Z-2 F100` (relative)
  → `[PRB:-9.400,-13.300,-82.138:1]`, `M493.1` (save TLO), `G53 G0 Z-20` (safe Z; not the
  -10 default the source suggests), `M493.2 T2`, `M494.2`, then **`Done ATC`**. It then returns XY
  to the starting point and leaves Z at clearance (-3). Total ~20 s after the confirmation.
- **Probe results are `[PRB:x,y,z:1]`** (1 = triggered), in machine coordinates.
- New TLO for T2: -12.586 (T3 was -18.211). The WPos Z shifts accordingly.
- **The state briefly reads `Idle` mid-routine** (between the confirmation and the calibration,
  and between the probe and the lift). `Idle` alone doesn't mean the tool change is finished.
  Wait for `Done ATC` and/or `T:<new>,…` with the target matching.
- No `A:` (ATC state) field is reported on the Z1: it's only emitted when FuncSetting has the ATC
  bit.

Design consequences: a "stop jog" button must be halt + `$X`; continuous jogging means streaming
short `$J` increments; a job pause button must use `suspend`, not `!`; the UI should never show
positions from a `Hold` or `Wait` status as the live position. The only immediate stop is halt;
`suspend` and `abort` act seconds later. Spindle stop must be verified. Generated programs should
stay clear of the soft limits and end with explicit `G90`; after an abort the app should restore
modal state itself.

## Open questions (hands-on)

1. Wireshark Makera Studio: compare UDP 3333, TCP 2222, :80, :82 against controller code.
2. ~~USB descriptors~~ — answered above: vendor bulk, no driver needed on macOS. Linux udev rule still untested.
3. Does the ESP32 pass `echo` / `?` through or answer itself? Max status poll rate?
4. Upload/play over WiFi vs USB: block size, throughput, md5 placeholder handling; is streaming
   without upload possible? (Compression is out: `ftype = nc`.)
5. HTTP API surface on :80.
6. M6 manual tool-change handshake (state reported, how user confirms, TLO sequence); laser
   offset (G92.5); 4th-axis homing.
7. G38.x reliability on v1.1.3 (issues [#4](https://github.com/MakeraInc/MakeraZ1Firmware/issues/4),
   [#5](https://github.com/MakeraInc/MakeraZ1Firmware/issues/5)) — user reports their probe works fine.
8. Support Community Firmware alongside stock?

Local clones used for this research (scratchpad, not in repo): official Z1 firmware, official
Carvera Controller, community controller, official Carvera firmware.
