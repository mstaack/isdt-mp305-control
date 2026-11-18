//! Request payload builders (little-endian, units as the firmware expects).

use crate::types::{BatteryType, Mode};

/// Remote-control field shared by every "connect" command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// Hand control back to the front panel.
    Release = 0,
    /// Apply this command (requires control to have been granted).
    Apply = 1,
    /// Ask the device to grant remote control; other fields are ignored.
    Request = 2,
}

/// 0xC8 — DC output control.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlCommand {
    pub remote: Remote,
    pub set_voltage: f64,
    pub set_current: f64,
    /// Live-apply mask: 1 = V, 2 = I, 3 = both.
    pub real_change: u8,
    pub voltage_slow: u8,
    /// 0 = constant-current limit, 1 = over-current protection trip.
    pub current_over: u8,
    pub output: bool,
    pub mode: Mode,
}

impl Default for ControlCommand {
    fn default() -> Self {
        Self {
            remote: Remote::Apply,
            set_voltage: 0.0,
            set_current: 0.0,
            real_change: 3,
            voltage_slow: 0,
            current_over: 0,
            output: false,
            mode: Mode::Dc,
        }
    }
}

impl ControlCommand {
    pub fn payload(&self) -> Vec<u8> {
        let mut p = vec![self.remote as u8];
        p.extend(((self.set_voltage * 100.0).round() as u16).to_le_bytes());
        p.extend(((self.set_current * 1000.0).round() as u16).to_le_bytes());
        p.extend([
            self.real_change,
            self.voltage_slow,
            self.current_over,
            self.output as u8,
            self.mode.raw(),
            0, // refresh
        ]);
        p
    }
}

/// 0xEE — charge control.
#[derive(Debug, Clone, PartialEq)]
pub struct ChargeCommand {
    pub remote: Remote,
    pub battery_type: BatteryType,
    /// Per-cell target in volts; for NiMH/Cd the raw device value is sent as-is.
    pub cell_voltage: f64,
    pub cells: u8,
    pub current: f64,
    pub output: bool,
    pub mode: Mode,
}

impl Default for ChargeCommand {
    fn default() -> Self {
        Self {
            remote: Remote::Apply,
            battery_type: BatteryType::Lihv,
            cell_voltage: 0.0,
            cells: 1,
            current: 0.0,
            output: false,
            mode: Mode::Charge,
        }
    }
}

impl ChargeCommand {
    pub fn payload(&self) -> Vec<u8> {
        let cap = if self.battery_type == BatteryType::Nimh {
            self.cell_voltage as u16
        } else {
            (self.cell_voltage * 1000.0).round() as u16
        };
        let mut p = vec![self.remote as u8, self.battery_type.raw()];
        p.extend(cap.to_le_bytes());
        p.push(self.cells);
        p.extend(((self.current * 1000.0).round() as u16).to_le_bytes());
        p.extend([self.output as u8, self.mode.raw()]);
        p
    }
}

/// 0xE2 — programmable-mode connect (only used for remote handshake / leaving the mode).
pub fn program_connect(remote: Remote, output: bool, mode: Mode) -> Vec<u8> {
    vec![remote as u8, 0, output as u8, mode.raw()]
}

/// 0xE8 — USB-PD connect (only used for remote handshake / leaving the mode).
pub fn pdo_connect(remote: Remote, output: bool, mode: Mode) -> Vec<u8> {
    vec![remote as u8, 0, 0, 0, output as u8, mode.raw()]
}

/// 0xC6 — system settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSetCommand {
    pub power_limit: u8,
    pub volume: u8,
    pub screen_off: u8,
    pub shutdown: u8,
    pub screen_direction: u8,
    pub slope_steps: u16,
    pub current_over: u16,
    pub usb_line: Option<u16>,
}

impl From<&crate::types::SystemSettings> for SystemSetCommand {
    fn from(s: &crate::types::SystemSettings) -> Self {
        Self {
            power_limit: s.power_limit,
            volume: s.volume,
            screen_off: s.screen_off,
            shutdown: s.shutdown,
            screen_direction: s.screen_direction,
            slope_steps: s.slope_steps,
            current_over: s.current_over,
            usb_line: s.usb_line,
        }
    }
}

impl SystemSetCommand {
    pub fn payload(&self) -> Vec<u8> {
        let mut p = vec![
            self.power_limit,
            self.volume,
            self.screen_off,
            self.shutdown,
            self.screen_direction,
        ];
        p.extend(self.slope_steps.to_le_bytes());
        p.extend(self.current_over.to_le_bytes());
        p.extend([0, 0]); // system_check, recover (factory reset) — never set
        if let Some(u) = self.usb_line {
            p.extend(u.to_le_bytes());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_payload_units() {
        let c = ControlCommand {
            set_voltage: 5.0,
            set_current: 1.0,
            output: true,
            ..Default::default()
        };
        // remote=1, 500 (0x01F4), 1000 (0x03E8), realChange=3, slow, over, output, model, refresh
        assert_eq!(
            c.payload(),
            vec![1, 0xF4, 0x01, 0xE8, 0x03, 3, 0, 0, 1, 0, 0]
        );
    }

    #[test]
    fn charge_payload_layout() {
        let c = ChargeCommand {
            battery_type: BatteryType::Lipo,
            cell_voltage: 4.2,
            cells: 3,
            current: 2.0,
            output: true,
            ..Default::default()
        };
        // <BBHBHBB
        assert_eq!(c.payload(), vec![1, 1, 0x68, 0x10, 3, 0xD0, 0x07, 1, 3]);
    }

    #[test]
    fn system_set_payload_layout() {
        let s = SystemSetCommand {
            power_limit: 90,
            volume: 3,
            screen_off: 0,
            shutdown: 0,
            screen_direction: 0,
            slope_steps: 0x0102,
            current_over: 0x0304,
            usb_line: None,
        };
        assert_eq!(s.payload(), vec![90, 3, 0, 0, 0, 2, 1, 4, 3, 0, 0]);
    }
}
