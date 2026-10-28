//! A small, transport-free Modbus TCP core.
//!
//! This core frames FC1-FC4 reads and FC5/FC6/FC15/FC16/FC22 writes. It is not a
//! `FrameDriver` or a WIT component. The Hub still owns I/O and the
//! production Modbus driver.
//! The shape is checked against the native implementation at
//! `banto-plc/src/modbus/frame.rs` and `banto-driver-modbus/src/frame_conn.rs`
//! at banto-industrial a0d01aa2. The separate `comparison` crate exercises
//! the native FrameDriver on shared FC3 fixtures; it does not run WASM.

#![no_std]

extern crate alloc;
use alloc::vec::Vec;

use banto_register_codec::{
    decode_f32, decode_f64, decode_i16, decode_i32, decode_i64, decode_u32, decode_u64,
    WordOrder as CodecWordOrder,
};

/// The largest Modbus TCP response accepted by the Hub's frame driver.
pub const MAX_RESPONSE_LEN: usize = 260;

/// One holding-register read. `offset` is a zero-based wire offset; the
/// canonical reference `400001` has offset zero. Address parsing belongs to
/// the future WIT / `FrameDriver` adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadHoldingU16 {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
}

/// One signed holding register, using the same FC3 wire shape as U16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadHoldingI16 {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
}

/// The first register contains the low or high 16 bits, respectively.
/// The native connection uses `LowHigh` when `word_order` is absent.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WordOrder {
    #[default]
    LowHigh,
    HighLow,
}

impl WordOrder {
    /// The two supported native setting values. Configuration validation is
    /// the caller's responsibility; an unknown value is not silently accepted.
    pub const fn parse(value: &str) -> Option<Self> {
        match value.as_bytes() {
            b"low_high" => Some(Self::LowHigh),
            b"high_low" => Some(Self::HighLow),
            _ => None,
        }
    }

    const fn codec(self) -> CodecWordOrder {
        match self {
            Self::LowHigh => CodecWordOrder::LowHigh,
            Self::HighLow => CodecWordOrder::HighLow,
        }
    }
}

/// One two-register holding-register read. The final register must fit in
/// the 16-bit Modbus offset space, so offset 65534 is the last valid start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadHoldingU32 {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
    pub word_order: WordOrder,
}

/// One signed value spanning two complete holding registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadHoldingI32 {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
    pub word_order: WordOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    AddressRange,
    Count,
}

/// The read result is either a value, a non-fatal device exception, or a
/// connection-fatal framing/protocol failure. These categories match the
/// corresponding `banto-driver-api` error types, but no API conversion is
/// claimed until the adapter exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    Device(u8),
    Framing,
    Protocol,
}

/// FC1/FC2 bit table. One point is read per tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitFunction {
    Coil,
    DiscreteInput,
}

