//! Binary 4E or 3E SLMP read (0401) and bounded Q/R word or bit write (1401) framing.
//! This core does no I/O or configured-range lookup; the guest gates writes.
//! 3E has no serial number, so the caller must match its responses by order.
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

mod type_name;
pub use type_name::{decode_type_name, encode_type_name, ModelFamily, TypeName};

mod sd_basic;
pub use sd_basic::{basic_sd_count, plan_basic_sd, BasicArea, BasicEvidence, PointCount};

mod q_zr;
pub use q_zr::{decode_q_zr_count, plan_q_zr, QZrEvidence};

mod q_extension;
pub use q_extension::{
    decode_q_extension_count, decode_q_w_total, plan_q_extension, plan_q_w_total, QExtensionArea,
    QExtensionEvidence,
};

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
    Y,
    B,
    L,
    F,
    V,
    SB,
    SM,
    D,
    W,
    SW,
    SD,
    TS,
    TC,
    TN,
    SS,
    SC,
    SN,
    STS,
    STC,
    STN,
    CS,
    CC,
    CN,
    /// Logical contacts/coils read through the four-word LTN response.
    LTS,
    LTC,
    LSTS,
    LSTC,
    LTN,
    LSTN,
    LCN,
    LCS,
    LCC,
    ZR,
    /// Read-only in the guest: step relay (bit), index registers and refresh data.
    S,
    Z,
    RD,
    /// Unit/CPU buffer memory (`U<h>\G<n>`), read through the 0401 device
    /// extension. Its module number is not part of `ReadSpan`; use `UnitBufferSpan`.
    UG,
}

impl Device {
    const fn code(self) -> u8 {
        match self {
            Self::M => 0x90,
            Self::X => 0x9c,
            Self::Y => 0x9d,
            Self::B => 0xa0,
            Self::L => 0x92,
            Self::F => 0x93,
            Self::V => 0x94,
            Self::SB => 0xa1,
            Self::SM => 0x91,
            Self::D => 0xa8,
            Self::W => 0xb4,
            Self::SW => 0xb5,
            Self::SD => 0xa9,
            Self::TS => 0xc1,
            Self::TC => 0xc0,
            Self::TN => 0xc2,
            Self::SS => 0xc7,
            Self::SC => 0xc6,
            Self::SN => 0xc8,
            Self::STS => 0xc7,
            Self::STC => 0xc6,
            Self::STN => 0xc8,
            Self::CS => 0xc4,
            Self::CC => 0xc3,
            Self::CN => 0xc5,
            Self::LTS | Self::LTC | Self::LTN => 0x52,
            Self::LSTS | Self::LSTC | Self::LSTN => 0x5a,
            Self::LCN => 0x56,
            Self::LCS => 0x55,
            Self::LCC => 0x54,
            Self::ZR => 0xb0,
            Self::S => 0x98,
            Self::Z => 0xcc,
            Self::RD => 0x2c,
            Self::UG => 0xab,
        }
    }

    pub const fn is_bit(self) -> bool {
        matches!(
            self,
            Self::M
                | Self::X
                | Self::Y
                | Self::B
                | Self::L
                | Self::F
                | Self::V
                | Self::SB
                | Self::SM
                | Self::TS
                | Self::TC
                | Self::SS
                | Self::SC
                | Self::STS
                | Self::STC
                | Self::CS
                | Self::CC
                | Self::LCS
                | Self::LCC
                | Self::S
        )
    }
}

/// `start` is a numeric device number, already parsed in the device's radix.
/// `count` is the 0401 wire access-point count: bits for bit-unit requests,
/// words for word-unit requests. An LTN/LSTN logical timer consumes four
/// wire words, while an LCN logical counter consumes two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadSpan {
    pub device: Device,
    pub start: u32,
    pub count: u16,
}

/// Binary frame header variant. Command and data fields are the same in both.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Frame {
    /// Subheader 5400/D400 with a serial number and two reserved bytes.
    #[default]
    E4,
    /// Subheader 5000/D000 without a serial number; responses are matched by order.
    E3,
}

impl Frame {
    /// Response bytes up to and including the end code.
    pub const fn response_header_len(self) -> usize {
        match self {
            Self::E4 => 15,
            Self::E3 => 11,
        }
    }

    /// Offset of the little-endian u16 response data length.
    pub const fn length_offset(self) -> usize {
        self.response_header_len() - 4
    }

    /// Complete response length minus the length field value.
    pub const fn length_adjust(self) -> usize {
        self.length_offset() + 2
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Route {
    pub frame: Frame,
    /// Sent and echoed by 4E only; 3E ignores it.
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
            frame: Frame::E4,
            serial: 1,
            network: 0,
            pc: 0xff,
            io: 0x03ff,
            station: 0,
            timer: 0x0010,
        }
    }
}

