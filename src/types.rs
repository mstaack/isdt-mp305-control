//! Decoded device responses, with physical units applied.
//! Offsets follow pymp305's `responses.py` (transcribed from WebLink).

use serde::Serialize;

/// Reads past the end yield 0, like WebLink's JS parsers (`undefined & 0xFF === 0`);
/// real frames are sometimes shorter than the maximal layout.
struct Reader<'a> {
    v: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn new(v: &'a [u8], i: usize) -> Self {
        Self { v, i }
    }
    fn at(&self, i: usize) -> u8 {
        self.v.get(i).copied().unwrap_or(0)
    }
    fn u8(&mut self) -> u8 {
        let b = self.at(self.i);
        self.i += 1;
        b
    }
    fn u16(&mut self) -> u16 {
        let b = u16::from_le_bytes([self.at(self.i), self.at(self.i + 1)]);
        self.i += 2;
        b
    }
    fn u32(&mut self) -> u32 {
        let b = u32::from_le_bytes([
            self.at(self.i),
            self.at(self.i + 1),
            self.at(self.i + 2),
            self.at(self.i + 3),
        ]);
        self.i += 4;
        b
    }
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for (k, o) in out.iter_mut().enumerate() {
            *o = self.at(self.i + k);
        }
        self.i += N;
        out
    }
    fn has(&self, n: usize) -> bool {
        self.i + n <= self.v.len()
    }
}

/// The frame length byte `N` (values[0]); optional trailing fields are gated on it.
fn frame_len(v: &[u8]) -> u8 {
    v.first().copied().unwrap_or(0)
}

/// Payload starts after `[N, 0xAA, group, L, cmd]`.
const PAYLOAD_INDEX: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Dc,
    Programmable,
    UsbPd,
    Charge,
    Unknown(u8),
}

