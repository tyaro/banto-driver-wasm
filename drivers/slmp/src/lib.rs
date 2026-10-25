//! Binary 4E SLMP batch-read (0401) framing for standard Q and iQ-R.
//! This core does no I/O, configured-range lookup, or writes.
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

mod type_name;
pub use type_name::{decode_type_name, encode_type_name, ModelFamily, TypeName};

/// Wire layout only. The actual CPU model and its supported devices are checked later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Series {
    Q,
    R,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Device {
    M,
    X,
    D,
    W,
    SD,
}

impl Device {
    const fn code(self) -> u8 {
        match self {
            Self::M => 0x90,
            Self::X => 0x9c,
            Self::D => 0xa8,
            Self::W => 0xb4,
            Self::SD => 0xa9,
        }
    }

    const fn is_bit(self) -> bool {
        matches!(self, Self::M | Self::X)
    }
}

/// `start` is a numeric device number, already parsed in the device's radix.
/// `count` is in device points (bits for M/X, words for D/W/SD).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadSpan {
    pub device: Device,
    pub start: u32,
    pub count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Route {
    pub serial: u16,
    pub network: u8,
    pub pc: u8,
    pub io: u16,
    pub station: u8,
    pub timer: u16,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            serial: 1,
            network: 0,
            pc: 0xff,
            io: 0x03ff,
            station: 0,
            timer: 0x0010,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Count,
    DeviceNumber,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseError {
    Framing,
    /// The 0101 name field is not a nonempty printable ASCII model name.
    MalformedName,
    /// Nonzero SLMP end code; the PLC rejected the request.
    EndCode(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Values {
    Bits(Vec<bool>),
    Words(Vec<u16>),
}

fn check_span(series: Series, span: ReadSpan) -> Result<(), RequestError> {
    let max_count = if span.device.is_bit() { 7168 } else { 960 };
    if span.count == 0 || span.count > max_count {
        return Err(RequestError::Count);
    }
    let last = span
        .start
        .checked_add(u32::from(span.count - 1))
        .ok_or(RequestError::DeviceNumber)?;
    if series == Series::Q && last > 0x00ff_ffff {
        return Err(RequestError::DeviceNumber);
    }
    Ok(())
}

fn request_head(route: Route, data_length: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(27);
    out.extend_from_slice(&[0x54, 0x00]);
    out.extend_from_slice(&route.serial.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.push(route.network);
    out.push(route.pc);
    out.extend_from_slice(&route.io.to_le_bytes());
    out.push(route.station);
    out.extend_from_slice(&data_length.to_le_bytes());
    out.extend_from_slice(&route.timer.to_le_bytes());
    out
}

/// Build one complete binary 4E batch-read request (command 0401).
pub fn encode_read(route: Route, series: Series, span: ReadSpan) -> Result<Vec<u8>, RequestError> {
    check_span(series, span)?;
    let mut out = request_head(route, if series == Series::Q { 12 } else { 14 });
    out.extend_from_slice(&0x0401u16.to_le_bytes());
    let subcommand = match (series, span.device.is_bit()) {
        (Series::Q, false) => 0,
        (Series::Q, true) => 1,
        (Series::R, false) => 2,
        (Series::R, true) => 3,
    };
    out.extend_from_slice(&u16::to_le_bytes(subcommand));
    let number = span.start.to_le_bytes();
    out.extend_from_slice(&number[..if series == Series::Q { 3 } else { 4 }]);
    out.push(span.device.code());
    if series == Series::R {
        out.push(0);
    }
    out.extend_from_slice(&span.count.to_le_bytes());
    Ok(out)
}

/// Parse a complete response to this request. The Hub owns stream framing and I/O.
/// Even PLC refusals must echo the request's route and serial number.
fn response_data(route: Route, frame: &[u8]) -> Result<&[u8], ResponseError> {
    if frame.len() < 15
        || frame[0..2] != [0xd4, 0]
        || frame[2..4] != route.serial.to_le_bytes()
        || frame[4..6] != [0, 0]
        || frame[6] != route.network
        || frame[7] != route.pc
        || frame[8..10] != route.io.to_le_bytes()
        || frame[10] != route.station
        || usize::from(u16::from_le_bytes([frame[11], frame[12]])) != frame.len() - 13
    {
        return Err(ResponseError::Framing);
    }
    let end_code = u16::from_le_bytes([frame[13], frame[14]]);
    if end_code != 0 {
        return Err(ResponseError::EndCode(end_code));
    }
    Ok(&frame[15..])
}

pub fn decode_read(route: Route, span: ReadSpan, frame: &[u8]) -> Result<Values, ResponseError> {
    let data = response_data(route, frame)?;
    if span.device.is_bit() {
        if data.len() != usize::from(span.count).div_ceil(2) {
            return Err(ResponseError::Framing);
        }
        let mut bits = Vec::with_capacity(usize::from(span.count));
        for (index, byte) in data.iter().copied().enumerate() {
            let high = byte >> 4;
            let low = byte & 0x0f;
            if high > 1 || low > 1 || (index == data.len() - 1 && span.count % 2 == 1 && low != 0) {
                return Err(ResponseError::Framing);
            }
            bits.push(high == 1);
            if bits.len() < usize::from(span.count) {
                bits.push(low == 1);
            }
        }
        Ok(Values::Bits(bits))
    } else {
        if data.len() != usize::from(span.count) * 2 {
            return Err(ResponseError::Framing);
        }
        Ok(Values::Words(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
        ))
    }
}