impl BitFunction {
    pub const fn code(self) -> u8 {
        match self {
            Self::Coil => 1,
            Self::DiscreteInput => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadBit {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
    pub function: BitFunction,
}

/// One bit of a holding or input register. Bit zero is the least significant
/// bit of the single wire word; multi-word `word_order` does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRegisterBit {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
    pub bit: u8,
    pub function: RegisterFunction,
}

impl ReadRegisterBit {
    pub fn request(self) -> [u8; 12] {
        request_function(
            self.transaction_id,
            self.unit_id,
            self.offset,
            1,
            self.function.code(),
        )
    }

    pub fn decode(self, response: &[u8]) -> Result<bool, ReadError> {
        if self.bit > 15 {
            return Err(ReadError::Protocol);
        }
        let data = decode_function(
            response,
            self.transaction_id,
            self.unit_id,
            2,
            self.function.code(),
        )?;
        let word = u16::from_be_bytes([data[0], data[1]]);
        Ok((word >> self.bit) & 1 != 0)
    }
}

impl ReadBit {
    pub fn request(self) -> [u8; 12] {
        request_function(
            self.transaction_id,
            self.unit_id,
            self.offset,
            1,
            self.function.code(),
        )
    }

    pub fn decode(self, response: &[u8]) -> Result<bool, ReadError> {
        let data = decode_function(
            response,
            self.transaction_id,
            self.unit_id,
            1,
            self.function.code(),
        )?;
        // A one-point reply must not set unused bits.
        if data[0] > 1 {
            return Err(ReadError::Framing);
        }
        Ok(data[0] == 1)
    }
}

/// FC5 writes one coil. The returned frame is valid only after its exact echo.
pub fn write_coil_request(transaction_id: u16, unit_id: u8, offset: u16, value: bool) -> [u8; 12] {
    let [hi, lo] = offset.to_be_bytes();
    [
        transaction_id.to_be_bytes()[0],
        transaction_id.to_be_bytes()[1],
        0,
        0,
        0,
        6,
        unit_id,
        5,
        hi,
        lo,
        if value { 0xff } else { 0 },
        0,
    ]
}

/// FC6 writes one holding register. The ACK echoes the address and value.
pub fn write_single_register_request(
    transaction_id: u16,
    unit_id: u8,
    offset: u16,
    value: u16,
) -> [u8; 12] {
    let [tid_hi, tid_lo] = transaction_id.to_be_bytes();
    let [hi, lo] = offset.to_be_bytes();
    let [value_hi, value_lo] = value.to_be_bytes();
    [
        tid_hi, tid_lo, 0, 0, 0, 6, unit_id, 6, hi, lo, value_hi, value_lo,
    ]
}

/// FC15 writes a contiguous coil span. Values are packed LSB-first, the first
/// coil in bit 0 of the first byte; unused high bits of the last byte are 0.
pub fn write_coils_request(
    transaction_id: u16,
    unit_id: u8,
    offset: u16,
    values: &[bool],
) -> Result<Vec<u8>, RequestError> {
    if values.is_empty() || values.len() > 1968 {
        return Err(RequestError::Count);
    }
    let count = u16::try_from(values.len()).map_err(|_| RequestError::Count)?;
    offset
        .checked_add(count - 1)
        .ok_or(RequestError::AddressRange)?;
    let byte_count = values.len().div_ceil(8);
    let mbap_len = 7 + byte_count;
    let mut frame = Vec::with_capacity(6 + mbap_len);
    frame.extend_from_slice(&transaction_id.to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&(mbap_len as u16).to_be_bytes());
    frame.extend_from_slice(&[unit_id, 15]);
    frame.extend_from_slice(&offset.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.push(byte_count as u8);
    for chunk in values.chunks(8) {
        frame.push(
            chunk
                .iter()
                .enumerate()
                .fold(0, |byte, (bit, value)| byte | (u8::from(*value) << bit)),
        );
    }
    Ok(frame)
}

/// FC16 writes a complete contiguous register span, capped by the Modbus PDU.
pub fn write_registers_request(
    transaction_id: u16,
    unit_id: u8,
    offset: u16,
    words: &[u16],
) -> Result<Vec<u8>, RequestError> {
    if words.is_empty() || words.len() > 123 {
        return Err(RequestError::Count);
    }
    let count = u16::try_from(words.len()).map_err(|_| RequestError::Count)?;
    offset
        .checked_add(count - 1)
        .ok_or(RequestError::AddressRange)?;
    let mbap_len = 7 + words.len() * 2;
    let mut frame = Vec::with_capacity(6 + mbap_len);
    frame.extend_from_slice(&transaction_id.to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&(mbap_len as u16).to_be_bytes());
    frame.extend_from_slice(&[unit_id, 16]);
    frame.extend_from_slice(&offset.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.push((words.len() * 2) as u8);
    for word in words {
        frame.extend_from_slice(&word.to_be_bytes());
    }
    Ok(frame)
}

/// FC22 AND/OR masks that change only `bit` of one holding register. The
/// device applies `(current & and) | (or & !and)`, so the other 15 bits are
/// preserved without a separate read.
pub fn register_bit_masks(bit: u8, value: bool) -> Result<(u16, u16), RequestError> {
    if bit > 15 {
        return Err(RequestError::AddressRange);
    }
    let mask = 1u16 << bit;
    Ok((!mask, if value { mask } else { 0 }))
}

/// FC22 mask-writes one holding register. A device that lacks FC22 answers
/// with an exception, which remains item-level.
pub fn mask_write_register_request(
    transaction_id: u16,
    unit_id: u8,
    offset: u16,
    and_mask: u16,
    or_mask: u16,
) -> [u8; 14] {
    let [tid_hi, tid_lo] = transaction_id.to_be_bytes();
    let [hi, lo] = offset.to_be_bytes();
    let [and_hi, and_lo] = and_mask.to_be_bytes();
    let [or_hi, or_lo] = or_mask.to_be_bytes();
    [
        tid_hi, tid_lo, 0, 0, 0, 8, unit_id, 22, hi, lo, and_hi, and_lo, or_hi, or_lo,
    ]
}

/// Strict ACK validation for FC5, FC6, FC15, FC16 and FC22; exceptions
/// remain item-level. `echo` is the request PDU after the function code (for
/// FC15/FC16 only address and quantity): four bytes for FC5/FC6/FC15/FC16 and
/// six for FC22.
pub fn decode_write_ack(
    response: &[u8],
    transaction_id: u16,
    unit_id: u8,
    function: u8,
    echo: &[u8],
) -> Result<(), ReadError> {
    let echo_len = match function {
        5 | 6 | 15 | 16 => 4,
        22 => 6,
        _ => return Err(ReadError::Protocol),
    };
    if echo.len() != echo_len {
        return Err(ReadError::Protocol);
    }
    if response.len() < 7 || response.len() > MAX_RESPONSE_LEN || response[2..4] != [0, 0] {
        return Err(ReadError::Framing);
    }
    let length = usize::from(u16::from_be_bytes([response[4], response[5]]));
    if length == 0 || response.len() != 6 + length {
        return Err(ReadError::Framing);
    }
    if u16::from_be_bytes([response[0], response[1]]) != transaction_id || response[6] != unit_id {
        return Err(ReadError::Protocol);
    }
    match &response[7..] {
        [code, exception] if *code == (function | 0x80) => Err(ReadError::Device(*exception)),
        [code, rest @ ..] if *code == function && rest == echo => Ok(()),
        [code, rest @ ..] if *code == function && rest.len() == echo_len => {
            Err(ReadError::Protocol)
        }
        _ => Err(ReadError::Framing),
    }
}

impl ReadHoldingU16 {
    /// MBAP + FC3 + start offset + count=1. No socket access occurs here.
    pub fn request(self) -> [u8; 12] {
        request(self.transaction_id, self.unit_id, self.offset, 1)
    }

    /// Decode one complete response supplied by the host. The host remains
    /// responsible for stream framing, timeouts and the 260-byte limit.
    pub fn decode(self, response: &[u8]) -> Result<u16, ReadError> {
        let data = decode_fc3(response, self.transaction_id, self.unit_id, 2)?;
        Ok(u16::from_be_bytes([data[0], data[1]]))
    }
}

impl ReadHoldingI16 {
    pub fn request(self) -> [u8; 12] {
        request(self.transaction_id, self.unit_id, self.offset, 1)
    }

    pub fn decode(self, response: &[u8]) -> Result<i16, ReadError> {
        let data = decode_fc3(response, self.transaction_id, self.unit_id, 2)?;
        Ok(decode_i16(u16::from_be_bytes([data[0], data[1]])))
    }
}

impl ReadHoldingU32 {
    /// MBAP + FC3 + start offset + count=2. No socket access occurs here.
    pub fn request(self) -> Result<[u8; 12], RequestError> {
        if self.offset == u16::MAX {
            return Err(RequestError::AddressRange);
        }
        Ok(request(self.transaction_id, self.unit_id, self.offset, 2))
    }

    /// Decode one complete response using the configured register order.
    pub fn decode(self, response: &[u8]) -> Result<u32, ReadError> {
        let data = decode_fc3(response, self.transaction_id, self.unit_id, 4)?;
        let first = u16::from_be_bytes([data[0], data[1]]);
        let second = u16::from_be_bytes([data[2], data[3]]);
        Ok(decode_u32([first, second], self.word_order.codec()))
    }
}

impl ReadHoldingI32 {
    pub fn request(self) -> Result<[u8; 12], RequestError> {
        ReadHoldingU32 {
            transaction_id: self.transaction_id,
            unit_id: self.unit_id,
            offset: self.offset,
            word_order: self.word_order,
        }
        .request()
    }

    pub fn decode(self, response: &[u8]) -> Result<i32, ReadError> {
        let data = decode_fc3(response, self.transaction_id, self.unit_id, 4)?;
        let first = u16::from_be_bytes([data[0], data[1]]);
        let second = u16::from_be_bytes([data[2], data[3]]);
        Ok(decode_i32([first, second], self.word_order.codec()))
    }
}

/// Register table selects FC3 holding or FC4 input reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterFunction {
    Holding,
    Input,
}

impl RegisterFunction {
    const fn code(self) -> u8 {
        match self {
            Self::Holding => 3,
            Self::Input => 4,
        }
    }
}

/// Numeric FC3/FC4 read width and interpretation, independent of wire framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericKind {
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl NumericKind {
    pub const fn word_count(self) -> u16 {
        match self {
            Self::I16 | Self::U16 => 1,
            Self::I32 | Self::U32 | Self::F32 => 2,
            Self::I64 | Self::U64 | Self::F64 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericValue {
    Int(i64),
    Uint(u64),
    Float(f64),
}

/// One complete numeric tag. No socket access or cross-tag word reads occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadHoldingNumeric {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub offset: u16,
    pub kind: NumericKind,
    pub word_order: WordOrder,
}

impl ReadHoldingNumeric {
    pub fn request(self) -> Result<[u8; 12], RequestError> {
        self.request_with_function(RegisterFunction::Holding)
    }

    pub fn request_with_function(
        self,
        function: RegisterFunction,
    ) -> Result<[u8; 12], RequestError> {
        self.offset
            .checked_add(self.kind.word_count() - 1)
            .ok_or(RequestError::AddressRange)?;
        Ok(request_function(
            self.transaction_id,
            self.unit_id,
            self.offset,
            self.kind.word_count(),
            function.code(),
        ))
    }

    pub fn decode(self, response: &[u8]) -> Result<NumericValue, ReadError> {
        self.decode_with_function(response, RegisterFunction::Holding)
    }

    pub fn decode_with_function(
        self,
        response: &[u8],
        function: RegisterFunction,
    ) -> Result<NumericValue, ReadError> {
        let data = decode_function(
            response,
            self.transaction_id,
            self.unit_id,
            usize::from(self.kind.word_count()) * 2,
            function.code(),
        )?;
        let mut words = [0u16; 4];
        for (index, bytes) in data.chunks_exact(2).enumerate() {
            words[index] = u16::from_be_bytes([bytes[0], bytes[1]]);
        }
        let pair = [words[0], words[1]];
        let order = self.word_order.codec();
        Ok(match self.kind {
            NumericKind::I16 => NumericValue::Int(i64::from(decode_i16(words[0]))),
            NumericKind::U16 => NumericValue::Uint(u64::from(words[0])),
            NumericKind::I32 => NumericValue::Int(i64::from(decode_i32(pair, order))),
            NumericKind::U32 => NumericValue::Uint(u64::from(decode_u32(pair, order))),
            NumericKind::I64 => NumericValue::Int(decode_i64(words, order)),
            NumericKind::U64 => NumericValue::Uint(decode_u64(words, order)),
            NumericKind::F32 => NumericValue::Float(f64::from(decode_f32(pair, order))),
            NumericKind::F64 => NumericValue::Float(decode_f64(words, order)),
        })
    }
}

fn request(transaction_id: u16, unit_id: u8, offset: u16, count: u16) -> [u8; 12] {
    request_function(transaction_id, unit_id, offset, count, 3)
}

fn request_function(
    transaction_id: u16,
    unit_id: u8,
    offset: u16,
    count: u16,
    function: u8,
) -> [u8; 12] {
    let [tid_hi, tid_lo] = transaction_id.to_be_bytes();
    let [offset_hi, offset_lo] = offset.to_be_bytes();
    let [count_hi, count_lo] = count.to_be_bytes();
    [
        tid_hi, tid_lo, 0, 0, 0, 6, unit_id, function, offset_hi, offset_lo, count_hi, count_lo,
    ]
}

fn decode_fc3(
    response: &[u8],
    transaction_id: u16,
    unit_id: u8,
    data_len: usize,
) -> Result<&[u8], ReadError> {
    decode_function(response, transaction_id, unit_id, data_len, 3)
}

fn decode_function(
    response: &[u8],
    transaction_id: u16,
    unit_id: u8,
    data_len: usize,
    function: u8,
) -> Result<&[u8], ReadError> {
    if response.len() < 7 || response.len() > MAX_RESPONSE_LEN {
        return Err(ReadError::Framing);
    }
    if response[2..4] != [0, 0] {
        return Err(ReadError::Framing);
    }
    let length = usize::from(u16::from_be_bytes([response[4], response[5]]));
    if length == 0 || response.len() != 6 + length {
        return Err(ReadError::Framing);
    }
    if u16::from_be_bytes([response[0], response[1]]) != transaction_id || response[6] != unit_id {
        return Err(ReadError::Protocol);
    }
    let pdu = &response[7..];
    match pdu.first() {
        Some(code) if *code == (function | 0x80) && pdu.len() == 2 => {
            Err(ReadError::Device(pdu[1]))
        }
        Some(code)
            if *code == function
                && pdu.len() == 2 + data_len
                && usize::from(pdu[1]) == data_len =>
        {
            Ok(&pdu[2..])
        }
        _ => Err(ReadError::Framing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_bits_use_one_fc3_or_fc4_word_and_ignore_word_order() {
        for (function, code) in [(RegisterFunction::Holding, 3), (RegisterFunction::Input, 4)] {
            for (word, bit, expected) in [
                (0x0000u16, 0, false),
                (0xffff, 0, true),
                (0x0001, 0, true),
                (0x0001, 15, false),
                (0x8000, 0, false),
                (0x8000, 15, true),
                (0x1234, 2, true),
                (0x1234, 15, false),
            ] {
                let read = ReadRegisterBit {
                    transaction_id: 0x1234,
                    unit_id: 1,
                    offset: 65535,
                    bit,
                    function,
                };
                assert_eq!(
                    read.request(),
                    [0x12, 0x34, 0, 0, 0, 6, 1, code, 0xff, 0xff, 0, 1]
                );
                let [hi, lo] = word.to_be_bytes();
                assert_eq!(
                    read.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, code, 2, hi, lo]),
                    Ok(expected)
                );
            }
            let bad = ReadRegisterBit {
                transaction_id: 0x1234,
                unit_id: 1,
                offset: 0,
                bit: 16,
                function,
            };
            assert_eq!(
                bad.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, code, 2, 0, 0]),
                Err(ReadError::Protocol)
            );
            let read = ReadRegisterBit { bit: 0, ..bad };
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, code | 0x80, 2]),
                Err(ReadError::Device(2))
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, code, 1, 1]),
                Err(ReadError::Framing)
            );
            assert_eq!(
                read.decode(&[0x12, 0x35, 0, 0, 0, 5, 1, code, 2, 0, 1]),
                Err(ReadError::Protocol)
            );
        }
    }

    #[test]
    fn fc1_fc2_bool_read_and_strict_bit_response() {
        for (function, code) in [(BitFunction::Coil, 1), (BitFunction::DiscreteInput, 2)] {
            let read = ReadBit {
                transaction_id: 0x1234,
                unit_id: 1,
                offset: 65535,
                function,
            };
            assert_eq!(
                read.request(),
                [0x12, 0x34, 0, 0, 0, 6, 1, code, 255, 255, 0, 1]
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, code, 1, 1]),
                Ok(true)
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, code, 1, 0]),
                Ok(false)
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, code | 0x80, 2]),
                Err(ReadError::Device(2))
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, code, 1, 2]),
                Err(ReadError::Framing)
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, 3, 1, 1]),
                Err(ReadError::Framing)
            );
            assert_eq!(
                read.decode(&[0x12, 0x35, 0, 0, 0, 4, 1, code, 1, 1]),
                Err(ReadError::Protocol)
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 4, 2, code, 1, 1]),
                Err(ReadError::Protocol)
            );
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, code, 1, 1]),
                Err(ReadError::Framing)
            );
        }
    }

    #[test]
    fn fc5_fc16_write_ack_checks_identity_echo_length_and_exception() {
        let coil = write_coil_request(0x1234, 1, 65535, true);
        assert_eq!(coil, [0x12, 0x34, 0, 0, 0, 6, 1, 5, 255, 255, 255, 0]);
        assert_eq!(
            decode_write_ack(&coil, 0x1234, 1, 5, &[255, 255, 255, 0]),
            Ok(())
        );
        let words = write_registers_request(0x1234, 1, 65534, &[0x1234, 0xabcd]).unwrap();
        assert_eq!(
            words,
            [0x12, 0x34, 0, 0, 0, 11, 1, 16, 255, 254, 0, 2, 4, 0x12, 0x34, 0xab, 0xcd]
        );
        let ack = [0x12, 0x34, 0, 0, 0, 6, 1, 16, 255, 254, 0, 2];
        assert_eq!(
            decode_write_ack(&ack, 0x1234, 1, 16, &[255, 254, 0, 2]),
            Ok(())
        );
        assert_eq!(
            decode_write_ack(
                &[0x12, 0x34, 0, 0, 0, 3, 1, 0x90, 2],
                0x1234,
                1,
                16,
                &[255, 254, 0, 2]
            ),
            Err(ReadError::Device(2))
        );
        for bad in [
            [0x12, 0x35, 0, 0, 0, 6, 1, 16, 255, 254, 0, 2],
            [0x12, 0x34, 0, 0, 0, 6, 2, 16, 255, 254, 0, 2],
            [0x12, 0x34, 0, 0, 0, 6, 1, 16, 255, 254, 0, 3],
            [0x12, 0x34, 0, 0, 0, 7, 1, 16, 255, 254, 0, 2],
            [0x12, 0x34, 0, 0, 0, 6, 1, 5, 255, 254, 0, 2],
        ] {
            assert!(decode_write_ack(&bad, 0x1234, 1, 16, &[255, 254, 0, 2]).is_err());
        }
        assert_eq!(
            write_registers_request(1, 1, 65535, &[1, 2]),
            Err(RequestError::AddressRange)
        );
        assert_eq!(
            write_registers_request(1, 1, 0, &[]),
            Err(RequestError::Count)
        );
        assert_eq!(
            write_registers_request(1, 1, 0, &[0; 124]),
            Err(RequestError::Count)
        );
    }

    #[test]
    fn fc22_mask_write_changes_one_bit_and_checks_full_echo() {
        assert_eq!(register_bit_masks(0, true), Ok((0xfffe, 0x0001)));
        assert_eq!(register_bit_masks(15, false), Ok((0x7fff, 0x0000)));
        assert_eq!(
            register_bit_masks(16, true),
            Err(RequestError::AddressRange)
        );
        // The device result `(current & and) | (or & !and)` keeps other bits.
        for bit in 0..16 {
            for value in [false, true] {
                let (and, or) = register_bit_masks(bit, value).unwrap();
                for current in [0u16, 0xffff, 0xa5a5, 0x5a5a] {
                    let result = (current & and) | (or & !and);
                    assert_eq!(result & !(1 << bit), current & !(1 << bit));
                    assert_eq!((result >> bit) & 1 != 0, value);
                }
            }
        }
        let frame = mask_write_register_request(0x1234, 1, 65535, 0xfff7, 0x0008);
        assert_eq!(
            frame,
            [0x12, 0x34, 0, 0, 0, 8, 1, 22, 255, 255, 0xff, 0xf7, 0, 8]
        );
        let echo = &frame[8..14];
        assert_eq!(decode_write_ack(&frame, 0x1234, 1, 22, echo), Ok(()));
        assert_eq!(
            decode_write_ack(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x96, 1], 0x1234, 1, 22, echo),
            Err(ReadError::Device(1))
        );
        let mut wrong_mask = frame;
        wrong_mask[13] = 0;
        assert_eq!(
            decode_write_ack(&wrong_mask, 0x1234, 1, 22, echo),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            decode_write_ack(&frame, 0x1234, 1, 22, &frame[8..12]),
            Err(ReadError::Protocol)
        );
        for bad in [
            &frame[..12],
            &[0x12, 0x34, 0, 0, 0, 6, 1, 22, 255, 255, 0xff, 0xf7][..],
            &[0x12, 0x34, 0, 0, 0, 8, 1, 16, 255, 255, 0xff, 0xf7, 0, 8][..],
        ] {
            assert!(decode_write_ack(bad, 0x1234, 1, 22, echo).is_err());
        }
        assert_eq!(
            decode_write_ack(&frame, 0x1234, 1, 6, echo),
            Err(ReadError::Protocol)
        );
    }

    #[test]
    fn fc6_single_register_write_checks_address_and_value_echo() {
        let frame = write_single_register_request(0x1234, 1, 65535, 0xabcd);
        assert_eq!(frame, [0x12, 0x34, 0, 0, 0, 6, 1, 6, 255, 255, 0xab, 0xcd]);
        let echo = &frame[8..12];
        assert_eq!(decode_write_ack(&frame, 0x1234, 1, 6, echo), Ok(()));
        assert_eq!(
            decode_write_ack(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x86, 1], 0x1234, 1, 6, echo),
            Err(ReadError::Device(1))
        );
        let mut wrong_value = frame;
        wrong_value[11] = 0xce;
        assert_eq!(
            decode_write_ack(&wrong_value, 0x1234, 1, 6, echo),
            Err(ReadError::Protocol)
        );
        let mut wrong_address = frame;
        wrong_address[9] = 0;
        assert_eq!(
            decode_write_ack(&wrong_address, 0x1234, 1, 6, echo),
            Err(ReadError::Protocol)
        );
        for bad in [
            &frame[..11],
            &[0x12, 0x34, 0, 0, 0, 6, 1, 16, 255, 255, 0xab, 0xcd][..],
            &[0x12, 0x35, 0, 0, 0, 6, 1, 6, 255, 255, 0xab, 0xcd][..],
        ] {
            assert!(decode_write_ack(bad, 0x1234, 1, 6, echo).is_err());
        }
    }

    #[test]
    fn fc15_coil_write_packs_bits_lsb_first_and_checks_quantity_echo() {
        let one = write_coils_request(0x1234, 1, 65535, &[true]).unwrap();
        assert_eq!(one, [0x12, 0x34, 0, 0, 0, 8, 1, 15, 255, 255, 0, 1, 1, 1]);
        let eight = [true, false, true, true, false, false, true, false];
        assert_eq!(
            write_coils_request(1, 1, 0, &eight).unwrap()[12..],
            [1, 0b0100_1101]
        );
        let nine = [true, false, false, false, false, false, false, false, true];
        assert_eq!(
            write_coils_request(1, 1, 0, &nine).unwrap(),
            [
                0,
                1,
                0,
                0,
                0,
                9,
                1,
                15,
                0,
                0,
                0,
                9,
                2,
                0b0000_0001,
                0b0000_0001
            ]
        );
        // The Modbus specification example: coils 20-29 = 1100 1101 01.
        let ten = [
            true, false, true, true, false, false, true, true, true, false,
        ];
        assert_eq!(
            write_coils_request(1, 1, 19, &ten).unwrap(),
            [0, 1, 0, 0, 0, 9, 1, 15, 0, 19, 0, 10, 2, 0xcd, 0x01]
        );
        let max = write_coils_request(1, 1, 0, &[true; 1968]).unwrap();
        assert_eq!(max.len(), 13 + 246);
        assert_eq!(max[10..13], [0x07, 0xb0, 246]);
        assert_eq!(
            write_coils_request(1, 1, 0, &[true; 1969]),
            Err(RequestError::Count)
        );
        assert_eq!(write_coils_request(1, 1, 0, &[]), Err(RequestError::Count));
        assert_eq!(
            write_coils_request(1, 1, 65535, &[true, false]),
            Err(RequestError::AddressRange)
        );

        let echo = [0, 19, 0, 10];
        let ack = [0, 1, 0, 0, 0, 6, 1, 15, 0, 19, 0, 10];
        assert_eq!(decode_write_ack(&ack, 1, 1, 15, &echo), Ok(()));
        assert_eq!(
            decode_write_ack(&[0, 1, 0, 0, 0, 3, 1, 0x8f, 2], 1, 1, 15, &echo),
            Err(ReadError::Device(2))
        );
        for mismatch in [
            [0, 1, 0, 0, 0, 6, 1, 15, 0, 19, 0, 9],
            [0, 1, 0, 0, 0, 6, 1, 15, 0, 20, 0, 10],
        ] {
            assert_eq!(
                decode_write_ack(&mismatch, 1, 1, 15, &echo),
                Err(ReadError::Protocol)
            );
        }
        for bad in [
            &ack[..11],
            &[0, 1, 0, 0, 0, 6, 1, 5, 0, 19, 0, 10][..],
            &[0, 1, 0, 0, 0, 7, 1, 15, 0, 19, 0, 10, 2][..],
        ] {
            assert!(decode_write_ack(bad, 1, 1, 15, &echo).is_err());
        }
        assert_eq!(
            decode_write_ack(&ack, 1, 1, 15, &[0, 19, 0, 10, 2]),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            decode_write_ack(&ack, 1, 1, 7, &echo),
            Err(ReadError::Protocol)
        );
    }

    const READ: ReadHoldingU16 = ReadHoldingU16 {
        transaction_id: 0x1234,
        unit_id: 1,
        offset: 0,
    };

    const READ_U32: ReadHoldingU32 = ReadHoldingU32 {
        transaction_id: 0x1234,
        unit_id: 1,
        offset: 0,
        word_order: WordOrder::LowHigh,
    };

    #[test]
    fn request_matches_native_mbap_shape() {
        assert_eq!(READ.request(), [0x12, 0x34, 0, 0, 0, 6, 1, 3, 0, 0, 0, 1]);
    }

    #[test]
    fn fc4_numeric_reads_reject_fc3_frames_and_handle_exceptions() {
        let read = ReadHoldingNumeric {
            transaction_id: 7,
            unit_id: 1,
            offset: 65535,
            kind: NumericKind::U16,
            word_order: WordOrder::LowHigh,
        };
        assert_eq!(
            read.request_with_function(RegisterFunction::Input).unwrap(),
            [0, 7, 0, 0, 0, 6, 1, 4, 255, 255, 0, 1]
        );
        assert_eq!(
            read.decode_with_function(
                &[0, 7, 0, 0, 0, 5, 1, 4, 2, 0x12, 0x34],
                RegisterFunction::Input
            ),
            Ok(NumericValue::Uint(0x1234))
        );
        assert_eq!(
            read.decode_with_function(&[0, 7, 0, 0, 0, 3, 1, 0x84, 2], RegisterFunction::Input),
            Err(ReadError::Device(2))
        );
        assert_eq!(
            read.decode_with_function(
                &[0, 7, 0, 0, 0, 5, 1, 3, 2, 0x12, 0x34],
                RegisterFunction::Input
            ),
            Err(ReadError::Framing)
        );
        assert_eq!(
            read.decode_with_function(
                &[0, 8, 0, 0, 0, 5, 1, 4, 2, 0x12, 0x34],
                RegisterFunction::Input
            ),
            Err(ReadError::Protocol)
        );
        assert!(ReadHoldingNumeric {
            kind: NumericKind::U32,
            ..read
        }
        .request_with_function(RegisterFunction::Input)
        .is_err());
    }

    #[test]
    fn decodes_one_holding_register() {
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, 3, 2, 0, 42]),
            Ok(42)
        );
    }

    #[test]
    fn device_exception_is_not_connection_fatal() {
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x83, 2]),
            Err(ReadError::Device(2))
        );
    }

    #[test]
    fn normal_response_rejects_trailing_pdu_byte_with_matching_mbap_length() {
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 6, 1, 3, 2, 0, 42, 0xff]),
            Err(ReadError::Framing)
        );
    }

    #[test]
    fn exception_response_rejects_trailing_pdu_byte_with_matching_mbap_length() {
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 4, 1, 0x83, 2, 0xff]),
            Err(ReadError::Framing)
        );
    }

    #[test]
    fn foreign_transaction_and_unit_are_protocol_errors() {
        assert_eq!(
            READ.decode(&[0x12, 0x35, 0, 0, 0, 5, 1, 3, 2, 0, 42]),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 5, 2, 3, 2, 0, 42]),
            Err(ReadError::Protocol)
        );
    }

    #[test]
    fn malformed_frames_are_fatal() {
        assert_eq!(READ.decode(&[0x12, 0x34]), Err(ReadError::Framing));
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 1, 0, 5, 1, 3, 2, 0, 42]),
            Err(ReadError::Framing)
        );
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 6, 1, 3, 2, 0, 42]),
            Err(ReadError::Framing)
        );
        assert_eq!(
            READ.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, 3, 1, 0, 42]),
            Err(ReadError::Framing)
        );
    }

    #[test]
    fn u32_request_checks_the_last_register() {
        assert_eq!(
            READ_U32.request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0, 0, 0, 2])
        );
        assert_eq!(
            ReadHoldingU32 {
                offset: 65_534,
                ..READ_U32
            }
            .request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xfe, 0, 2])
        );
        assert_eq!(
            ReadHoldingU32 {
                offset: 65_535,
                ..READ_U32
            }
            .request(),
            Err(RequestError::AddressRange)
        );
    }

    #[test]
    fn u32_words_and_native_connection_default() {
        assert_eq!(WordOrder::parse("low_high"), Some(WordOrder::LowHigh));
        assert_eq!(WordOrder::parse("high_low"), Some(WordOrder::HighLow));
        assert_eq!(WordOrder::parse("other"), None);
        assert_eq!(WordOrder::default(), WordOrder::LowHigh);
        let response = [0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd];
        assert_eq!(READ_U32.decode(&response), Ok(0xabcd_1234));
        assert_eq!(
            ReadHoldingU32 {
                word_order: WordOrder::HighLow,
                ..READ_U32
            }
            .decode(&response),
            Ok(0x1234_abcd)
        );
        for (words, expected) in [([0, 0, 0, 0], 0), ([255, 255, 255, 255], u32::MAX)] {
            let response = [
                0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, words[0], words[1], words[2], words[3],
            ];
            assert_eq!(READ_U32.decode(&response), Ok(expected));
        }
    }

    #[test]
    fn u32_response_errors_and_strict_pdu() {
        assert_eq!(
            READ_U32.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x83, 2]),
            Err(ReadError::Device(2))
        );
        assert_eq!(
            READ_U32.decode(&[0x12, 0x35, 0, 0, 0, 7, 1, 3, 4, 0, 1, 0, 2]),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            READ_U32.decode(&[0x12, 0x34, 0, 0, 0, 7, 2, 3, 4, 0, 1, 0, 2]),
            Err(ReadError::Protocol)
        );
        for response in [
            &[0x12, 0x34, 0, 0, 0, 7][..],
            &[0x12, 0x34, 0, 1, 0, 7, 1, 3, 4, 0, 1, 0, 2],
            &[0x12, 0x34, 0, 0, 0, 8, 1, 3, 4, 0, 1, 0, 2],
            &[0x12, 0x34, 0, 0, 0, 7, 1, 3, 2, 0, 1, 0, 2],
            &[0x12, 0x34, 0, 0, 0, 6, 1, 3, 4, 0, 1, 0],
            &[0x12, 0x34, 0, 0, 0, 8, 1, 3, 4, 0, 1, 0, 2, 255],
            &[0x12, 0x34, 0, 0, 0, 4, 1, 0x83, 2, 255],
        ] {
            assert_eq!(READ_U32.decode(response), Err(ReadError::Framing));
        }
    }

    #[test]
    fn i16_uses_one_register_and_preserves_signed_limits() {
        let read = ReadHoldingI16 {
            transaction_id: 0x1234,
            unit_id: 1,
            offset: u16::MAX,
        };
        assert_eq!(
            read.request(),
            [0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xff, 0, 1]
        );
        for (word, expected) in [(0x8000u16, i16::MIN), (0x7fff, i16::MAX), (0xffff, -1)] {
            let [high, low] = word.to_be_bytes();
            assert_eq!(
                read.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, 3, 2, high, low]),
                Ok(expected)
            );
        }
        assert_eq!(
            read.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x83, 2]),
            Err(ReadError::Device(2))
        );
        assert_eq!(
            read.decode(&[0x12, 0x35, 0, 0, 0, 5, 1, 3, 2, 0, 1]),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            read.decode(&[0x12, 0x34, 0, 0, 0, 5, 1, 3, 1, 0, 1]),
            Err(ReadError::Framing)
        );
    }

    #[test]
    fn i32_checks_last_register_and_both_word_orders() {
        let read = ReadHoldingI32 {
            transaction_id: 0x1234,
            unit_id: 1,
            offset: 65_534,
            word_order: WordOrder::LowHigh,
        };
        assert_eq!(
            read.request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xfe, 0, 2])
        );
        assert_eq!(
            ReadHoldingI32 {
                offset: u16::MAX,
                ..read
            }
            .request(),
            Err(RequestError::AddressRange)
        );
        let low_high = [0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x80, 0];
        let high_low = [0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, 0x80, 0, 0, 0];
        assert_eq!(read.decode(&low_high), Ok(i32::MIN));
        assert_eq!(
            ReadHoldingI32 {
                word_order: WordOrder::HighLow,
                ..read
            }
            .decode(&high_low),
            Ok(i32::MIN)
        );
        assert_eq!(read.decode(&high_low), Ok(0x0000_8000));
        assert_eq!(
            read.decode(&[0x12, 0x34, 0, 0, 0, 3, 1, 0x83, 2]),
            Err(ReadError::Device(2))
        );
        assert_eq!(
            read.decode(&[0x12, 0x35, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x80, 0]),
            Err(ReadError::Protocol)
        );
        assert_eq!(
            read.decode(&[0x12, 0x34, 0, 0, 0, 7, 1, 3, 2, 0, 0, 0x80, 0]),
            Err(ReadError::Framing)
        );
    }

    #[test]
    fn numeric_widths_codec_and_nonfinite_bits() {
        let mut read = ReadHoldingNumeric {
            transaction_id: 0x1234,
            unit_id: 1,
            offset: 65_532,
            kind: NumericKind::I64,
            word_order: WordOrder::HighLow,
        };
        assert_eq!(
            read.request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xfc, 0, 4])
        );
        assert_eq!(
            ReadHoldingNumeric {
                offset: 65_533,
                ..read
            }
            .request(),
            Err(RequestError::AddressRange)
        );
        let min_i64 = [0x12, 0x34, 0, 0, 0, 11, 1, 3, 8, 0x80, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(read.decode(&min_i64), Ok(NumericValue::Int(i64::MIN)));
        read.word_order = WordOrder::LowHigh;
        let reversed = [0x12, 0x34, 0, 0, 0, 11, 1, 3, 8, 0, 0, 0, 0, 0, 0, 0x80, 0];
        assert_eq!(read.decode(&reversed), Ok(NumericValue::Int(i64::MIN)));
        read.kind = NumericKind::U64;
        let precise = [0x12, 0x34, 0, 0, 0, 11, 1, 3, 8, 0, 1, 0, 0, 0, 0, 0, 0x20];
        assert_eq!(
            read.decode(&precise),
            Ok(NumericValue::Uint(9_007_199_254_740_993))
        );
        read.kind = NumericKind::F64;
        let finite = [
            0x12, 0x34, 0, 0, 0, 11, 1, 3, 8, 0, 0, 0, 0, 0, 0, 0x3f, 0xf8,
        ];
        assert_eq!(read.decode(&finite), Ok(NumericValue::Float(1.5)));
        read.kind = NumericKind::F32;
        read.offset = 65_534;
        assert_eq!(
            read.request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xfe, 0, 2])
        );
        let f32_words = [0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x3f, 0xc0];
        assert_eq!(read.decode(&f32_words), Ok(NumericValue::Float(1.5)));
        let nan_words = [0x12, 0x34, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x7f, 0xc0];
        assert!(matches!(read.decode(&nan_words), Ok(NumericValue::Float(n)) if n.is_nan()));
        assert_eq!(
            read.decode(&[0x12, 0x34, 0, 0, 0, 7, 1, 3, 2, 0, 0, 0x3f, 0xc0]),
            Err(ReadError::Framing)
        );
        read.kind = NumericKind::U16;
        read.offset = u16::MAX;
        assert_eq!(
            read.request(),
            Ok([0x12, 0x34, 0, 0, 0, 6, 1, 3, 0xff, 0xff, 0, 1])
        );
    }
}
