//! High-level MP305 operations on top of a [`Transport`].

use std::time::{Duration, Instant};

use crate::Error;
use crate::commands::{self, ChargeCommand, ControlCommand, Remote, SystemSetCommand};
use crate::protocol::{Frame, cmd, resp};
use crate::transport::{HidTransport, Transport};
use crate::types::*;

pub const MAX_VOLTAGE: f64 = 30.0;
pub const MAX_CURRENT: f64 = 5.0;

/// What `set_output` should change; `None` keeps the current value.
#[derive(Debug, Clone, Default)]
pub struct OutputSettings {
    pub voltage: Option<f64>,
    pub current: Option<f64>,
    pub on: Option<bool>,
    /// `Some(true)` = over-current trips the output (OCP), `Some(false)` = constant-current limit.
    pub ocp: Option<bool>,
    /// Cycle the output after applying, so a *lowered* current limit re-arms CC immediately.
    pub reapply: bool,
}

/// What `start_charge` should use; `None` falls back to the device's stored charge settings.
#[derive(Debug, Clone, Default)]
pub struct ChargeSettings {
    pub battery_type: Option<BatteryType>,
    pub cells: Option<u8>,
    pub current: Option<f64>,
    pub cell_voltage: Option<f64>,
}

pub struct Mp305<T: Transport> {
    t: T,
    timeout: Duration,
    device_name: Option<String>,
    remote_held: bool,
}

impl Mp305<HidTransport> {
    /// Open the first MP305 found over USB (or the HID device at `path`).
    pub fn open_usb(path: Option<&str>) -> Result<Self, Error> {
        let mut dev = Self::new(HidTransport::open(path)?);
        // Learn the model name for model-aware error decoding.
        let _ = dev.info();
        Ok(dev)
    }
}

