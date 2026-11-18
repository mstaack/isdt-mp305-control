//! Byte transports. USB-HID today; a BLE transport can implement the same trait later.

use std::time::Duration;

use crate::Error;
use crate::protocol::{self, Frame};

pub trait Transport {
    /// Send one command frame.
    fn send(&mut self, cmd: u8, payload: &[u8]) -> Result<(), Error>;
    /// Receive the next frame, or `None` if nothing arrived within `timeout`.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, Error>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceEntry {
    pub path: String,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
}

pub struct HidTransport {
    dev: hidapi::HidDevice,
}

impl HidTransport {
    pub fn list() -> Result<Vec<DeviceEntry>, Error> {
        let api = hidapi::HidApi::new()?;
        Ok(api
            .device_list()
            .filter(|d| d.vendor_id() == protocol::VENDOR_ID)
            .map(|d| DeviceEntry {
                path: d.path().to_string_lossy().into_owned(),
                product: d.product_string().map(str::to_owned),
                manufacturer: d.manufacturer_string().map(str::to_owned),
                serial: d
                    .serial_number()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                product_id: d.product_id(),
                usage_page: d.usage_page(),
                usage: d.usage(),
            })
            .collect())
    }

    /// Open the device at `path`, or the first MP305 HID interface (preferring usage 0x01/0x04).
    pub fn open(path: Option<&str>) -> Result<Self, Error> {
        let api = hidapi::HidApi::new()?;
        let dev = match path {
            Some(p) => api.open_path(&std::ffi::CString::new(p).map_err(|_| Error::NotFound)?)?,
            None => {
                let candidates: Vec<_> = api
                    .device_list()
                    .filter(|d| d.vendor_id() == protocol::VENDOR_ID)
                    .collect();
                let chosen = candidates
                    .iter()
                    .find(|d| {
                        d.usage_page() == protocol::HID_USAGE_PAGE
                            && d.usage() == protocol::HID_USAGE
                    })
                    .or_else(|| candidates.first())
                    .ok_or(Error::NotFound)?;
                chosen.open_device(&api)?
            }
        };
        dev.set_blocking_mode(true)?;
        let mut t = Self { dev };
        t.drain();
        Ok(t)
    }

    /// Discard any stale input reports queued from before we opened.
    fn drain(&mut self) {
        let mut buf = [0u8; protocol::REPORT_SIZE + 1];
        while let Ok(n) = self.dev.read_timeout(&mut buf, 0) {
            if n == 0 {
                break;
            }
        }
    }
}

impl Transport for HidTransport {
    fn send(&mut self, cmd: u8, payload: &[u8]) -> Result<(), Error> {
        let reports = protocol::build_reports(cmd, payload)?;
        let n = reports.len();
        for (i, r) in reports.iter().enumerate() {
            self.dev.write(r)?;
            if i + 1 < n {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, Error> {
        let mut buf = [0u8; protocol::REPORT_SIZE + 1];
        let n = self
            .dev
            .read_timeout(&mut buf, timeout.as_millis().min(i32::MAX as u128) as i32)?;
        if n == 0 {
            return Ok(None);
        }
        if std::env::var_os("MP305_DEBUG").is_some() {
            eprintln!("<< {}", hex(&buf[..n]));
        }
        Ok(protocol::parse_report(&buf[..n]))
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
