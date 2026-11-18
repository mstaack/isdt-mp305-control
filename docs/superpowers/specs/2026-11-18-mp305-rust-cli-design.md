# MP305 Rust CLI — Design

## Goal
A Rust command-line tool for bench use with an ISDT MP305B power supply over USB-HID:
live readings, DC setpoints/output control, firmware/device info, system settings, and
battery charging. Success = the commands below work against the real MP305B and readings
match the front panel.

Out of scope for v1 (structure must allow adding later): Bluetooth, USB-PD profiles,
programmable sequences, OTA/reboot/bootloader.

## Protocol source
Ported from [pymp305](https://github.com/nemanjan00/pymp305) (MIT), which reverse-engineered
ISDT WebLink and is hardware-verified on an MP305B.

- HID: VID `0x28E9`, usage page `0x01` / usage `0x04`; output report id 1, 64-byte reports.
- Frame: `[N, 0xAA, 0x12, L=1+len(payload), cmd, payload…, checksum]`. Every `0xAA` at index > 5
  is doubled; checksum = sum of bytes from index 2 to second-to-last (a doubled `0xAA` counted
  once) & 0xFF; if checksum == `0xAA`, it is doubled too. `N` = frame length − 1.
- Responses use group `0x21`; parse by locating `0xAA, 0x12|0x21`, de-stuffing, and truncating to `N`.
- Remote control: request with `remoteCon=2` in the current mode's connect command, then send changes with
  `remoteCon=1`; status byte (first payload byte) 1 = rejected. `remoteCon=0` releases to the panel.

Commands used: `E0→E1` hw info, `C2→C3` state, `C4→C5` / `C6→C7` system settings,
`C8→C9` DC control, `EC→ED` charge state, `EA→EB` charge settings, `EE→EF` charge control.

## Architecture (single crate `mp305`, lib + bin)
- `protocol` — pure framing (build/parse), constants. No I/O. Unit-tested with pymp305's golden vectors.
- `types` — decoded responses with physical units (`HardwareInfo`, `State`, `SystemSettings`,
  `ChargeState`, `ChargeInfo`), `Mode`, `BatteryType`, error-bit decoding.
- `commands` — payload builders (`ControlCommand`, `ChargeCommand`, `SystemSetCommand`).
- `transport` — `Transport` trait (`send_report`, `read_report(timeout)`); `HidTransport` (hidapi);
  a mock in tests. BLE later = another `Transport`.
- `client` — `Mp305<T: Transport>`: request/response matching, remote handshake, mode switching,
  `set_output` (with the "lower current while on needs output cycle" `reapply` workaround), charge ops.
- `main` — `clap` CLI.

## CLI
```
mp305 list                                   # enumerate devices
mp305 info                                   # name, device id, hardware/boot/firmware versions
mp305 status [--json]                        # firmware + mode, output, measured & set V/I, W, Wh, temp, errors
mp305 set [-v V] [-i A] [--on|--off] [--ocp|--cc] [--reapply]
mp305 on | off
mp305 release                                # hand control back to the front panel
mp305 mode dc|charge
mp305 watch [--interval ms] [--count n] [--csv FILE] [--json]
mp305 settings [show]
mp305 settings set [--volume n] [--screen-off n] [--shutdown n] [--power-limit n] …   # read-modify-write
mp305 charge status [--json]
mp305 charge start [--type lipo|lihv|liion|life|pb|nimh] [--cells n] [--current A] [--cell-voltage V]
mp305 charge stop
```
- `watch`: refreshing single line on a TTY; `--csv` appends one timestamped row per sample
  (flushed each row); `--json` prints one JSON object per line instead of the live line.
- `charge start` defaults any unspecified field from the device's current charge settings (`0xEA`),
  switches to charge mode, then starts. Charging is not hardware-verified upstream → printed warning.
- `set` refuses outside DC mode with a hint to run `mp305 mode dc`. V limited to 0–30 V, I to 0–5 A.

## Errors
`thiserror` in the lib (`NotFound`, `Timeout{cmd}`, `RemoteRefused`, `WrongMode`, `Hid`), `anyhow` in the CLI
with actionable messages.

## Testing
Unit tests: framing golden vectors, stuffing round-trip, decoders, payload layouts, client flows against a
mock transport. Manual hardware check on the connected MP305B: `info`, `status`, `set`/`on`/`off`, `watch`.
