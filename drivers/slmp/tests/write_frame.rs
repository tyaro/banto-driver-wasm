use banto_slmp_read_core::{
    decode_write_ack, encode_bits, encode_q25prh_d3010_3013_test_write, encode_q_d_words,
    encode_q_dw_words, encode_r_d_words, encode_r_d_write, encode_r_dw_words, Device, RequestError,
    ResponseError, Route, Series,
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
// Independent Q 4E wire fixture: 1401/0000, 24-bit D3010, count 4.
const Q_D3010_FOUR_TEST_WRITE: [u8; 33] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0x10, 0, 1, 0x14, 0, 0, 0xc2, 0x0b, 0, 0xa8,
    4, 0, 1, 0, 2, 0, 3, 0, 4, 0,
];
// Independent 4E word-unit fixtures: hexadecimal W1A/W1B = 1,2.
const R_W1A_TWO_WRITE: [u8; 31] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 18, 0, 0x10, 0, 1, 0x14, 2, 0, 0x1a, 0, 0, 0, 0xb4,
    0, 2, 0, 1, 0, 2, 0,
];
const Q_W1A_TWO_WRITE: [u8; 29] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 0, 0, 0x1a, 0, 0, 0xb4, 2,
    0, 1, 0, 2, 0,
];

// Independent 4E bit-unit fixtures, M100..M102 = on, off, on. The last
// unused low nibble is zero. Q uses 0001 and R uses 0003.
const Q_M100_THREE_BITS: [u8; 27] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 0x14, 1, 0, 100, 0, 0, 0x90, 3, 0,
    0x10, 0x10,
];
const R_M100_THREE_BITS: [u8; 29] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 3, 0, 100, 0, 0, 0, 0x90, 0,
    3, 0, 0x10, 0x10,
];

#[test]
fn bit_write_matches_q_r_fixtures_and_rejects_unsupported_device_or_span() {
    let bits = [true, false, true];
    assert_eq!(
        encode_bits(Route::default(), Series::Q, Device::M, 100, &bits),
        Ok(Q_M100_THREE_BITS.to_vec())
    );
    assert_eq!(
        encode_bits(Route::default(), Series::R, Device::M, 100, &bits),
        Ok(R_M100_THREE_BITS.to_vec())
    );
    assert_eq!(
        encode_bits(Route::default(), Series::R, Device::X, 100, &bits),
        Err(RequestError::DeviceNumber)
    );
    assert_eq!(
        encode_bits(Route::default(), Series::R, Device::M, 100, &[]),
        Err(RequestError::Count)
    );
    assert_eq!(
        encode_bits(Route::default(), Series::Q, Device::M, 0x00ff_ffff, &bits),
        Err(RequestError::DeviceNumber)
    );
    assert!(encode_bits(Route::default(), Series::R, Device::M, 0, &[false; 7168]).is_ok());
    assert_eq!(
        encode_bits(Route::default(), Series::R, Device::M, 0, &[false; 7169]),
        Err(RequestError::Count)
    );
    for (series, device, number, tail) in [
        (
            Series::Q,
            Device::Y,
            0x1a,
            &vec![0x1a, 0, 0, 0x9d, 2, 0, 0x10],
        ),
        (
            Series::Q,
            Device::B,
            0x2f,
            &vec![0x2f, 0, 0, 0xa0, 2, 0, 0x10],
        ),
        (
            Series::R,
            Device::L,
            10,
            &vec![10, 0, 0, 0, 0x92, 0, 2, 0, 0x10],
        ),
    ] {
        let frame = encode_bits(Route::default(), series, device, number, &[true, false]).unwrap();
        assert_eq!(
            &frame[frame.len() - tail.len()..],
            tail,
            "{series:?} {device:?}"
        );
    }
}

#[test]
fn common_word_write_keeps_d_bytes_and_encodes_r_q_w() {
    assert_eq!(
        encode_r_dw_words(Route::default(), Device::W, 0x1a, &[1, 2]),
        Ok(R_W1A_TWO_WRITE.to_vec())
    );
    assert_eq!(
        encode_q_dw_words(Route::default(), Device::W, 0x1a, &[1, 2]),
        Ok(Q_W1A_TWO_WRITE.to_vec())
    );
    assert_eq!(
        encode_r_dw_words(Route::default(), Device::D, 3010, &[1, 2, 3, 4]),
        Ok(R_D3010_FOUR_WRITE.to_vec())
    );
    assert_eq!(
        encode_q_dw_words(Route::default(), Device::D, 3010, &[1, 2, 3, 4]),
        Ok(Q_D3010_FOUR_TEST_WRITE.to_vec())
    );
    assert_eq!(
        encode_r_dw_words(Route::default(), Device::M, 0, &[1]),
        Err(RequestError::DeviceNumber)
    );
}

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
fn q25prh_four_word_test_frame_uses_q_subcommand_and_24_bit_address() {
    assert_eq!(
        encode_q25prh_d3010_3013_test_write(Route::default(), [1, 2, 3, 4]),
        Q_D3010_FOUR_TEST_WRITE
    );
    assert_eq!(decode_write_ack(Route::default(), &ACK), Ok(()));
}

#[test]
fn q_word_write_preserves_test_bytes_and_checks_width_and_address() {
    assert_eq!(
        encode_q_d_words(Route::default(), 3010, &[1, 2, 3, 4]),
        Ok(Q_D3010_FOUR_TEST_WRITE.to_vec())
    );
    assert_eq!(
        encode_q_d_words(Route::default(), 0, &[]),
        Err(RequestError::Count)
    );
    assert!(encode_q_d_words(Route::default(), 0, &[0; 960]).is_ok());
    assert_eq!(
        encode_q_d_words(Route::default(), 0, &[0; 961]),
        Err(RequestError::Count)
    );
    assert!(encode_q_d_words(Route::default(), 0x00ff_fffe, &[1, 2]).is_ok());
    assert_eq!(
        encode_q_d_words(Route::default(), 0x00ff_fffe, &[1, 2, 3]),
        Err(RequestError::DeviceNumber)
    );
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
