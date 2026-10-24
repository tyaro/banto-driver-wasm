//! A small, transport-free Modbus TCP read core.
//!
//! This is an FC3, one-register experiment, not a `FrameDriver` or a WIT
//! component. The Hub still owns I/O and the production Modbus driver.
//! The shape is checked against the native implementation at
//! `banto-plc/src/modbus/frame.rs` and `banto-driver-modbus/src/frame_conn.rs`
//! at banto-industrial a0d01aa2. Runtime parity remains unverified.

#![no_std]

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
        let [tid_hi, tid_lo] = self.transaction_id.to_be_bytes();
        let [offset_hi, offset_lo] = self.offset.to_be_bytes();
        [
            tid_hi,
            tid_lo,
            0,
            0, // transaction and protocol ids
            0,
            6,
            self.unit_id, // MBAP length and unit id
            3,
            offset_hi,
            offset_lo,
            0,
            1, // FC3 and one register
        ]
    }

    /// Decode one complete response supplied by the host. The host remains
    /// responsible for stream framing, timeouts and the 260-byte limit.
    pub fn decode(self, response: &[u8]) -> Result<u16, ReadError> {
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
        if u16::from_be_bytes([response[0], response[1]]) != self.transaction_id
            || response[6] != self.unit_id
        {
            return Err(ReadError::Protocol);
        }
        let pdu = &response[7..];
        match pdu.first() {
            Some(0x83) if pdu.len() == 2 => Err(ReadError::Device(pdu[1])),
            Some(0x03) if pdu.len() == 4 && pdu[1] == 2 => Ok(u16::from_be_bytes([pdu[2], pdu[3]])),
            _ => Err(ReadError::Framing),
        }
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
}
