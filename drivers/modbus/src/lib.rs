//! A small, transport-free Modbus TCP read core.
//!
//! This is an FC3, single-tag U16/U32 experiment, not a `FrameDriver` or a WIT
//! component. The Hub still owns I/O and the production Modbus driver.
//! The shape is checked against the native implementation at
//! `banto-plc/src/modbus/frame.rs` and `banto-driver-modbus/src/frame_conn.rs`
//! at banto-industrial a0d01aa2. The separate `comparison` crate exercises
//! the native FrameDriver on shared FC3 fixtures; it does not run WASM.

#![no_std]

use banto_register_codec::{decode_u32, WordOrder as CodecWordOrder};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    AddressRange,
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
        let order = match self.word_order {
            WordOrder::LowHigh => CodecWordOrder::LowHigh,
            WordOrder::HighLow => CodecWordOrder::HighLow,
        };
        Ok(decode_u32([first, second], order))
    }
}

fn request(transaction_id: u16, unit_id: u8, offset: u16, count: u16) -> [u8; 12] {
    let [tid_hi, tid_lo] = transaction_id.to_be_bytes();
    let [offset_hi, offset_lo] = offset.to_be_bytes();
    let [count_hi, count_lo] = count.to_be_bytes();
    [
        tid_hi, tid_lo, 0, 0, 0, 6, unit_id, 3, offset_hi, offset_lo, count_hi, count_lo,
    ]
}

fn decode_fc3(
    response: &[u8],
    transaction_id: u16,
    unit_id: u8,
    data_len: usize,
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
        Some(0x83) if pdu.len() == 2 => Err(ReadError::Device(pdu[1])),
        Some(0x03) if pdu.len() == 2 + data_len && usize::from(pdu[1]) == data_len => Ok(&pdu[2..]),
        _ => Err(ReadError::Framing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
