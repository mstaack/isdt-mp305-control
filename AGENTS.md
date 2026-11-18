# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex, etc.) working in this repo. `CLAUDE.md` is a symlink to this file.

## What this is

`mp305` — a Rust CLI + library that controls an **ISDT MP305B/MP305A** bench power supply over USB-HID
(VID `0x28E9`, PID `0x028A` for the MP305B). The wire protocol is ported from
[pymp305](https://github.com/nemanjan00/pymp305) (MIT); when in doubt about a field offset or command,
that project's `protocol.py` / `responses.py` / `device.py` are the reference.

## Commands

```sh
cargo build                          # debug build
cargo test                           # unit tests — no hardware needed
cargo clippy --all-targets -- -D warnings
cargo fmt --check                    # CI enforces fmt + clippy + tests
MP305_DEBUG=1 cargo run -- status    # dump raw HID input reports to stderr
dist plan                            # preview release artifacts (cargo-dist)
```

## Layout

| File | Role |
|---|---|
| `src/protocol.rs` | Frame build/parse: length byte, `0xAA` stuffing, checksum, HID report fragmentation. Pure; golden-vector tests. |
| `src/types.rs` | Response decoders with physical units (`State`, `HardwareInfo`, `SystemSettings`, `ChargeState`, `ChargeInfo`). Short frames read as 0, never panic. |
| `src/commands.rs` | Request payload builders (`ControlCommand` 0xC8, `ChargeCommand` 0xEE, `SystemSetCommand` 0xC6). |
| `src/transport.rs` | `Transport` trait + `HidTransport` (hidapi). A BLE transport should implement the same trait. |
| `src/client.rs` | `Mp305<T: Transport>`: request/response matching, remote-control handshake, mode switching, output and charge ops. Mock-transport tests. |
| `src/main.rs` | `clap` CLI. |
| `docs/superpowers/specs/` | Original design spec. |

## Protocol facts that bite

- Frame: `[N, 0xAA, 0x12, L, cmd, payload…, checksum]`; replies use group `0x21`. Every `0xAA` at index > 5 is
  doubled; checksum sums index 2..last counting a doubled `0xAA` once.
- **Remote control is a handshake**: send the current mode's connect command with `remoteCon=2` (request) first,
  then changes with `remoteCon=1`. Reply status byte `1` = rejected. `remoteCon=0` releases to the panel.
  Each CLI invocation is a new process, so it re-requests control every time — that is expected.
- Mode switches go through the **current** mode's connect command (DC `0xC8`, program `0xE2`, USB-PD `0xE8`,
  charge `0xEE`).
- The `0xC3` state frame **lags** a control command by a few hundred ms — `set_output` polls until it matches.
- Lowering the current limit while output is on doesn't engage CC until the output is cycled (`reapply`).
- On macOS the HID interface reports usage `0x01/0x00`, not `0x01/0x04`; `HidTransport::open` falls back to the
  first VID match.

## Hardware safety

- **Never energize the output or start a charge on real hardware without the user's explicit OK** — something
  may be connected to the terminals. Read-only commands (`info`, `status`, `settings`, `charge status`, `watch`)
  and setpoint changes with `--off` are safe.
- After any hardware test, restore the user's previous setpoints, turn the output off, and run `mp305 release`.
- Charge control is not hardware-verified; NiMH/Cd start is deliberately refused (encoding unverified).
- Never send `system_check`/`recover` (factory reset) in `0xC6`, and don't add OTA/bootloader/reboot commands
  without gating them behind explicit confirmation.

## Conventions

- Library errors: `mp305::Error` (`thiserror`), with actionable messages. CLI: `anyhow`.
- New commands: add the byte constants to `protocol::{cmd, resp}`, a decoder in `types.rs` and/or builder in
  `commands.rs` with a unit test, then the client method, then the CLI subcommand. Update `README.md`.
- Releases: bump `version` in `Cargo.toml`, commit, tag `vX.Y.Z`, push the tag — `.github/workflows/release.yml`
  (cargo-dist) builds macOS arm64/x86_64 + Linux x86_64 binaries and the shell installer.
