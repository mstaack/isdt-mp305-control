# mp305

[![CI](https://github.com/mstaack/isdt-mp305-control/actions/workflows/ci.yml/badge.svg)](https://github.com/mstaack/isdt-mp305-control/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/mstaack/isdt-mp305-control)](https://github.com/mstaack/isdt-mp305-control/releases/latest)

Command-line tool and Rust library to control the **ISDT MP305B** (and MP305A) smart bench power supply
over USB: live readings, voltage/current setpoints, output on/off, CSV logging, system settings and battery
charging. One static binary, no drivers, no Python.

```
$ mp305 status
MP305B — firmware V1.6.0.51
mode:      DC
output:    ON
measured:    5.00 V   0.000 A     0.00 W
setpoint:    5.00 V   1.000 A  (CC limit)
energy:    0.000 Wh over 0:00:00
temp:      26 °C
```

## Install

**macOS (Apple Silicon or Intel) and Linux x86_64 — prebuilt binary, no Rust needed:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/mstaack/isdt-mp305-control/releases/latest/download/mp305-installer.sh | sh
```

This installs `mp305` into `~/.cargo/bin` and adds it to your `PATH` (open a new terminal afterwards).
Binaries and checksums are also on the [releases page](https://github.com/mstaack/isdt-mp305-control/releases).

**From source** (needs [Rust](https://rustup.rs)):

```sh
cargo install --git https://github.com/mstaack/isdt-mp305-control
```

**Linux permissions** — let your user access the device without root:

```sh
echo 'SUBSYSTEM=="hidraw", ATTRS{idVendor}=="28e9", MODE="0660", TAG+="uaccess"' | sudo tee /etc/udev/rules.d/99-mp305.rules
sudo udevadm control --reload && sudo udevadm trigger   # then replug the PSU
```

macOS needs nothing — just plug the PSU in via USB.

## Usage

```sh
mp305 list                              # connected devices
mp305 info                              # name, device id, firmware/hardware/boot versions
mp305 status [--json]                   # firmware, mode, output, measured + set V/I, W, Wh, temp, errors

mp305 set -v 5 -i 1 --on                # set voltage + current limit, enable output
mp305 set -v 12                         # change only the voltage (everything else is kept)
mp305 set -i 0.5 --reapply              # lower the current limit while on (cycles output so CC re-arms)
mp305 set --ocp                         # over-current trips the output off …
mp305 set --cc                          # … or is limited (constant current, default)
mp305 on
mp305 off
mp305 release                           # hand control back to the front panel

mp305 watch                             # live updating line, every 500 ms (Ctrl-C to stop)
mp305 watch --interval 200 --csv run.csv   # also append timestamped samples to a CSV
mp305 watch --json --count 10           # JSON lines, for scripts

mp305 settings                          # system settings
mp305 settings set --volume 2           # change one field (read-modify-write)

mp305 mode dc|charge                    # switch operating mode
mp305 charge status
mp305 charge start --type lipo --cells 3 --current 1.5 [--cell-voltage 4.2] [-y]
mp305 charge stop
```

Global options: `--device <path>` (from `mp305 list`) and `--timeout <ms>`.
`MP305_DEBUG=1` dumps the raw HID replies to stderr.

CSV columns: `timestamp, elapsed_s, voltage_v, current_a, power_w, energy_wh, set_voltage_v, set_current_a,
output, mode, temperature_c, errors`.

## Good to know

- Any change takes **remote control** of the PSU; it is held until `mp305 release` (or you take over on the panel).
- **Lowering** the current limit while the output is on only takes effect after an output cycle — a firmware
  quirk, also present in ISDT's own app. Use `--reapply`.
- `set` / `on` / `off` work in DC mode only. `charge start` switches to charge mode by itself; use
  `mp305 mode dc` to go back.
- Limits enforced by the tool: 0–30 V, 0–5 A.
- If commands are refused, close ISDT WebLink / the PolyLink app — only one controller can hold the PSU.

### Status

Verified on an **MP305B, firmware V1.6.0.51**: info, status, settings, setpoints, output on/off, release,
mode switching, watch/CSV, charge status.

**Battery charging (`charge start`/`stop`) is not hardware-verified yet** — supervise it, and check the PSU's
screen. NiMH/Cd charging is refused for now because its parameter encoding is unverified.
MP305A should work (same protocol) but is untested.

## Library

The crate is also a library:

```rust
use mp305::{Mp305, OutputSettings};

let mut psu = Mp305::open_usb(None)?;
println!("{}", psu.info()?.firmware_version);
psu.set_output(&OutputSettings { voltage: Some(5.0), current: Some(1.0), on: Some(true), ..Default::default() })?;
let st = psu.state()?;
println!("{:.2} V {:.3} A", st.voltage, st.current);
psu.release()?;
```

| Module | Role |
|---|---|
| `protocol` | frame build/parse (length, `0xAA` stuffing, checksum) — pure, golden-vector tests |
| `types` | decoded responses with units (`State`, `HardwareInfo`, `ChargeState`, …) |
| `commands` | request payload builders |
| `transport` | `Transport` trait + `HidTransport` (a Bluetooth transport can slot in later) |
| `client` | `Mp305<T>`: request/response, remote-control handshake, mode switching, output/charge ops |

## Roadmap

Bluetooth (BLE) transport, USB-PD profiles, programmable V/I/time sequences — the protocol for all of them
is known (see pymp305).

## Credits & license

MIT. The wire protocol is ported from [pymp305](https://github.com/nemanjan00/pymp305) by nemanjan00 (MIT),
which reverse-engineered ISDT's WebLink app. Not affiliated with ISDT.

Contributing with an AI agent? See [AGENTS.md](AGENTS.md).
