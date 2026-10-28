# Carvera / Makera open-source protocol reference

Protocol details extracted from the Makera / Carvera open-source ecosystem, read from shallow clones
on 2026-10-26. Paths are relative to each repo. **[C]** = confirmed in source or official text,
**[I]** = inferred. All GPL code here is a *specification* for us — reimplement, don't copy.
OpenSpindle and carve-control are MIT.

## Repository catalogue

| Repo | Licence | Notes |
|---|---|---|
| [MakeraInc/CarveraFirmware](https://github.com/MakeraInc/CarveraFirmware) | GPL-3.0 | Smoothie fork for C1/CA1, v1.0.8 |
| [MakeraInc/MakeraZ1Firmware](https://github.com/MakeraInc/MakeraZ1Firmware) | GPL-3.0 | Z1 LPC firmware, v1.1.3 |
| [MakeraInc/CarveraController](https://github.com/MakeraInc/CarveraController) | GPL-3.0 | Official Python/Kivy app; framed protocol in `src/Controller.py` |
| [MakeraInc/CarveraProfiles](https://github.com/MakeraInc/CarveraProfiles) | none | Fusion `Makera Z1 3 axis.f3d` / `4 axis.f3d`, `Carvera.cps` post, FreeCAD `Makera_Z1.fcm` |
| [Carvera-Community/Carvera_Controller](https://github.com/Carvera-Community/Carvera_Controller) | GPL-2.0 | Most complete host reference. Z1 USB + updater on `develop` |
| [Carvera-Community/Carvera_Community_Firmware](https://github.com/Carvera-Community/Carvera_Community_Firmware) | GPL-3.0 | Both protocols (M485 switch); Z1 builds since PR #437 |
| Carvera-Community docs | none | https://carvera-community.gitbook.io/docs |
| [openspindle/openspindle](https://github.com/openspindle/openspindle) | **MIT** | Electron/TS Z1 controller, early alpha. **Closest prior art to Hobby Jogger** |
| [GridSpace/carve-control](https://github.com/GridSpace/carve-control) | **MIT** | Node web controller, proxy, UDP spoofer |
| [hagmonk/carvera-cli](https://github.com/hagmonk/carvera-cli) | none | Python CLI (upload, ls, md5, config) |
| [AngryApostrophe/Clout](https://github.com/AngryApostrophe/Clout) | GPL-3.0 | — |
| [f355/carvera_firmware_simulator](https://github.com/f355/carvera_firmware_simulator) | GPL-3.0 | Community firmware built as a host executable, fake WiFi on :2222, ships `default_sdcard/z1/config.txt`. **Useful as a dev/test target** |
| [blackveilprecision/awesome-makera](https://github.com/blackveilprecision/awesome-makera) | CC0 | Index |

## Transport & discovery

- **WiFi:** TCP **2222** (`CarveraFirmware/src/modules/utils/wifi/WifiProvider.cpp:112`). **[C]**
- **Discovery:** machine broadcasts UDP **3333**, ASCII `name,ip,tcp_port,busy` (busy=1 when a
  client is connected) (`WifiProvider.cpp:398,422`). **[C]** OpenSpindle accepts an optional 5th
  `version` field for Z1 (`src/machine/firmware/makera/adapter.ts:204`). **[I]**
- **USB, C1/CA1:** FTDI serial @ 115200; community fw supports `baud <rate>`. **[C]**
- **USB, Z1:** *not serial*. Vendor-class bulk device **VID 0x303A / PID 0x4002** (Espressif),
  via libusb (community `develop:carveracontroller/USBBulkStream.py`, PR #754). Windows needs WinUSB
  (Zadig), Linux needs a udev rule. Serial string is always `123456`. **[C]**
  → In Rust: `nusb` or `rusb`, not `serialport`.
- **Z1 camera (ESP32):** `ws://<ip>:82/ws_video`, send `start_stream`, one JPEG per binary message;
  `POST http://<ip>:80/api/camera/resolution {"resolution":10..15}` (`addons/camera/Z1Camera.py`). **[C]**

## Framing ("Makera protocol")

`0x86 0x68 | len:u16be | type:u8 | payload | crc:u16be | 0x55 0xAA` **[C]**

- `len = 1 + payload.len() + 2`
- CRC-16/CCITT, XMODEM variant (init 0), over `len + type + payload`.
- Sources: firmware `src/libs/PublicData.h`, `SerialConsole.cpp::gets`, `WifiProvider.cpp`; host
  community `protocols/framing.py`, `protocols/makera.py`, `protocols/detector.py`; OpenSpindle
  `src/machine/firmware/makera/codec.ts`.

| Dir | Type | Meaning |
|---|---|---|
| →M | `0xA1` CTRL_SINGLE | one realtime byte |
| →M | `0xA2` CTRL_MULTI | command line (no trailing newline) |
| →M | `0xB0` FILE_START | `upload` / `download` command |
| ↔ | `0xB1` MD5, `0xB2` VIEW, `0xB3` DATA, `0xB4` END, `0xB5` CAN, `0xB6` RETRY | file transfer |
| M→ | `0x81` STATUS_RES | status report |
| M→ | `0x82` DIAG_RES | diagnose report |
| M→ | `0x83` LOAD_INFO, `0x84` LOAD_FINISH, `0x85` LOAD_ERROR | load results |
| M→ | `0x90` NORMAL_INFO | console text, may be fragmented — buffer until `\n` |

Z1 LPC also defines `0x71` FIRM_VER, `0xD1–D5` CONFIG_*, `0xE1–E5` FACTORY_*, `0xF1–F7` PLAY_*/GOTO_*
(`MakeraZ1Firmware/src/libs/Kernel.h:139-157`, `SerialConsole.cpp:205-275`) **[C]** — likely the
internal ESP↔LPC link. **[I]**

**Detection:** send raw `echo echo\n`; plaintext echo ⇒ legacy Smoothie line protocol, silence ⇒
framed. Community fw: `M485` report, `M485.1` Smoothie, `M485.2` Makera
(`SimpleShell.cpp:340`). **[C]**

## Realtime, status, diagnose

- Realtime bytes in `0xA1`: `?` status, `0x18` halt, `!` hold / `~` resume (only if feed hold enabled). **[C]**
- Community-only: continuous jog keepalive `?`+`0x1A` in one write, stop `0x19`. Stock Z1 support unverified.
- **Status** (`Kernel::get_query_string()`): **[C]**
  ```
  <State|MPos:x,y,z,a,b|WPos:x,y,z,a,b|F:cur,req,ovr|S:cur,req,ovr,vacmode,spindleTemp,powerTemp,0,0,extout
   |T:tool,tlo[,target]|W:wpVolt|L:mode,state,testing,power,scale|P:lines,pct,secs|A:atcState
   |O:levelMaxDelta|H:haltReason|C:model,funcSetting,inch,abs>
  ```
  - States: `Sleep Pause Wait Tool Alarm Home Hold Idle Run`.
  - `C:` model: 1 Carvera, 2 Air, 3 Z1, 4 Z1 Pro.
  - Z1 adds `|E:transfer,rec-req,recording,sd-used,sd-total` (timelapse) and `|OTA:phase,progress`
    (community `develop:Controller.py:1530`), likely from the ESP. **[I]**
  - Parser: `Controller.py::parseBracketAngle`.
- **Diagnose:** command `diagnose` → `{S:..|L:..|V:..|F:..|G:..|...}`; Z1 adds `|E: |P: |A: |I: |RSSI:`.
  `Kernel::get_diagnose_string()`, parser `Controller.py::parseBigParentheses`. **[C]**
- **Halt reasons** (`enum HALT_REASON`, `Kernel.h`): **[C]**
  - no reset: 1 MANUAL, 2 HOME_FAIL, 3 PROBE_FAIL, 4 CALIBRATE_FAIL, 5–8 ATC, 9 SPINDLE_OVERHEATED,
    10 SOFT_LIMIT, 11 COVER_OPEN, 12 PROBE_INVALID, 13 E_STOP, 14 POWER_OVERHEATED, 15 NON_HOME
  - reset: 21 HARD_LIMIT, 22–24 MOTOR_ERROR_X/Y/Z, 25 SPINDLE_STALL, 26 SD_ERROR
  - power cycle: 41 SPINDLE_ALARM
- **ATC state:** 0 none, 1 drop, 2 pick, 3 calibrate, 4 margin, 5 zprobe, 6 autolevel, 9 done.
- Messages prefixed `ALARM:` / `ERROR:` / `Error:`; halt prints `HALTED, M999 or $X to exit HALT state`.

## Console commands

`commands_table[]` in `src/modules/utils/simpleshell/SimpleShell.cpp`. Z1 list: `ls cd pwd cat echo
rm mv mkdir upload download reset dfu break help ftype version mem get set_temp switch net diagnose
sleep power load save remount md5sum time test model check_4th check_5th fset fget …`. Player
(`src/modules/utils/player/Player.cpp`): `play abort suspend resume goto buffer upload download progress`. **[C]**

Host usage (community `Controller.py`):

- Files: `ls -e -s <dir>`, `md5sum <f> -e`, `rm`, `mv`, `mkdir`, `cat`.
- Jobs: `play <file> [-O]`, `suspend`, `resume`, `abort`.
- Info: `model` → `model = Z1, <id>, <func>, <x>`; `version`; `ftype` (`.lz` support); `time <unix>`.
- Config: `config-get-all -e`, `config-set sd <key> <val>`, `config-restore`, `config-default`.
- Arg escaping: space → `\x01`, `? & ! ~` → `\x02`–`\x05` (`Controller.py::escape`; Hobby
  Jogger had `*` for `\x03` until 2026-10-27). **[C]**
- `buffer <cmd>` queues into the job. Jog `$J X.. [F..]` (community: `$J -c` continuous). Home `$H`.

## File transfer

Machine-pulled framed upload (`Player::upload_command` @ `Player.cpp:1127`; host `XMODEM.py::send`;
OpenSpindle `transfer.ts`): **[C]**

1. Host → FILE_START `upload /sd/gcodes/x.nc[.lz]`
2. Host → FILE_MD5 (32-char hex)
3. Machine → FILE_VIEW; host → FILE_VIEW `u32 packet_count, u16 packet_size`
4. Machine → FILE_DATA `u32 seq`; host → FILE_DATA `u32 seq + data` (repeat)
5. Machine → FILE_END (payload may be `ok\r\n`). CAN aborts; RETRY resends last frame.

- Packet size 8192 on WiFi, 128 on USB.
- **Compression:** if `ftype` contains `lz`, send QuickLZ: 4096-byte blocks as `u32be len + data`,
  then `u16be` byte-sum. Firmware reports `decompart = N`.
- Download mirrors upload (`Player::download_command`).
- **Z1 quirk:** `md5sum` returns placeholder `default_md5_hash_value_32_bytes_` (`XMODEM.py:492`).
- Legacy Smoothie mode: XMODEM 128 (SOH)/8K (STX), CRC-16, 2-byte length prefix, MD5 as seq 0.

## Machine features

- **Tool change:** `M6 T<n>`, `M6 T-1` drop. TLO calibrate `M491` (community `M491.1` break check).
  `M490.1`/`.2` clamp/unclamp. `M493.2 T<n>` set current tool. Z1 handler also `M486`–`M499`, `M887`/`M888`.
- **Probing:** G38.2–G38.5 → `[PRB:x,y,z:1]` (verified on Z1); failure `ALARM: Probe fail` (`ZProbe.cpp:599,604`).
  G38.6 calibration on Z1. Built-in 3D routines `M480.1`–`M480.10`; `M495.3 H D` XYZ probe.
  Community fw M460–M466.
- **Autolevel / margin:** `M495 X Y [C D] [O F] [A B I J H] [P1 T]` (`Controller.py::autoCommand`);
  clear `M370`; `O:` field = max delta.
- **Laser:** `M321`/`M322` on/off, `M323`/`M324` test, `M325 S` scale, fire `M3 S<0..1>`.
  On the Z1 (and Air) `M321` also does `M6 T8888` (a manual tool change to the laser module),
  `G92.5 Z0`, and runs the power fan at 40 % (`M801 S40`); `M322` does `G92.1` and leaves the
  tool alone. Test fire runs at `laser_module_test_power` × scale (`Laser.cpp`, 1.1.3). **[C]**
- **Diagnose on the Z1** (`Kernel::get_diagnose_string`, 1.1.3): `V` is the power fan on machines
  without an ATC (vacuum with one), `F` the spindle fan, `G:light,beep,extin,extout,extoutPWM`,
  `R` air (absent on the Z1 seen so far), `T` tool sensor, `C` probe charger. **[C]**
- **`config-get-all -e`** prints `/sd/config.txt` as `key=value` lines, then an EOT (0x04)
  without a newline (`SimpleShell::config_get_all_command`). **[C]** On the Z1 over WiFi
  (2026-10-28) nothing recognisable came back; the community controller downloads
  `/sd/config.txt` instead, and so does Hobby Jogger.
- **Overrides:** feed `M220 S%`, spindle `M223 S%`.
- **Outputs:** vacuum M801/M802, spindle fan M811/M812, light M821/M822, ext out M851/M852,
  tool sensor M831/M832, air M7/M9, modes M331/M332 (.3 ext-out, .4 ionizer), Z1 timelapse M951/M952.
- **4th axis:** A in MPos/WPos; shrink `G92.4 A0 S0`; `check_4th`, `enable_4th_hd`.
- **Firmware update:** Z1 — `lpc1768.bin` on TF root + power cycle; bundles to `/sd/firmware.bin`
  (magic `0x4D5173EE`); ESP OTA `POST http://<ip>:80/update` multipart `firmware`
  (community `develop:updater/z1_firmware.py`, `updater/esp_ota.py`).

## Z1 compatibility

- **Confirmed:** same frame format, PTYPEs, status/diagnose builders, shell commands, M-codes; Community
  Controller and OpenSpindle both drive real Z1s over 2222/3333 framed; config keys match Carvera's.
- **Verify on hardware:** what the ESP32 passes through vs handles itself (storage, md5, E/OTA
  fields, discovery version field); continuous jog on stock; `Carvera.cps` output on Z1 unchanged.
- Makera wiki supported-codes page is JS-rendered and wasn't readable.
