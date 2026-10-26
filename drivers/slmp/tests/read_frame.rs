use banto_slmp_read_core::{
    decode_read, encode_read, Device, ReadSpan, RequestError, ResponseError, Route, Series, Values,
};

// Independent binary 4E fixtures from SLMP Reference Manual pp.20-30,31,37-47.
// Native banto-driver-slmp frame.rs has the same common-case layout.
const Q_D100_TWO: [u8; 25] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 12, 0, 0x10, 0, 1, 4, 0, 0, 100, 0, 0, 0xa8, 2, 0,
];
const R_M100_THREE: [u8; 27] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 3, 0, 100, 0, 0, 0, 0x90, 0, 3,
    0,
];
const D100_RESPONSE: [u8; 19] = [
    0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0, 0, 0x34, 0x12, 0xef, 0xbe,
];
const M100_RESPONSE: [u8; 17] = [
    0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 4, 0, 0, 0, 0x10, 0x10,
];
fn span(device: Device, start: u32, count: u16) -> ReadSpan {
    ReadSpan {
        device,
        start,
        count,
    }
}

#[test]
fn q_word_and_r_bit_match_independent_wire_fixtures() {
    assert_eq!(
        encode_read(Route::default(), Series::Q, span(Device::D, 100, 2)).unwrap(),
        Q_D100_TWO
    );
    assert_eq!(
        encode_read(Route::default(), Series::R, span(Device::M, 100, 3)).unwrap(),
        R_M100_THREE
    );
    assert_eq!(
        decode_read(Route::default(), span(Device::D, 100, 2), &D100_RESPONSE).unwrap(),
        Values::Words(vec![0x1234, 0xbeef])
    );
    assert_eq!(
        decode_read(Route::default(), span(Device::M, 100, 3), &M100_RESPONSE).unwrap(),
        Values::Bits(vec![true, false, true])
    );
}

#[test]
fn subcommands_device_code_and_number_width_are_series_specific() {
    let q = encode_read(Route::default(), Series::Q, span(Device::X, 0x1234, 1)).unwrap();
    assert_eq!(&q[17..25], &[1, 0, 0x34, 0x12, 0, 0x9c, 1, 0]);
    let q_y = encode_read(Route::default(), Series::Q, span(Device::Y, 0x1a, 3)).unwrap();
    assert_eq!(&q_y[17..25], &[1, 0, 0x1a, 0, 0, 0x9d, 3, 0]);
    let q_b = encode_read(Route::default(), Series::Q, span(Device::B, 0x2f, 1)).unwrap();
    assert_eq!(&q_b[17..25], &[1, 0, 0x2f, 0, 0, 0xa0, 1, 0]);
    let r_y = encode_read(Route::default(), Series::R, span(Device::Y, 0x1234_5678, 1)).unwrap();
    assert_eq!(&r_y[17..27], &[3, 0, 0x78, 0x56, 0x34, 0x12, 0x9d, 0, 1, 0]);
    let r_b = encode_read(Route::default(), Series::R, span(Device::B, 0x2f, 3)).unwrap();
    assert_eq!(&r_b[17..27], &[3, 0, 0x2f, 0, 0, 0, 0xa0, 0, 3, 0]);
    let r = encode_read(
        Route::default(),
        Series::R,
        span(Device::SD, 0x1234_5678, 1),
    )
    .unwrap();
    assert_eq!(&r[17..27], &[2, 0, 0x78, 0x56, 0x34, 0x12, 0xa9, 0, 1, 0]);
    let w = encode_read(Route::default(), Series::R, span(Device::W, 0x20, 1)).unwrap();
    assert_eq!(&w[17..27], &[2, 0, 0x20, 0, 0, 0, 0xb4, 0, 1, 0]);
    let q_w = encode_read(Route::default(), Series::Q, span(Device::W, 0x1e, 2)).unwrap();
    assert_eq!(&q_w[17..25], &[0, 0, 0x1e, 0, 0, 0xb4, 2, 0]);
}

