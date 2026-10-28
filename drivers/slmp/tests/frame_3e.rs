use banto_slmp_read_core::{
    decode_read, decode_type_name, decode_write_ack, encode_bits, encode_q_dw_words,
    encode_r_d_write, encode_read, encode_type_name, Device, Frame, ModelFamily, ReadSpan,
    ResponseError, Route, Series, Values,
};

// Independent binary 3E fixtures: subheader 5000/D000, no serial number and no
// reserved bytes; everything after the timer matches the 4E fixtures.
const Q_D100_TWO: [u8; 21] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 12, 0, 0x10, 0, 1, 4, 0, 0, 100, 0, 0, 0xa8, 2, 0,
];
const R_M100_THREE: [u8; 23] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 3, 0, 100, 0, 0, 0, 0x90, 0, 3, 0,
];
const Q_W1A_TWO_WRITE: [u8; 25] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 0, 0, 0x1a, 0, 0, 0xb4, 2, 0, 1, 0, 2, 0,
];
const R_D31_WRITE: [u8; 25] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 2, 0, 31, 0, 0, 0, 0xa8, 0, 1, 0, 0x34,
    0x12,
];
const Q_M100_THREE_BITS: [u8; 23] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 0x14, 1, 0, 100, 0, 0, 0x90, 3, 0, 0x10, 0x10,
];
const TYPE_NAME: [u8; 15] = [0x50, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0x10, 0, 1, 1, 0, 0];

const D100_RESPONSE: [u8; 15] = [
    0xd0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0, 0, 0x34, 0x12, 0xef, 0xbe,
];
const M100_RESPONSE: [u8; 13] = [0xd0, 0, 0, 0xff, 0xff, 3, 0, 4, 0, 0, 0, 0x10, 0x10];
const ACK: [u8; 11] = [0xd0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0, 0];
const REFUSAL: [u8; 11] = [0xd0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0x51, 0xc0];
const Q02UCPU: [u8; 29] = [
    0xd0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0, b'Q', b'0', b'2', b'U', b'C', b'P', b'U', b' ',
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', 0x63, 0x02,
];
// The same D100 data framed as 4E with the default serial number 1.
const D100_RESPONSE_4E: [u8; 19] = [
    0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0, 0, 0x34, 0x12, 0xef, 0xbe,
];

fn e3() -> Route {
    Route {
        frame: Frame::E3,
        ..Route::default()
    }
}

fn span(device: Device, start: u32, count: u16) -> ReadSpan {
    ReadSpan {
        device,
        start,
        count,
    }
}

#[test]
fn default_frame_stays_4e_and_frame_layout_constants_match() {
    assert_eq!(Route::default().frame, Frame::E4);
    assert_eq!(
        (
            Frame::E4.response_header_len(),
            Frame::E4.length_offset(),
            Frame::E4.length_adjust()
        ),
        (15, 11, 13)
    );
    assert_eq!(
        (
            Frame::E3.response_header_len(),
            Frame::E3.length_offset(),
            Frame::E3.length_adjust()
        ),
        (11, 7, 9)
    );
}

#[test]
fn requests_match_independent_3e_fixtures_and_ignore_serial() {
    assert_eq!(
        encode_read(e3(), Series::Q, span(Device::D, 100, 2)).unwrap(),
        Q_D100_TWO
    );
    assert_eq!(
        encode_read(e3(), Series::R, span(Device::M, 100, 3)).unwrap(),
        R_M100_THREE
    );
    assert_eq!(
        encode_q_dw_words(e3(), Device::W, 0x1a, &[1, 2]).unwrap(),
        Q_W1A_TWO_WRITE
    );
    assert_eq!(encode_r_d_write(e3(), 31, 0x1234), R_D31_WRITE);
    assert_eq!(
        encode_bits(e3(), Series::Q, Device::M, 100, &[true, false, true]).unwrap(),
        Q_M100_THREE_BITS
    );
    assert_eq!(encode_type_name(e3()), TYPE_NAME);
    let other_serial = Route {
        serial: 0x1234,
        ..e3()
    };
    assert_eq!(encode_type_name(other_serial), TYPE_NAME);
    assert_eq!(
        decode_read(other_serial, span(Device::D, 100, 2), &D100_RESPONSE).unwrap(),
        Values::Words(vec![0x1234, 0xbeef])
    );
}

