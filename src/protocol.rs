//! Wire framing for the ISDT MP305 over USB-HID. Pure functions, no I/O.
//!
//! Frame layout: `[N, 0xAA, 0x12, L, cmd, payload…, checksum]` where `N` is the frame
//! length minus one and `L = 1 + payload.len()`. Every `0xAA` in the data region
//! (index > 5) is doubled on the wire, and the checksum is the byte-sum of index 2..last
//! with a doubled `0xAA` counted once. Ported from pymp305 (MIT), which follows WebLink.

pub const VENDOR_ID: u16 = 0x28E9;
pub const HID_USAGE_PAGE: u16 = 0x01;
pub const HID_USAGE: u16 = 0x04;
pub const REPORT_ID: u8 = 0x01;
/// Output report payload size (excluding the report-id byte).
pub const REPORT_SIZE: usize = 64;

pub const FRAME_START: u8 = 0xAA;
/// Host → device command group.
pub const GROUP_ID: u8 = 0x12;
/// Device → host response group over USB-HID.
pub const RESP_GROUP_ID: u8 = 0x21;
/// One length byte caps a frame at 254 payload bytes.
pub const MAX_FRAME_PAYLOAD: usize = 0xFF - 1;

pub mod cmd {
    pub const HW_INFO: u8 = 0xE0;
    pub const STATE_INFO: u8 = 0xC2;
    pub const SYS_GET: u8 = 0xC4;
    pub const SYS_SET: u8 = 0xC6;
    pub const CONTROL: u8 = 0xC8;
    pub const CHARGE_SEARCH: u8 = 0xEA;
    pub const CHARGE_INFO: u8 = 0xEC;
    pub const CHARGE_CONTROL: u8 = 0xEE;
    pub const PROGRAM_CONNECT: u8 = 0xE2;
    pub const PDO_CONNECT: u8 = 0xE8;
}

pub mod resp {
    pub const HW_INFO: u8 = 0xE1;
    pub const STATE: u8 = 0xC3;
    pub const SYS: u8 = 0xC5;
    pub const SYS_SET: u8 = 0xC7;
    pub const CONTROL: u8 = 0xC9;
    pub const CHARGE_INFO: u8 = 0xEB;
    pub const CHARGE: u8 = 0xED;
    pub const CHARGE_CONTROL: u8 = 0xEF;
    pub const PROGRAM_CONNECT: u8 = 0xE3;
    pub const PDO_CONNECT: u8 = 0xE9;
}

/// A parsed, de-stuffed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub cmd: u8,
    /// Bytes after the command byte, excluding the checksum.
    pub payload: Vec<u8>,
    /// Full de-stuffed frame `[N, 0xAA, group, L, cmd, …, checksum]`; decoders index into this.
    pub values: Vec<u8>,
}

impl Frame {
    /// Checksum the frame claims vs. the one computed over its contents.
    pub fn checksum_ok(&self) -> bool {
        let v = &self.values;
        if v.len() < 3 {
            return false;
        }
        let sum = v[2..v.len() - 1]
            .iter()
            .fold(0u8, |a, &b| a.wrapping_add(b));
        sum == v[v.len() - 1]
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "payload for command 0x{cmd:02X} is {len} bytes; a frame carries at most {MAX_FRAME_PAYLOAD}"
)]
pub struct PayloadTooLong {
    pub cmd: u8,
    pub len: usize,
}

fn add_0xaa(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    for (i, &b) in data.iter().enumerate() {
        out.push(b);
        if b == FRAME_START && i > 5 {
            out.push(FRAME_START);
        }
    }
    out
}

fn finalize(mut a: Vec<u8>) -> Vec<u8> {
    let n = a.len();
    a[0] = (n - 1) as u8;
    let mut total: u8 = 0;
    let mut prev_is_aa = false;
    for &v in &a[2..n - 1] {
        let cur_is_aa = v == FRAME_START;
        if cur_is_aa && prev_is_aa {
            prev_is_aa = false;
            continue;
        }
        total = total.wrapping_add(v);
        prev_is_aa = cur_is_aa;
    }
    a[n - 1] = total;
    if total == FRAME_START {
        a.push(FRAME_START);
        a[0] = a[0].wrapping_add(1);
    }
    a
}

/// Build a complete frame (no report id) for `cmd` + `payload`.
pub fn build_frame(cmd: u8, payload: &[u8]) -> Result<Vec<u8>, PayloadTooLong> {
    if payload.len() > MAX_FRAME_PAYLOAD {
        return Err(PayloadTooLong {
            cmd,
            len: payload.len(),
        });
    }
    let mut body = vec![0x00, FRAME_START, GROUP_ID, (1 + payload.len()) as u8, cmd];
    body.extend_from_slice(payload);
    body.push(0x00);
    Ok(finalize(add_0xaa(&body)))
}