#[test]
fn additional_bit_and_word_codes_keep_q_r_units_and_radices() {
    // MC device-code table, printed p.68. Addresses here are already parsed
    // according to each device's decimal or hexadecimal notation.
    for (device, code) in [
        (Device::L, 0x92),
        (Device::F, 0x93),
        (Device::V, 0x94),
        (Device::SB, 0xa1),
        (Device::SM, 0x91),
    ] {
        let q = encode_read(Route::default(), Series::Q, span(device, 0x123, 1)).unwrap();
        let r = encode_read(Route::default(), Series::R, span(device, 0x123, 1)).unwrap();
        assert_eq!(&q[17..25], &[1, 0, 0x23, 1, 0, code, 1, 0]);
        assert_eq!(&r[17..27], &[3, 0, 0x23, 1, 0, 0, code, 0, 1, 0]);
        assert_eq!(
            decode_read(
                Route::default(),
                span(device, 0x123, 1),
                &[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 3, 0, 0, 0, 0x10]
            ),
            Ok(Values::Bits(vec![true]))
        );
    }
    for (device, code) in [(Device::SW, 0xb5), (Device::SD, 0xa9)] {
        let q = encode_read(Route::default(), Series::Q, span(device, 0x123, 2)).unwrap();
        let r = encode_read(Route::default(), Series::R, span(device, 0x123, 2)).unwrap();
        assert_eq!(&q[17..25], &[0, 0, 0x23, 1, 0, code, 2, 0]);
        assert_eq!(&r[17..27], &[2, 0, 0x23, 1, 0, 0, code, 0, 2, 0]);
        assert_eq!(
            decode_read(Route::default(), span(device, 0x123, 2), &D100_RESPONSE),
            Ok(Values::Words(vec![0x1234, 0xbeef]))
        );
    }
}

#[test]
fn timer_counter_and_serial_file_codes_follow_mc_word_and_bit_units() {
    // MC pp.69,87: LTS/LTC and LSTS/LSTC cannot head a bit-unit batch.
    // Their logical tags read the LTN/LSTN four-word response flags instead.
    for (device, code) in [
        (Device::LTS, 0x52),
        (Device::LTC, 0x52),
        (Device::LSTS, 0x5a),
        (Device::LSTC, 0x5a),
    ] {
        let frame = encode_read(Route::default(), Series::R, span(device, 7, 4)).unwrap();
        assert_eq!(&frame[17..], &[2, 0, 7, 0, 0, 0, code, 0, 4, 0]);
    }
    // Independent MC code table, printed pp.68-69. The long timer request
    // uses four word access points per one logical timer (printed p.87).
    for (device, code) in [
        (Device::TS, 0xc1),
        (Device::TC, 0xc0),
        (Device::SS, 0xc7),
        (Device::SC, 0xc6),
        (Device::CS, 0xc4),
        (Device::CC, 0xc3),
    ] {
        for (series, tail) in [
            (Series::Q, vec![1, 0, 0x23, 1, 0, code, 1, 0]),
            (Series::R, vec![3, 0, 0x23, 1, 0, 0, code, 0, 1, 0]),
        ] {
            let frame = encode_read(Route::default(), series, span(device, 0x123, 1)).unwrap();
            assert_eq!(&frame[17..], tail);
            assert_eq!(
                decode_read(
                    Route::default(),
                    span(device, 0, 1),
                    &[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 3, 0, 0, 0, 0x10]
                ),
                Ok(Values::Bits(vec![true]))
            );
        }
    }
    for (device, code, count) in [
        (Device::TN, 0xc2, 1),
        (Device::SN, 0xc8, 1),
        (Device::CN, 0xc5, 1),
        (Device::LTN, 0x52, 4),
        (Device::LSTN, 0x5a, 4),
        (Device::LCN, 0x56, 2),
        (Device::ZR, 0xb0, 1),
    ] {
        let frame = encode_read(Route::default(), Series::R, span(device, 0x123, count)).unwrap();
        assert_eq!(
            &frame[17..],
            &[2, 0, 0x23, 1, 0, 0, code, 0, count as u8, 0]
        );
    }
    // Unlike long timer contacts, the R long counter contact and coil are
    // allowed as ordinary 0401 bit-unit head devices (MC pp.69, 90-91).
    for (device, code) in [(Device::LCS, 0x55), (Device::LCC, 0x54)] {
        let frame = encode_read(Route::default(), Series::R, span(device, 123, 1)).unwrap();
        assert_eq!(&frame[17..], &[3, 0, 123, 0, 0, 0, code, 0, 1, 0]);
        assert_eq!(
            decode_read(
                Route::default(),
                span(device, 123, 1),
                &[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 3, 0, 0, 0, 0x10]
            ),
            Ok(Values::Bits(vec![true]))
        );
    }
}

