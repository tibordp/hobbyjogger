# Research

Background research for Hobby Jogger, collected 2026-10-26. Nothing here has been verified on a
real Z1 yet.

- [z1-protocol.md](z1-protocol.md) — Z1 hardware/firmware architecture, wire protocol, G/M-codes,
  open questions for hands-on testing.
- [carvera-reference.md](carvera-reference.md) — detailed protocol reference extracted from the
  Makera / Carvera open-source firmware and controllers, with file pointers.
- [probing.md](probing.md) — probing building blocks and verified touch-off routines.
- [oss-prior-art.md](oss-prior-art.md) — general CNC senders, probing routines, conversational CAM,
  Rust/React libraries.

## Key takeaways

1. **The protocol is knowable.** The Z1 motion firmware is open source (Smoothieware fork, GPL-3.0,
   [MakeraInc/MakeraZ1Firmware](https://github.com/MakeraInc/MakeraZ1Firmware)) and two open
   controllers already drive real Z1s. Reverse engineering is only needed for the closed ESP32
   layer's quirks.
2. **Two boards.** LPC1768 does motion; a closed ESP32 owns WiFi, USB, SD card, camera, HTTP.
   We talk to the ESP32.
3. **One framed protocol, two transports.** `0x8668 | len | type | payload | crc16 | 0x55AA` over
   TCP 2222 (WiFi, discovery via UDP 3333 broadcast) or **USB vendor bulk** (VID 0x303A /
   PID 0x4002 — libusb, *not* a serial port). The transport abstraction is "a byte stream of
   frames", the framing codec sits above it.
4. **Jobs run from the machine's SD card.** Upload is machine-pulled and block-based (no
   compression on the stock Z1); then `play`. Interactive things (jog, MDI, probing) are individual command
   frames.
5. **Status is a Carvera-style `<State|MPos|WPos|F|S|T|...>` line** with a well-defined halt-reason
   enum — good basis for the Rust machine-state model.
6. **Probing primitives exist and report cleanly** (`G38.x` → `[PRB:x,y,z:1]` (verified on Z1)), so Rust-side
   orchestrated probing routines are viable.
7. **Licensing.** Almost all Makera/Carvera code is GPL; we're GPL-3.0 too (see below). MIT references
   worth reading closely: [OpenSpindle](https://github.com/openspindle/openspindle) (Z1 controller
   in TS, closest prior art), [carve-control](https://github.com/GridSpace/carve-control), cncjs,
   ioSender (BSD, probing math).
8. **Dev without hardware:** [carvera_firmware_simulator](https://github.com/f355/carvera_firmware_simulator)
   runs the community firmware on the host with a fake WiFi endpoint on :2222 and a Z1 config —
   a candidate test target alongside our own mock backend.

## Implications for the design

- Rust crates: `nusb`/`rusb` for USB, `tokio` TCP/UDP for WiFi; no `serialport` needed for Z1.
- Linux needs a udev rule for 303A:4002; macOS should work without a driver (to verify).
- **Hobby Jogger is GPL-3.0** (decided 2026-10-27), so GPL-3.0 sources (Makera firmware and
  controller, Carvera Community firmware, gSender, UGS) can be borrowed from directly with
  attribution. GPL-2.0-*only* code (Carvera Community Controller, bCNC, LinuxCNC) is not
  compatible with GPL-3.0 — still read-only reference unless a file says "or later".
  Unlicensed repos stay read-only.