/// HID output report(s) for `cmd` + `payload`, each starting with the report id.
///
/// Frames up to 63 bytes go in one zero-padded report. Longer frames are split into
/// 61-byte fragments, each prefixed with its own length byte and sent unpadded (that is
/// what the firmware's reassembly expects).
pub fn build_reports(cmd: u8, payload: &[u8]) -> Result<Vec<Vec<u8>>, PayloadTooLong> {
    let frame = build_frame(cmd, payload)?;
    if frame.len() < REPORT_SIZE {
        let mut buf = Vec::with_capacity(REPORT_SIZE + 1);
        buf.push(REPORT_ID);
        buf.extend_from_slice(&frame);
        buf.resize(REPORT_SIZE + 1, 0);
        return Ok(vec![buf]);
    }
    let mut reports = Vec::new();
    for (i, chunk) in frame.chunks(61).enumerate() {
        let mut pkt = if i == 0 {
            chunk.to_vec()
        } else {
            [&[0u8][..], chunk].concat()
        };
        pkt[0] = (pkt.len() - 1) as u8;
        let mut r = vec![REPORT_ID];
        r.extend_from_slice(&pkt);
        reports.push(r);
    }
    Ok(reports)
}

/// De-stuff and parse a raw HID input report (with or without the report-id byte,
/// trailing zero padding allowed). Returns `None` when no frame header is found.
pub fn parse_report(raw: &[u8]) -> Option<Frame> {
    let pos = raw
        .windows(2)
        .position(|w| w[0] == FRAME_START && (w[1] == GROUP_ID || w[1] == RESP_GROUP_ID))?;
    if pos == 0 {
        return None; // the length byte must precede the header
    }
    let mut values: Vec<u8> = Vec::with_capacity(raw.len());
    let mut prev_is_aa = false;
    for &b in &raw[pos - 1..] {
        let cur_is_aa = b == FRAME_START;
        if cur_is_aa && prev_is_aa {
            if let Some(n) = values.first_mut() {
                *n = n.wrapping_sub(1);
            }
            prev_is_aa = false;
        } else {
            values.push(b);
            prev_is_aa = cur_is_aa;
        }
    }
    if values.len() < 6 {
        return None;
    }
    let n = values[0] as usize;
    if n + 1 >= 6 && n < values.len() {
        values.truncate(n + 1);
    }
    let cmd = values[4];
    let payload = values[5..values.len() - 1].to_vec();
    Some(Frame {
        cmd,
        payload,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_no_payload_frames() {
        // Vectors from pymp305's test_protocol.py (which match WebLink).
        assert_eq!(
            build_frame(0xE0, &[]).unwrap(),
            vec![0x05, 0xAA, 0x12, 0x01, 0xE0, 0xF3]
        );
        assert_eq!(
            build_frame(0xC2, &[]).unwrap(),
            vec![0x05, 0xAA, 0x12, 0x01, 0xC2, 0xD5]
        );
        assert_eq!(
            build_frame(0xC4, &[]).unwrap(),
            vec![0x05, 0xAA, 0x12, 0x01, 0xC4, 0xD7]
        );
    }

    #[test]
    fn report_is_padded_with_report_id() {
        let r = build_reports(0xE0, &[]).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].len(), REPORT_SIZE + 1);
        assert_eq!(r[0][0], REPORT_ID);
        assert_eq!(&r[0][1..7], &[0x05, 0xAA, 0x12, 0x01, 0xE0, 0xF3]);
        assert!(r[0][7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_with_and_without_report_id_and_padding() {
        let f = build_frame(0xE0, &[]).unwrap();
        assert_eq!(parse_report(&f).unwrap().cmd, 0xE0);
        let mut with_id = vec![0x01];
        with_id.extend_from_slice(&f);
        assert_eq!(parse_report(&with_id).unwrap().cmd, 0xE0);
        with_id.extend_from_slice(&[0; 50]);
        let p = parse_report(&with_id).unwrap();
        assert_eq!(p.cmd, 0xE0);
        assert!(p.payload.is_empty());
        assert!(p.checksum_ok());
    }

    #[test]
    fn stuffing_roundtrip() {
        let payload = [0x10, 0xAA, 0x20, 0xAA, 0x30];
        let f = build_frame(0x55, &payload).unwrap();
        assert_eq!(f.iter().filter(|&&b| b == 0xAA).count(), 1 + 2 * 2);
        let p = parse_report(&f).unwrap();
        assert_eq!(p.cmd, 0x55);
        assert_eq!(p.payload, payload);
        assert!(p.checksum_ok());
    }

    #[test]
    fn response_group_parses() {
        let mut f = build_frame(0xC3, &[1, 2, 3]).unwrap();
        f[2] = RESP_GROUP_ID;
        let p = parse_report(&f).unwrap();
        assert_eq!(p.cmd, 0xC3);
        assert_eq!(p.payload, vec![1, 2, 3]);
    }

    #[test]
    fn long_frames_fragment() {
        let payload = vec![0x11; 100];
        let r = build_reports(0xDA, &payload).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0][1] as usize, r[0].len() - 2);
        assert_eq!(r[1][1] as usize, r[1].len() - 2);
        assert!(build_frame(0xDA, &vec![0; 255]).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_report(&[0; 64]).is_none());
        assert!(parse_report(&[0xAA, 0x12, 0x01]).is_none());
    }
}