#[test]
fn routed_3e_request_and_response_echo_every_route_field() {
    let route = Route {
        frame: Frame::E3,
        serial: 0x1234,
        network: 2,
        pc: 0xfe,
        io: 0x0123,
        station: 5,
        timer: 0x0099,
    };
    assert_eq!(
        encode_type_name(route),
        [0x50, 0, 2, 0xfe, 0x23, 1, 5, 6, 0, 0x99, 0, 1, 1, 0, 0]
    );
    let ack = [0xd0, 0, 2, 0xfe, 0x23, 1, 5, 2, 0, 0, 0];
    assert_eq!(decode_write_ack(route, &ack), Ok(()));
    for (index, value) in [(2, 3), (3, 0xff), (4, 0x24), (5, 2), (6, 6)] {
        let mut wrong = ack;
        wrong[index] = value;
        assert_eq!(
            decode_write_ack(route, &wrong),
            Err(ResponseError::Framing),
            "route byte {index}"
        );
    }
}

#[test]
fn responses_decode_words_bits_ack_type_name_and_end_code() {
    assert_eq!(
        decode_read(e3(), span(Device::D, 100, 2), &D100_RESPONSE).unwrap(),
        Values::Words(vec![0x1234, 0xbeef])
    );
    assert_eq!(
        decode_read(e3(), span(Device::M, 100, 3), &M100_RESPONSE).unwrap(),
        Values::Bits(vec![true, false, true])
    );
    assert_eq!(decode_write_ack(e3(), &ACK), Ok(()));
    let model = decode_type_name(e3(), &Q02UCPU).unwrap();
    assert_eq!(model.name, "Q02UCPU");
    assert_eq!(model.code, 0x0263);
    assert_eq!(model.family, ModelFamily::StandardQ);
    assert_eq!(
        decode_read(e3(), span(Device::D, 100, 2), &REFUSAL),
        Err(ResponseError::EndCode(0xc051))
    );
    assert_eq!(
        decode_write_ack(e3(), &REFUSAL),
        Err(ResponseError::EndCode(0xc051))
    );
    assert_eq!(
        decode_type_name(e3(), &REFUSAL),
        Err(ResponseError::EndCode(0xc051))
    );
}

#[test]
fn mismatched_frame_kind_subheader_length_and_short_frames_are_framing() {
    let d100 = span(Device::D, 100, 2);
    assert_eq!(
        decode_read(e3(), d100, &D100_RESPONSE_4E),
        Err(ResponseError::Framing),
        "4E response to a 3E route"
    );
    assert_eq!(
        decode_read(Route::default(), d100, &D100_RESPONSE),
        Err(ResponseError::Framing),
        "3E response to a 4E route"
    );
    assert_eq!(
        decode_write_ack(Route::default(), &ACK),
        Err(ResponseError::Framing)
    );
    for subheader in [[0x50, 0], [0xd4, 0], [0xd0, 1]] {
        let mut wrong = D100_RESPONSE;
        wrong[0..2].copy_from_slice(&subheader);
        assert_eq!(decode_read(e3(), d100, &wrong), Err(ResponseError::Framing));
    }
    let mut wrong_route = D100_RESPONSE;
    wrong_route[2] = 1;
    assert_eq!(
        decode_read(e3(), d100, &wrong_route),
        Err(ResponseError::Framing)
    );
    for length in [5, 7] {
        let mut wrong = D100_RESPONSE;
        wrong[7] = length;
        assert_eq!(decode_read(e3(), d100, &wrong), Err(ResponseError::Framing));
    }
    assert_eq!(
        decode_read(e3(), d100, &D100_RESPONSE[..14]),
        Err(ResponseError::Framing)
    );
    // Fewer than 11 bytes cannot hold the end code, even with a matching length.
    let mut short = [0xd0, 0, 0, 0xff, 0xff, 3, 0, 1, 0, 0];
    assert_eq!(decode_write_ack(e3(), &short), Err(ResponseError::Framing));
    short[7] = 0;
    assert_eq!(
        decode_write_ack(e3(), &short[..9]),
        Err(ResponseError::Framing)
    );
    assert_eq!(decode_write_ack(e3(), &[]), Err(ResponseError::Framing));
    let mut extra = ACK.to_vec();
    extra.push(0);
    extra[7] = 3;
    assert_eq!(decode_write_ack(e3(), &extra), Err(ResponseError::Framing));
}