impl<T: Transport> Mp305<T> {
    pub fn new(t: T) -> Self {
        Self {
            t,
            timeout: Duration::from_millis(1500),
            device_name: None,
            remote_held: false,
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Send `command` and wait for the frame whose command byte is `expect`.
    pub fn request(&mut self, command: u8, expect: u8, payload: &[u8]) -> Result<Frame, Error> {
        self.t.send(command, payload)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout {
                    cmd: command,
                    expect,
                });
            }
            if let Some(f) = self.t.recv(left)?
                && f.cmd == expect
            {
                return Ok(f);
            }
        }
    }

    fn name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    pub fn info(&mut self) -> Result<HardwareInfo, Error> {
        let f = self.request(cmd::HW_INFO, resp::HW_INFO, &[])?;
        let info = HardwareInfo::parse(&f.values);
        if !info.device_name.is_empty() {
            self.device_name = Some(info.device_name.clone());
        }
        Ok(info)
    }

    pub fn state(&mut self) -> Result<State, Error> {
        let f = self.request(cmd::STATE_INFO, resp::STATE, &[])?;
        Ok(State::parse(&f.values, self.name()))
    }

    pub fn system_settings(&mut self) -> Result<SystemSettings, Error> {
        let f = self.request(cmd::SYS_GET, resp::SYS, &[])?;
        Ok(SystemSettings::parse(&f.values))
    }

    pub fn set_system_settings(&mut self, s: &SystemSetCommand) -> Result<(), Error> {
        self.request(cmd::SYS_SET, resp::SYS_SET, &s.payload())?;
        Ok(())
    }

    pub fn charge_state(&mut self) -> Result<ChargeState, Error> {
        let f = self.request(cmd::CHARGE_INFO, resp::CHARGE, &[])?;
        Ok(ChargeState::parse(&f.values, self.name()))
    }

    pub fn charge_settings(&mut self) -> Result<ChargeInfo, Error> {
        let f = self.request(cmd::CHARGE_SEARCH, resp::CHARGE_INFO, &[])?;
        Ok(ChargeInfo::parse(&f.values))
    }

    // ---- remote control & modes ----------------------------------------

    /// Each mode has its own connect command; remote requests and mode switches go
    /// through the *current* mode's command.
    fn connect_cmd(mode: Mode) -> (u8, u8) {
        match mode {
            Mode::Programmable => (cmd::PROGRAM_CONNECT, resp::PROGRAM_CONNECT),
            Mode::UsbPd => (cmd::PDO_CONNECT, resp::PDO_CONNECT),
            Mode::Charge => (cmd::CHARGE_CONTROL, resp::CHARGE_CONTROL),
            _ => (cmd::CONTROL, resp::CONTROL),
        }
    }

    /// Payload for `mode`'s connect command, output off, selecting `target`.
    /// When leaving DC, the DC setpoints from `st` are preserved.
    fn connect_payload(mode: Mode, remote: Remote, target: Mode, st: Option<&State>) -> Vec<u8> {
        match mode {
            Mode::Programmable => commands::program_connect(remote, false, target),
            Mode::UsbPd => commands::pdo_connect(remote, false, target),
            Mode::Charge => ChargeCommand {
                remote,
                mode: target,
                ..Default::default()
            }
            .payload(),
            _ => {
                let mut c = ControlCommand {
                    remote,
                    mode: target,
                    ..Default::default()
                };
                if let Some(st) = st {
                    c.set_voltage = st.set_voltage;
                    c.set_current = st.set_current;
                    c.voltage_slow = st.voltage_slow;
                    c.current_over = st.current_over;
                }
                c.payload()
            }
        }
    }

    fn status(f: &Frame) -> Option<u8> {
        f.payload.first().copied()
    }

    fn ensure_remote(&mut self, mode: Mode) -> Result<(), Error> {
        if self.remote_held {
            return Ok(());
        }
        let (c, r) = Self::connect_cmd(mode);
        let f = self.request(
            c,
            r,
            &Self::connect_payload(mode, Remote::Request, mode, None),
        )?;
        self.remote_held = Self::status(&f) == Some(0);
        if self.remote_held {
            Ok(())
        } else {
            Err(Error::RemoteRefused)
        }
    }

    /// Send a connect command that needs remote control, re-acquiring control once if rejected.
    fn connect(&mut self, mode: Mode, payload: &[u8]) -> Result<Frame, Error> {
        let (c, r) = Self::connect_cmd(mode);
        self.ensure_remote(mode)?;
        let f = self.request(c, r, payload)?;
        if Self::status(&f) != Some(1) {
            return Ok(f);
        }
        self.remote_held = false;
        self.ensure_remote(mode)?;
        let f = self.request(c, r, payload)?;
        if Self::status(&f) == Some(1) {
            return Err(Error::RemoteRefused);
        }
        Ok(f)
    }

    /// Switch the active mode (takes and keeps remote control). Output is turned off.
    pub fn set_mode(&mut self, target: Mode) -> Result<(), Error> {
        let st = self.state()?;
        self.ensure_remote(st.mode)?;
        if st.mode == target {
            return Ok(());
        }
        let payload = Self::connect_payload(st.mode, Remote::Apply, target, Some(&st));
        self.connect(st.mode, &payload)?;
        let mut last = st.mode;
        for _ in 0..8 {
            last = self.state()?.mode;
            if last == target {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(Error::ModeSwitch {
            target,
            actual: last,
        })
    }

    /// Hand control back to the front panel.
    pub fn release(&mut self) -> Result<(), Error> {
        let mode = self.state()?.mode;
        let (c, r) = Self::connect_cmd(mode);
        self.request(
            c,
            r,
            &Self::connect_payload(mode, Remote::Release, mode, None),
        )?;
        self.remote_held = false;
        Ok(())
    }

    // ---- DC output -------------------------------------------------------

    /// Apply DC setpoints/output (unspecified values are preserved). Requires DC mode.
    pub fn set_output(&mut self, o: &OutputSettings) -> Result<State, Error> {
        if let Some(v) = o.voltage
            && !(0.0..=MAX_VOLTAGE).contains(&v)
        {
            return Err(Error::OutOfRange(format!(
                "voltage {v} V (0–{MAX_VOLTAGE} V)"
            )));
        }
        if let Some(i) = o.current
            && !(0.0..=MAX_CURRENT).contains(&i)
        {
            return Err(Error::OutOfRange(format!(
                "current {i} A (0–{MAX_CURRENT} A)"
            )));
        }
        let st = self.state()?;
        if st.mode != Mode::Dc {
            return Err(Error::WrongMode {
                needed: Mode::Dc,
                actual: st.mode,
            });
        }
        let mut c = ControlCommand {
            remote: Remote::Apply,
            set_voltage: o.voltage.unwrap_or(st.set_voltage),
            set_current: o.current.unwrap_or(st.set_current),
            real_change: 3,
            voltage_slow: st.voltage_slow,
            current_over: o.ocp.map(u8::from).unwrap_or(st.current_over),
            output: o.on.unwrap_or(st.output),
            mode: Mode::Dc,
        };
        self.connect(Mode::Dc, &c.payload())?;
        if o.reapply && c.output {
            c.output = false;
            self.connect(Mode::Dc, &c.payload())?;
            c.output = true;
            self.connect(Mode::Dc, &c.payload())?;
        }
        // The state frame lags a control command by a few hundred ms; poll until it reflects it.
        let applied = |s: &State| {
            (s.set_voltage - c.set_voltage).abs() < 0.006
                && (s.set_current - c.set_current).abs() < 0.0006
                && s.output == c.output
        };
        let mut st = self.state()?;
        for _ in 0..15 {
            if applied(&st) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            st = self.state()?;
        }
        Ok(st)
    }

    // ---- charging --------------------------------------------------------

    /// Switch to charge mode and start charging. Unspecified fields come from the
    /// device's stored charge settings.
    pub fn start_charge(&mut self, s: &ChargeSettings) -> Result<ChargeCommand, Error> {
        let stored = self.charge_settings()?;
        let battery_type = s
            .battery_type
            .or(stored.battery_type)
            .ok_or_else(|| Error::OutOfRange("battery type unknown; pass --type".into()))?;
        let cell_voltage = match s.cell_voltage {
            Some(v) => v,
            None if Some(battery_type) == stored.battery_type => {
                if battery_type == BatteryType::Nimh {
                    stored.capacity_voltage_raw as f64
                } else {
                    stored.cell_voltage()
                }
            }
            None => default_cell_voltage(battery_type),
        };
        let c = ChargeCommand {
            remote: Remote::Apply,
            battery_type,
            cell_voltage,
            cells: s.cells.unwrap_or(stored.cells).max(1),
            current: s.current.unwrap_or(stored.current),
            output: true,
            mode: Mode::Charge,
        };
        if c.current <= 0.0 || c.current > MAX_CURRENT {
            return Err(Error::OutOfRange(format!(
                "charge current {} A (0–{MAX_CURRENT} A)",
                c.current
            )));
        }
        self.set_mode(Mode::Charge)?;
        self.connect(Mode::Charge, &c.payload())?;
        Ok(c)
    }

    /// Stop charging (output off, stays in charge mode until `release` / `set_mode`).
    pub fn stop_charge(&mut self) -> Result<(), Error> {
        let st = self.state()?;
        if st.mode != Mode::Charge {
            return Err(Error::WrongMode {
                needed: Mode::Charge,
                actual: st.mode,
            });
        }
        let stored = self.charge_settings()?;
        let c = ChargeCommand {
            remote: Remote::Apply,
            battery_type: stored.battery_type.unwrap_or(BatteryType::Lipo),
            cell_voltage: match stored.battery_type {
                Some(BatteryType::Nimh) => stored.capacity_voltage_raw as f64,
                _ => stored.cell_voltage(),
            },
            cells: stored.cells,
            current: stored.current,
            output: false,
            mode: Mode::Charge,
        };
        self.connect(Mode::Charge, &c.payload())?;
        Ok(())
    }
}

/// Standard full-charge per-cell voltage for each chemistry (NiMH: raw device value 0).
pub fn default_cell_voltage(t: BatteryType) -> f64 {
    match t {
        BatteryType::Lihv => 4.35,
        BatteryType::Lipo => 4.20,
        BatteryType::Liion => 4.10,
        BatteryType::Life => 3.60,
        BatteryType::Pb => 2.40,
        BatteryType::Nimh => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{RESP_GROUP_ID, build_frame, parse_report};
    use std::collections::VecDeque;

    /// Mock that replies to each command via a closure and records what was sent.
    struct Mock<F: FnMut(u8, &[u8]) -> Option<(u8, Vec<u8>)>> {
        reply: F,
        sent: Vec<(u8, Vec<u8>)>,
        queue: VecDeque<Frame>,
    }

    impl<F: FnMut(u8, &[u8]) -> Option<(u8, Vec<u8>)>> Transport for Mock<F> {
        fn send(&mut self, c: u8, p: &[u8]) -> Result<(), Error> {
            self.sent.push((c, p.to_vec()));
            if let Some((rc, rp)) = (self.reply)(c, p) {
                let mut f = build_frame(rc, &rp).unwrap();
                f[2] = RESP_GROUP_ID;
                self.queue.push_back(parse_report(&f).unwrap());
            }
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> Result<Option<Frame>, Error> {
            Ok(self.queue.pop_front())
        }
    }

    fn state_payload(mode: u8, set_v: u16, set_i: u16, output: u8) -> Vec<u8> {
        let mut p = vec![0, 0, 0];
        p.extend(0u16.to_le_bytes());
        p.extend(set_v.to_le_bytes());
        p.extend(0u16.to_le_bytes());
        p.extend(set_i.to_le_bytes());
        p.extend([0; 8]);
        p.extend(0u16.to_le_bytes());
        p.extend([0, 3, 0, output, mode, 0, 0, 25]);
        p
    }

    #[test]
    fn set_output_requests_remote_then_applies() {
        let mock = Mock {
            reply: |c, _p: &[u8]| match c {
                0xC2 => Some((0xC3, state_payload(0, 1200, 500, 0))),
                0xC8 => Some((0xC9, vec![0])),
                _ => None,
            },
            sent: vec![],
            queue: VecDeque::new(),
        };
        let mut psu = Mp305::new(mock);
        psu.set_output(&OutputSettings {
            voltage: Some(5.0),
            on: Some(true),
            ..Default::default()
        })
        .unwrap();
        let sent = &psu.t.sent;
        // state, remote request (remoteCon=2), apply, state
        assert_eq!(sent[0].0, 0xC2);
        assert_eq!(sent[1], (0xC8, vec![2, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0]));
        // 5.00 V, current preserved at 0.5 A (500 mA), output on
        assert_eq!(
            sent[2],
            (0xC8, vec![1, 0xF4, 0x01, 0xF4, 0x01, 3, 0, 0, 1, 0, 0])
        );
        assert_eq!(sent[3].0, 0xC2);
    }

    #[test]
    fn set_output_refused_by_device() {
        let mock = Mock {
            reply: |c, _p: &[u8]| match c {
                0xC2 => Some((0xC3, state_payload(0, 0, 0, 0))),
                0xC8 => Some((0xC9, vec![1])),
                _ => None,
            },
            sent: vec![],
            queue: VecDeque::new(),
        };
        let mut psu = Mp305::new(mock);
        let e = psu
            .set_output(&OutputSettings {
                on: Some(true),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(e, Error::RemoteRefused));
    }

    #[test]
    fn set_output_rejects_non_dc_and_out_of_range() {
        let mock = Mock {
            reply: |c, _p: &[u8]| (c == 0xC2).then(|| (0xC3, state_payload(3, 0, 0, 0))),
            sent: vec![],
            queue: VecDeque::new(),
        };
        let mut psu = Mp305::new(mock);
        assert!(matches!(
            psu.set_output(&OutputSettings {
                on: Some(true),
                ..Default::default()
            }),
            Err(Error::WrongMode { .. })
        ));
        assert!(matches!(
            psu.set_output(&OutputSettings {
                voltage: Some(31.0),
                ..Default::default()
            }),
            Err(Error::OutOfRange(_))
        ));
    }

    #[test]
    fn timeout_when_no_reply() {
        let mock = Mock {
            reply: |_, _: &[u8]| None,
            sent: vec![],
            queue: VecDeque::new(),
        };
        let mut psu = Mp305::new(mock);
        psu.set_timeout(Duration::from_millis(10));
        assert!(matches!(
            psu.state(),
            Err(Error::Timeout {
                cmd: 0xC2,
                expect: 0xC3
            })
        ));
    }
}
