//! Binary 4E/3E Q/R clock read wiring (H3): the fixed special devices that
//! hold the CPU clock, read only and never written here.
//!
//! SM210-SM213 are clock data set request, set error, display (Q) or
//! reserved, and read request. SD210 onward holds the clock only after SM213
//! (clock data read request) asks for it:
//!
//! - iQ-R (MELSEC iQ-R CPU module user's manual (application), 4.1 clock
//!   function, and its SD list): SD210-SD216 are year (four digits), month,
//!   day, hour, minute, second and day of week, one BIN word each.
//! - Q (QCPU user's manual SH-080483ENG, SD210-SD213, printed p.545): BCD,
//!   two digits per byte. SD210 is the lower two year digits (upper byte) and
//!   month (lower byte), SD211 day and hour, SD212 minute and second, SD213
//!   the upper two year digits and day of week. The manual's examples are
//!   H9307 (July 1993), H3110 (31st, 10 o'clock), H3548 (35 min 48 s) and
//!   H1905 (1993, Friday).
//!
//! Only these spans are encoded here, with each series' device
//! specification: a 0401 bit read of SM210-SM213 and one 0401 word read of
//! SD210-SD216 (R) or SD210-SD213 (Q). Which CPU may receive them, and how
//! the values are checked, is the guest's decision; responses use
//! `decode_read`.

use alloc::vec::Vec;

use crate::{encode_read, Device, ReadSpan, Route, Series};

/// SM210 (set request), SM211 (set error), SM212 and SM213 (read request).
pub const CLOCK_READ_STATUS_SPAN: ReadSpan = ReadSpan {
    device: Device::SM,
    start: 210,
    count: 4,
};

/// SD210-SD216 of an iQ-R CPU, BIN.
pub const R_CLOCK_DATA_SPAN: ReadSpan = ReadSpan {
    device: Device::SD,
    start: 210,
    count: 7,
};

/// SD210-SD213 of a Q CPU, BCD.
pub const Q_CLOCK_DATA_SPAN: ReadSpan = ReadSpan {
    device: Device::SD,
    start: 210,
    count: 4,
};

/// The clock data span of `series`.
pub const fn clock_data_span(series: Series) -> ReadSpan {
    match series {
        Series::Q => Q_CLOCK_DATA_SPAN,
        Series::R => R_CLOCK_DATA_SPAN,
    }
}

/// 0401 bit-unit read of SM210-SM213 (subcommand 0001 on Q, 0003 on R).
pub fn encode_clock_read_status(route: Route, series: Series) -> Vec<u8> {
    encode_read(route, series, CLOCK_READ_STATUS_SPAN).expect("fixed SM210-SM213 span")
}

/// 0401 word-unit read of the clock data (subcommand 0000 on Q, 0002 on R).
pub fn encode_clock_data_read(route: Route, series: Series) -> Vec<u8> {
    encode_read(route, series, clock_data_span(series)).expect("fixed SD210 span")
}

/// Clock fields as read, not yet range-checked. The day of the week is
/// not kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockData {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// SD210-SD216 of an iQ-R CPU as BIN words; SD216 (day of week) is ignored.
pub fn decode_r_clock_data(words: [u16; 7]) -> ClockData {
    ClockData {
        year: words[0],
        month: words[1],
        day: words[2],
        hour: words[3],
        minute: words[4],
        second: words[5],
    }
}

/// A Q clock word whose year, date or time byte is not two BCD digits;
/// the value is the SD number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotBcd(pub u16);

fn bcd(byte: u8) -> Option<u16> {
    let (high, low) = (byte >> 4, byte & 0x0f);
    (high <= 9 && low <= 9).then_some(u16::from(high) * 10 + u16::from(low))
}

/// SD210-SD213 of a Q CPU. The day-of-week byte (lower byte of SD213) is
/// not decoded or checked.
pub fn decode_q_clock_data(words: [u16; 4]) -> Result<ClockData, NotBcd> {
    // Upper byte, lower byte of SD210 + index, each two BCD digits.
    let pair = |index: u16| -> Result<(u16, u16), NotBcd> {
        let [low, high] = words[usize::from(index)].to_le_bytes();
        let sd = NotBcd(210 + index);
        Ok((bcd(high).ok_or(sd)?, bcd(low).ok_or(sd)?))
    };
    let (year_low, month) = pair(0)?;
    let (day, hour) = pair(1)?;
    let (minute, second) = pair(2)?;
    let [_, century] = words[3].to_le_bytes();
    let century = bcd(century).ok_or(NotBcd(213))?;
    Ok(ClockData {
        year: century * 100 + year_low,
        month,
        day,
        hour,
        minute,
        second,
    })
}
