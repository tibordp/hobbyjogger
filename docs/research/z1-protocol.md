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

## Open questions (hands-on)

1. Wireshark Makera Studio: compare UDP 3333, TCP 2222, :80, :82 against controller code.
2. USB descriptors (`ioreg -p IOUSB -l` / `lsusb -v`): CDC-ACM or vendor bulk? Baud?
3. Does the ESP32 pass `echo` / `?` through or answer itself? Max status poll rate?
4. Upload/play over WiFi vs USB: block size, throughput, md5 placeholder handling; is streaming
   without upload possible?
5. HTTP API surface on :80.
6. M6 manual tool-change handshake (state reported, how user confirms, TLO sequence); laser
   offset (G92.5); 4th-axis homing.
7. G38.x reliability on v1.1.3 (issues [#4](https://github.com/MakeraInc/MakeraZ1Firmware/issues/4),
   [#5](https://github.com/MakeraInc/MakeraZ1Firmware/issues/5)) — user reports their probe works fine.
8. Support Community Firmware alongside stock?

Local clones used for this research (scratchpad, not in repo): official Z1 firmware, official
Carvera Controller, community controller, official Carvera firmware.
