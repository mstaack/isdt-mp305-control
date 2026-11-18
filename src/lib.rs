//! Client library for the ISDT MP305 (MP305A/MP305B) smart bench power supply.
//!
//! Protocol ported from [pymp305](https://github.com/nemanjan00/pymp305) (MIT).

pub mod client;
pub mod commands;
pub mod protocol;
pub mod transport;
pub mod types;

pub use client::{ChargeSettings, Mp305, OutputSettings};
pub use types::{BatteryType, Mode};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no MP305 found on USB (vendor id 0x28E9) — is it plugged in and switched on?")]
    NotFound,
    #[error("USB HID error: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("no reply 0x{expect:02X} to command 0x{cmd:02X} (timed out)")]
    Timeout { cmd: u8, expect: u8 },
    #[error(
        "the PSU refused remote control — is another app (WebLink/PolyLink) connected, or the panel locked?"
    )]
    RemoteRefused,
    #[error("the PSU is in {actual} mode; this needs {needed} mode (run `mp305 mode {}`)", mode_arg(*needed))]
    WrongMode { needed: Mode, actual: Mode },
    #[error("mode switch to {target} not confirmed (device still reports {actual})")]
    ModeSwitch { target: Mode, actual: Mode },
    #[error("out of range: {0}")]
    OutOfRange(String),
    #[error(transparent)]
    Frame(#[from] protocol::PayloadTooLong),
}

fn mode_arg(m: Mode) -> &'static str {
    match m {
        Mode::Charge => "charge",
        _ => "dc",
    }
}