impl Mode {
    pub fn from_raw(v: u8) -> Self {
        match v {
            0 => Mode::Dc,
            1 => Mode::Programmable,
            2 => Mode::UsbPd,
            3 => Mode::Charge,
            x => Mode::Unknown(x),
        }
    }
    pub fn raw(self) -> u8 {
        match self {
            Mode::Dc => 0,
            Mode::Programmable => 1,
            Mode::UsbPd => 2,
            Mode::Charge => 3,
            Mode::Unknown(x) => x,
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Dc => f.write_str("DC"),
            Mode::Programmable => f.write_str("programmable"),
            Mode::UsbPd => f.write_str("USB-PD"),
            Mode::Charge => f.write_str("charge"),
            Mode::Unknown(x) => write!(f, "unknown({x})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BatteryType {
    Lihv,
    Lipo,
    Liion,
    Life,
    Pb,
    Nimh,
}

impl BatteryType {
    pub const ALL: [BatteryType; 6] = [
        BatteryType::Lihv,
        BatteryType::Lipo,
        BatteryType::Liion,
        BatteryType::Life,
        BatteryType::Pb,
        BatteryType::Nimh,
    ];
    pub fn from_raw(v: u8) -> Option<Self> {
        Self::ALL.get(v as usize).copied()
    }
    pub fn raw(self) -> u8 {
        Self::ALL.iter().position(|&b| b == self).unwrap() as u8
    }
    pub fn label(self) -> &'static str {
        match self {
            BatteryType::Lihv => "LiHV",
            BatteryType::Lipo => "LiPo",
            BatteryType::Liion => "Li-ion",
            BatteryType::Life => "LiFe",
            BatteryType::Pb => "Pb",
            BatteryType::Nimh => "NiMH/Cd",
        }
    }
}

const ERROR_LIST: [&str; 17] = [
    "output reversed",
    "battery voltage",
    "battery temperature low",
    "battery temperature high",
    "board temperature high",
    "DC output over-current",
    "DC output over-voltage",
    "regulator init failed",
    "DC output voltage",
    "timeout",
    "connection broken",
    "battery over-voltage",
    "battery low",
    "cell node",
    "no battery",
    "capacity",
    "unknown",
];

/// Decode an error bitmask. MP305B maps bits directly; other models (MP305A) remap bits 1–3.
/// Non-charge modes only use the low 9 bits.
pub fn decode_errors(mask: u32, device_name: Option<&str>, charge_mode: bool) -> Vec<String> {
    let width = if charge_mode { 17 } else { 9 };
    let is_b = device_name == Some("MP305B");
    (0..width)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let name = match (is_b, i) {
                (false, 1) | (false, 2) => "unknown",
                (false, 3) => "battery temperature high (A)",
                _ => ERROR_LIST[i],
            };
            name.to_string()
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareInfo {
    pub device_name: String,
    pub device_id: String,
    pub hardware_version: String,
    pub boot_version: String,
    pub firmware_version: String,
}

fn version(b: [u8; 4]) -> String {
    format!("V{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

impl HardwareInfo {
    pub fn parse(v: &[u8]) -> Self {
        let mut r = Reader::new(v, PAYLOAD_INDEX);
        let id: [u8; 8] = r.bytes();
        let hw = r.bytes();
        let boot = r.bytes();
        let app = r.bytes();
        let name: [u8; 10] = r.bytes();
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Self {
            device_name: String::from_utf8_lossy(&name[..end]).trim().to_string(),
            device_id: id.iter().map(|b| format!("{b:02X}")).collect(),
            hardware_version: version(hw),
            boot_version: version(boot),
            firmware_version: version(app),
        }
    }
}

/// Live state (0xC3).
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub out_state: u8,
    pub voltage: f64,
    pub set_voltage: f64,
    pub current: f64,
    pub set_current: f64,
    pub working_time_s: u32,
    pub energy_wh: f64,
    pub power: f64,
    /// 0 = constant-current limit, 1 = over-current protection trip.
    pub current_over: u8,
    pub real_change: u8,
    pub voltage_slow: u8,
    pub output: bool,
    pub mode: Mode,
    pub temperature_c: u8,
    pub error_mask: u16,
    pub errors: Vec<String>,
}

impl State {
    pub fn parse(v: &[u8], device_name: Option<&str>) -> Self {
        let mut r = Reader::new(v, PAYLOAD_INDEX);
        let out_state = r.u8();
        let _battery_state = r.u8();
        let _percentage = r.u8();
        let voltage = r.u16() as f64 / 100.0;
        let set_voltage = r.u16() as f64 / 100.0;
        let current = r.u16() as f64 / 1000.0;
        let set_current = r.u16() as f64 / 1000.0;
        let working_time_s = r.u32();
        let energy_wh = r.u32() as f64 / 1000.0;
        let power = r.u16() as f64 / 100.0;
        let current_over = r.u8();
        let real_change = r.u8();
        let voltage_slow = r.u8();
        let output = r.u8() != 0;
        let mode = Mode::from_raw(r.u8());
        let _voltage_board = r.u8();
        let _current_board = r.u8();
        let temperature_c = r.u8();
        let mut error_mask = 0;
        let mut errors = Vec::new();
        if frame_len(v) > 34 && r.has(2) {
            error_mask = r.u16();
            errors = decode_errors(error_mask as u32, device_name, mode == Mode::Charge);
        }
        Self {
            out_state,
            voltage,
            set_voltage,
            current,
            set_current,
            working_time_s,
            energy_wh,
            power,
            current_over,
            real_change,
            voltage_slow,
            output,
            mode,
            temperature_c,
            error_mask,
            errors,
        }
    }
}

/// System settings (0xC5). Field meanings are as named in WebLink; units are the raw device values.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SystemSettings {
    pub power_limit: u8,
    pub volume: u8,
    pub screen_off: u8,
    pub shutdown: u8,
    pub screen_direction: u8,
    pub slope_steps: u16,
    pub current_over: u16,
    pub usb_line: Option<u16>,
}

impl SystemSettings {
    pub fn parse(v: &[u8]) -> Self {
        let mut r = Reader::new(v, PAYLOAD_INDEX);
        let mut s = Self {
            power_limit: r.u8(),
            volume: r.u8(),
            screen_off: r.u8(),
            shutdown: r.u8(),
            screen_direction: r.u8(),
            slope_steps: r.u16(),
            current_over: r.u16(),
            usb_line: None,
        };
        if frame_len(v) > 14 && r.has(2) {
            s.usb_line = Some(r.u16());
        }
        s
    }
}

/// Charge progress (0xED).
#[derive(Debug, Clone, Serialize)]
pub struct ChargeState {
    pub battery_state: u8,
    pub percentage: u8,
    pub current: f64,
    pub capacity_mah: u32,
    pub battery_type: Option<BatteryType>,
    pub cells: u8,
    pub voltage: f64,
    pub energy_wh: f64,
    pub working_time_s: u32,
    pub power: f64,
    pub charge_full: bool,
    pub output: bool,
    pub mode: Mode,
    pub temperature_c: u8,
    pub error_mask: u32,
    pub errors: Vec<String>,
}

impl ChargeState {
    pub fn parse(v: &[u8], device_name: Option<&str>) -> Self {
        let mut r = Reader::new(v, PAYLOAD_INDEX);
        let mut s = Self {
            battery_state: r.u8(),
            percentage: r.u8(),
            current: r.u16() as f64 / 1000.0,
            capacity_mah: r.u32(),
            battery_type: BatteryType::from_raw(r.u8()),
            cells: r.u8(),
            voltage: r.u16() as f64 / 100.0,
            energy_wh: r.u32() as f64 / 1000.0,
            working_time_s: r.u32(),
            power: r.u16() as f64 / 100.0,
            charge_full: r.u8() != 0,
            output: r.u8() != 0,
            mode: Mode::from_raw(r.u8()),
            temperature_c: r.u8(),
            error_mask: 0,
            errors: Vec::new(),
        };
        if frame_len(v) > 31 && r.has(4) {
            s.error_mask = r.u32();
            s.errors = decode_errors(s.error_mask, device_name, true);
        }
        s
    }
}

/// Charge settings (0xEB): the configured charge and the previous run's results.
#[derive(Debug, Clone, Serialize)]
pub struct ChargeInfo {
    pub battery_type: Option<BatteryType>,
    /// Raw target: per-cell millivolts (NiMH/Cd: raw device value).
    pub capacity_voltage_raw: u16,
    pub cells: u8,
    pub current: f64,
    pub prev_battery_type: Option<BatteryType>,
    pub prev_cells: u8,
    pub prev_capacity_mah: u32,
    pub prev_energy_raw: u32,
    pub prev_working_time_s: u32,
}

impl ChargeInfo {
    pub fn parse(v: &[u8]) -> Self {
        let mut r = Reader::new(v, PAYLOAD_INDEX);
        Self {
            battery_type: BatteryType::from_raw(r.u8()),
            capacity_voltage_raw: r.u16(),
            cells: r.u8(),
            current: r.u16() as f64 / 1000.0,
            prev_battery_type: BatteryType::from_raw(r.u8()),
            prev_cells: r.u8(),
            prev_capacity_mah: r.u32(),
            prev_energy_raw: r.u32(),
            prev_working_time_s: r.u32(),
        }
    }

    /// Target per-cell voltage in volts (not meaningful for NiMH/Cd).
    pub fn cell_voltage(&self) -> f64 {
        self.capacity_voltage_raw as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{build_frame, parse_report};

    fn frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
        parse_report(&build_frame(cmd, payload).unwrap())
            .unwrap()
            .values
    }

    #[test]
    fn state_decode() {
        let mut p = vec![1, 0, 73];
        p.extend(1234u16.to_le_bytes());
        p.extend(2000u16.to_le_bytes());
        p.extend(1500u16.to_le_bytes());
        p.extend(3000u16.to_le_bytes());
        p.extend(3661u32.to_le_bytes());
        p.extend(12345u32.to_le_bytes());
        p.extend(1850u16.to_le_bytes());
        p.extend([1, 0, 0, 1, 0, 0, 0, 27]);
        let st = State::parse(&frame(0xC3, &p), Some("MP305B"));
        assert!((st.voltage - 12.34).abs() < 1e-9);
        assert!((st.set_voltage - 20.0).abs() < 1e-9);
        assert!((st.current - 1.5).abs() < 1e-9);
        assert!((st.set_current - 3.0).abs() < 1e-9);
        assert_eq!(st.working_time_s, 3661);
        assert!((st.energy_wh - 12.345).abs() < 1e-9);
        assert!((st.power - 18.5).abs() < 1e-9);
        assert!(st.output);
        assert_eq!(st.mode, Mode::Dc);
        assert_eq!(st.temperature_c, 27);
        assert!(st.errors.is_empty());

        // with the trailing error word
        p.extend((1u16 << 5).to_le_bytes());
        let st = State::parse(&frame(0xC3, &p), Some("MP305B"));
        assert_eq!(st.errors, vec!["DC output over-current"]);
    }

    #[test]
    fn hardware_info_decode() {
        let mut p = vec![1, 2, 3, 4, 5, 6, 7, 8];
        p.extend([1, 0, 0, 0, 1, 2, 0, 0, 1, 6, 0, 46]);
        p.extend(b"MP305B\0\0\0\0");
        let hi = HardwareInfo::parse(&frame(0xE1, &p));
        assert_eq!(hi.device_name, "MP305B");
        assert_eq!(hi.device_id, "0102030405060708");
        assert_eq!(hi.hardware_version, "V1.0.0.0");
        assert_eq!(hi.boot_version, "V1.2.0.0");
        assert_eq!(hi.firmware_version, "V1.6.0.46");
    }

    #[test]
    fn short_frames_do_not_panic() {
        let st = State::parse(&frame(0xC3, &[1]), None);
        assert_eq!(st.voltage, 0.0);
        let _ = ChargeState::parse(&frame(0xED, &[]), None);
        let _ = HardwareInfo::parse(&[]);
    }

    #[test]
    fn error_decoding_is_model_aware() {
        assert_eq!(
            decode_errors(1 << 3, Some("MP305B"), false),
            vec!["battery temperature high"]
        );
        assert_eq!(
            decode_errors(1 << 1, Some("MP305A"), false),
            vec!["unknown"]
        );
        assert!(decode_errors(1 << 12, Some("MP305B"), false).is_empty());
        assert_eq!(
            decode_errors(1 << 12, Some("MP305B"), true),
            vec!["battery low"]
        );
    }

    #[test]
    fn charge_state_decode() {
        let mut p = vec![2, 55];
        p.extend(2500u16.to_le_bytes());
        p.extend(1200u32.to_le_bytes());
        p.extend([1, 3]);
        p.extend(1180u16.to_le_bytes());
        p.extend(5000u32.to_le_bytes());
        p.extend(600u32.to_le_bytes());
        p.extend(2950u16.to_le_bytes());
        p.extend([0, 1, 3, 30]);
        let cs = ChargeState::parse(&frame(0xED, &p), Some("MP305B"));
        assert_eq!(cs.percentage, 55);
        assert!((cs.current - 2.5).abs() < 1e-9);
        assert_eq!(cs.capacity_mah, 1200);
        assert_eq!(cs.battery_type, Some(BatteryType::Lipo));
        assert_eq!(cs.cells, 3);
        assert!((cs.voltage - 11.8).abs() < 1e-9);
        assert_eq!(cs.mode, Mode::Charge);
        assert_eq!(cs.temperature_c, 30);
    }
}