/// A word read of unit or CPU buffer memory. `module` is the upper three hex
/// digits of the unit's first I/O number (`U<h>`): 0..=0xFF on Q and R, plus
/// 0x3E0..=0x3E3 (CPU buffer memory of CPU 1-4) on R only. `start` is the
/// decimal buffer address, `count` the number of words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnitBufferSpan {
    pub module: u16,
    pub start: u32,
    pub count: u16,
}

impl UnitBufferSpan {
    /// The response layout is that of a normal word read, so decoding can use
    /// `decode_read` with this span.
    pub const fn read_span(self) -> ReadSpan {
        ReadSpan {
            device: Device::UG,
            start: self.start,
            count: self.count,
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
    if span.device == Device::UG {
        // Buffer memory needs a module; use encode_read_unit_buffer.
        return Err(RequestError::DeviceNumber);
    }
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
    match route.frame {
        Frame::E4 => {
            out.extend_from_slice(&[0x54, 0x00]);
            out.extend_from_slice(&route.serial.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        Frame::E3 => out.extend_from_slice(&[0x50, 0x00]),
    }
    out.push(route.network);
    out.push(route.pc);
    out.extend_from_slice(&route.io.to_le_bytes());
    out.push(route.station);
    out.extend_from_slice(&data_length.to_le_bytes());
    out.extend_from_slice(&route.timer.to_le_bytes());
    out
}

/// Build one complete binary 4E or 3E batch-read request (command 0401).
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

/// Build one binary 4E or 3E batch word read of unit/CPU buffer memory
/// (command 0401, device-extension subcommand 0080 on Q and 0082 on R).
pub fn encode_read_unit_buffer(
    route: Route,
    series: Series,
    span: UnitBufferSpan,
) -> Result<Vec<u8>, RequestError> {
    if span.count == 0 || span.count > 960 {
        return Err(RequestError::Count);
    }
    let cpu_buffer = matches!(span.module, 0x3e0..=0x3e3);
    let module_ok = match series {
        Series::Q => span.module <= 0xff,
        Series::R => span.module <= 0xff || cpu_buffer,
    };
    let last = span
        .start
        .checked_add(u32::from(span.count - 1))
        .ok_or(RequestError::DeviceNumber)?;
    if !module_ok || (series == Series::Q && last > 0x00ff_ffff) {
        return Err(RequestError::DeviceNumber);
    }
    // Timer, command and subcommand, then the extended device specification.
    let mut out = request_head(route, if series == Series::Q { 19 } else { 21 });
    out.extend_from_slice(&0x0401u16.to_le_bytes());
    out.extend_from_slice(
        &(if series == Series::Q {
            0x0080u16
        } else {
            0x0082
        })
        .to_le_bytes(),
    );
    out.extend_from_slice(&[0, 0]);
    let number = span.start.to_le_bytes();
    out.extend_from_slice(&number[..if series == Series::Q { 3 } else { 4 }]);
    out.push(Device::UG.code());
    if series == Series::R {
        out.push(0);
    }
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&span.module.to_le_bytes());
    // FAH marks the iQ-R CPU buffer memory; every unit module uses F8H.
    out.push(if cpu_buffer { 0xfa } else { 0xf8 });
    out.extend_from_slice(&span.count.to_le_bytes());
    Ok(out)
}

/// Build one binary 4E or 3E iQ-R word write for contiguous D or W points.
/// The caller must establish the CPU model and configured area range first.
pub fn encode_r_dw_words(
    route: Route,
    device: Device,
    number: u32,
    words: &[u16],
) -> Result<Vec<u8>, RequestError> {
    if !matches!(device, Device::D | Device::W) {
        return Err(RequestError::DeviceNumber);
    }
    let count = u16::try_from(words.len()).map_err(|_| RequestError::Count)?;
    check_span(
        Series::R,
        ReadSpan {
            device,
            start: number,
            count,
        },
    )?;
    let data_length = 14 + count * 2; // 960 words fit in the length field.
    let mut out = request_head(route, data_length);
    out.extend_from_slice(&0x1401u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&number.to_le_bytes());
    out.extend_from_slice(&[device.code(), 0]);
    out.extend_from_slice(&count.to_le_bytes());
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    Ok(out)
}

pub fn encode_r_d_words(route: Route, number: u32, words: &[u16]) -> Result<Vec<u8>, RequestError> {
    encode_r_dw_words(route, Device::D, number, words)
}

/// Preserve the original single-point API for its existing callers.
pub fn encode_r_d_write(route: Route, number: u32, value: u16) -> Vec<u8> {
    encode_r_d_words(route, number, &[value]).expect("one D word fits a request")
}

/// Build one binary 4E or 3E Q word write for contiguous D or W points.
/// The caller must establish the exact CPU model and configured area span first.
pub fn encode_q_dw_words(
    route: Route,
    device: Device,
    number: u32,
    words: &[u16],
) -> Result<Vec<u8>, RequestError> {
    if !matches!(device, Device::D | Device::W) {
        return Err(RequestError::DeviceNumber);
    }
    let count = u16::try_from(words.len()).map_err(|_| RequestError::Count)?;
    check_span(
        Series::Q,
        ReadSpan {
            device,
            start: number,
            count,
        },
    )?;
    let mut out = request_head(route, 12 + count * 2);
    out.extend_from_slice(&0x1401u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // Q word-unit subcommand
    out.extend_from_slice(&number.to_le_bytes()[..3]); // 24-bit Q address
    out.push(device.code());
    out.extend_from_slice(&count.to_le_bytes());
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    Ok(out)
}

pub fn encode_q_d_words(route: Route, number: u32, words: &[u16]) -> Result<Vec<u8>, RequestError> {
    encode_q_dw_words(route, Device::D, number, words)
}

/// Build one binary 4E or 3E bit-unit write. Model and configured point counts must
/// be checked by the caller before this wire-only function is used.
pub fn encode_bits(
    route: Route,
    series: Series,
    device: Device,
    number: u32,
    bits: &[bool],
) -> Result<Vec<u8>, RequestError> {
    if !matches!(device, Device::M | Device::Y | Device::L | Device::B) {
        return Err(RequestError::DeviceNumber);
    }
    let count = u16::try_from(bits.len()).map_err(|_| RequestError::Count)?;
    check_span(
        series,
        ReadSpan {
            device,
            start: number,
            count,
        },
    )?;
    let bit_bytes = count.div_ceil(2);
    let data_length = if series == Series::Q { 12 } else { 14 } + bit_bytes;
    let mut out = request_head(route, data_length);
    out.extend_from_slice(&0x1401u16.to_le_bytes());
    out.extend_from_slice(&(if series == Series::Q { 1u16 } else { 3u16 }).to_le_bytes());
    let address = number.to_le_bytes();
    out.extend_from_slice(&address[..if series == Series::Q { 3 } else { 4 }]);
    out.push(device.code());
    if series == Series::R {
        out.push(0);
    }
    out.extend_from_slice(&count.to_le_bytes());
    for pair in bits.chunks(2) {
        out.push(
            (if pair[0] { 0x10 } else { 0 }) | (if pair.get(1) == Some(&true) { 1 } else { 0 }),
        );
    }
    Ok(out)
}

/// Preserve the separately gated Q25PRH four-word test frame.
pub fn encode_q25prh_d3010_3013_test_write(route: Route, words: [u16; 4]) -> Vec<u8> {
    encode_q_d_words(route, 3010, &words).expect("fixed Q25PRH test frame fits")
}

/// A successful 1401 response has no data after its end code.
pub fn decode_write_ack(route: Route, frame: &[u8]) -> Result<(), ResponseError> {
    if response_data(route, frame)?.is_empty() {
        Ok(())
    } else {
        Err(ResponseError::Framing)
    }
}

/// Parse a complete response to this request. The Hub owns stream framing and I/O.
/// Even PLC refusals must echo the request's route, and with 4E its serial number.
fn response_data(route: Route, frame: &[u8]) -> Result<&[u8], ResponseError> {
    let header = route.frame.response_header_len();
    if frame.len() < header {
        return Err(ResponseError::Framing);
    }
    let route_at = match route.frame {
        Frame::E4 => {
            if frame[0..2] != [0xd4, 0]
                || frame[2..4] != route.serial.to_le_bytes()
                || frame[4..6] != [0, 0]
            {
                return Err(ResponseError::Framing);
            }
            6
        }
        Frame::E3 => {
            if frame[0..2] != [0xd0, 0] {
                return Err(ResponseError::Framing);
            }
            2
        }
    };
    let length_at = route.frame.length_offset();
    if frame[route_at] != route.network
        || frame[route_at + 1] != route.pc
        || frame[route_at + 2..route_at + 4] != route.io.to_le_bytes()
        || frame[route_at + 4] != route.station
        || usize::from(u16::from_le_bytes([frame[length_at], frame[length_at + 1]]))
            != frame.len() - route.frame.length_adjust()
    {
        return Err(ResponseError::Framing);
    }
    let end_code = u16::from_le_bytes([frame[header - 2], frame[header - 1]]);
    if end_code != 0 {
        return Err(ResponseError::EndCode(end_code));
    }
    Ok(&frame[header..])
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
