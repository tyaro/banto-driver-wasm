//! Binary 4E/3E iQ-R clock set wiring (H): the fixed special devices of the
//! CPU clock (MELSEC iQ-R CPU module user's manual (application), 4.1 clock
//! function, printed pp.118-119, and its SD/SM lists).
//!
//! SD210-SD216 hold year (four digits), month, day, hour, minute, second and
//! day of week (0 = Sunday .. 6 = Saturday), one BIN word each. When SM210
//! (clock data set request) turns OFF to ON, the CPU takes SD210-SD216 into
//! its clock and turns SM210 OFF again; out-of-range data turns SM211 (clock
//! data set error) ON instead and is not taken.
//!
//! Only these devices are encoded here, with the iQ-R device specification
//! (four-byte number, two-byte code SM 0091H / SD 00A9H, decimal numbers):
//! a 0401 bit read of SM210-SM211, a 1401 word write of SD210-SD216 and a
//! 1401 bit write of SM210 ON. Which CPU may receive them and in what order
//! is the guest's decision; responses use `decode_read` and
//! `decode_write_ack`.

use alloc::vec::Vec;

use crate::{encode_read, request_head, Device, ReadSpan, Route, Series};

/// SM210 (clock data set request) and SM211 (clock data set error).
pub const CLOCK_STATUS_SPAN: ReadSpan = ReadSpan {
    device: Device::SM,
    start: 210,
    count: 2,
};

const SD_CLOCK_DATA: u32 = 210;
const SM_CLOCK_SET_REQUEST: u32 = 210;

/// 0401 bit-unit read (subcommand 0003) of SM210 and SM211.
pub fn encode_r_clock_status_read(route: Route) -> Vec<u8> {
    encode_read(route, Series::R, CLOCK_STATUS_SPAN).expect("fixed SM210-SM211 span")
}

/// 1401 word-unit write (subcommand 0002) of SD210-SD216: year, month, day,
/// hour, minute, second, day of week. Values are sent as given.
pub fn encode_r_clock_data_write(route: Route, words: [u16; 7]) -> Vec<u8> {
    // Monitoring timer, command, subcommand, device, count, then the data.
    let mut out = request_head(route, 14 + 7 * 2);
    out.extend_from_slice(&0x1401u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&SD_CLOCK_DATA.to_le_bytes());
    out.extend_from_slice(&[0xa9, 0]);
    out.extend_from_slice(&7u16.to_le_bytes());
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// 1401 bit-unit write (subcommand 0003) of SM210 = ON, one point.
pub fn encode_r_clock_set_request(route: Route) -> Vec<u8> {
    let mut out = request_head(route, 14 + 1);
    out.extend_from_slice(&0x1401u16.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&SM_CLOCK_SET_REQUEST.to_le_bytes());
    out.extend_from_slice(&[0x91, 0]);
    out.extend_from_slice(&1u16.to_le_bytes());
    // One point: ON in the high nibble, the unused low nibble zero.
    out.push(0x10);
    out
}