#[test]
fn point_counts_and_complete_number_ranges_are_checked() {
    for (series, request, expected) in [
        (Series::Q, span(Device::D, 0, 0), RequestError::Count),
        (Series::Q, span(Device::D, 0, 961), RequestError::Count),
        (Series::R, span(Device::M, 0, 7169), RequestError::Count),
        (Series::Q, span(Device::B, 0, 7169), RequestError::Count),
        (
            Series::Q,
            span(Device::D, 0x00ff_ffff, 2),
            RequestError::DeviceNumber,
        ),
        (
            Series::R,
            span(Device::D, u32::MAX, 2),
            RequestError::DeviceNumber,
        ),
    ] {
        assert_eq!(
            encode_read(Route::default(), series, request),
            Err(expected)
        );
    }
    assert!(encode_read(Route::default(), Series::Q, span(Device::D, 0x00ff_ffff, 1)).is_ok());
    assert!(encode_read(Route::default(), Series::R, span(Device::D, u32::MAX, 1)).is_ok());
    assert!(encode_read(Route::default(), Series::Q, span(Device::M, 0, 7168)).is_ok());
    assert!(encode_read(Route::default(), Series::Q, span(Device::B, 0, 7168)).is_ok());
    assert!(encode_read(Route::default(), Series::R, span(Device::W, 0, 960)).is_ok());
}

#[test]
fn response_must_echo_route_and_have_exact_length() {
    let expected = span(Device::D, 100, 2);
    for index in [0, 2, 4, 6, 7, 8, 10, 11] {
        let mut bad = D100_RESPONSE;
        bad[index] ^= 1;
        assert_eq!(
            decode_read(Route::default(), expected, &bad),
            Err(ResponseError::Framing)
        );
    }
    let mut short = D100_RESPONSE.to_vec();
    short.pop();
    assert_eq!(
        decode_read(Route::default(), expected, &short),
        Err(ResponseError::Framing)
    );
    let mut extra = D100_RESPONSE.to_vec();
    extra.push(0);
    assert_eq!(
        decode_read(Route::default(), expected, &extra),
        Err(ResponseError::Framing)
    );
    extra[11] += 1;
    assert_eq!(
        decode_read(Route::default(), expected, &extra),
        Err(ResponseError::Framing)
    );
}

#[test]
fn refusal_is_distinct_from_framing() {
    let mut refusal = D100_RESPONSE[..15].to_vec();
    refusal[11] = 2;
    refusal[13] = 0x59;
    refusal[14] = 0xc0;
    assert_eq!(
        decode_read(Route::default(), span(Device::D, 100, 2), &refusal),
        Err(ResponseError::EndCode(0xc059))
    );
    refusal[2] = 2;
    assert_eq!(
        decode_read(Route::default(), span(Device::D, 100, 2), &refusal),
        Err(ResponseError::Framing)
    );
}

#[test]
fn invalid_bit_nibbles_and_odd_padding_are_rejected() {
    let mut bad = M100_RESPONSE;
    for byte in [0x21, 0x12] {
        bad[15] = byte;
        assert_eq!(
            decode_read(Route::default(), span(Device::M, 100, 3), &bad),
            Err(ResponseError::Framing)
        );
    }
    bad[15] = 0x10;
    bad[16] = 0x11;
    assert_eq!(
        decode_read(Route::default(), span(Device::M, 100, 3), &bad),
        Err(ResponseError::Framing)
    );
}
