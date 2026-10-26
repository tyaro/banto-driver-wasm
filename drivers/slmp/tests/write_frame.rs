use banto_slmp_read_core::{
    decode_write_ack, encode_r_d_words, encode_r_d_write, RequestError, ResponseError, Route,
};

// Binary 4E, 1401/0002, D31=0x1234; independently transcribed from the
// SLMP Reference Manual's command and R device specification.
const R_D31_WRITE: [u8; 29] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 2, 0, 31, 0, 0, 0, 0xa8, 0,
    1, 0, 0x34, 0x12,
];
const ACK: [u8; 15] = [0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0, 0];
// Independent loopback fixture: 4E 1401/0002, D3010..D3013 = 1,2,3,4.
const R_D3010_FOUR_WRITE: [u8; 35] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 22, 0, 0x10, 0, 1, 0x14, 2, 0, 0xc2, 0x0b, 0, 0,
    0xa8, 0, 4, 0, 1, 0, 2, 0, 3, 0, 4, 0,
];

#[test]
fn r_single_d_write_matches_independent_wire_fixture() {
    assert_eq!(encode_r_d_write(Route::default(), 31, 0x1234), R_D31_WRITE);
    assert_eq!(decode_write_ack(Route::default(), &ACK), Ok(()));
}

#[test]
fn r_four_d_words_match_independent_loopback_fixture() {
    assert_eq!(
        encode_r_d_words(Route::default(), 3010, &[1, 2, 3, 4]),
        Ok(R_D3010_FOUR_WRITE.to_vec())
    );
    assert_eq!(decode_write_ack(Route::default(), &ACK), Ok(()));
}

#[test]
fn r_write_checks_count_and_last_device_before_encoding() {
    assert_eq!(
        encode_r_d_words(Route::default(), 0, &[]),
        Err(RequestError::Count)
    );
    assert!(encode_r_d_words(Route::default(), 0, &[0; 960]).is_ok());
    assert_eq!(
        encode_r_d_words(Route::default(), 0, &[0; 961]),
        Err(RequestError::Count)
    );
    assert!(encode_r_d_words(Route::default(), u32::MAX - 3, &[0; 4]).is_ok());
    assert_eq!(
        encode_r_d_words(Route::default(), u32::MAX - 2, &[0; 4]),
        Err(RequestError::DeviceNumber)
    );
}

#[test]
fn write_ack_requires_exact_route_serial_length_and_no_payload() {
    for index in [0, 2, 4, 6, 7, 8, 10, 11] {
        let mut bad = ACK;
        bad[index] ^= 1;
        assert_eq!(
            decode_write_ack(Route::default(), &bad),
            Err(ResponseError::Framing)
        );
    }
    let mut payload = ACK.to_vec();
    payload[11] = 4;
    payload.extend_from_slice(&[0x34, 0x12]);
    assert_eq!(
        decode_write_ack(Route::default(), &payload),
        Err(ResponseError::Framing)
    );
    let mut rejected = ACK;
    rejected[13..15].copy_from_slice(&0xc051u16.to_le_bytes());
    assert_eq!(
        decode_write_ack(Route::default(), &rejected),
        Err(ResponseError::EndCode(0xc051))
    );
}
