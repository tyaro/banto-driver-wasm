use banto_slmp_read_core::{
    decode_read, decode_write_ack, encode_r_clock_data_write, encode_r_clock_set_request,
    encode_r_clock_status_read, Device, Frame, ReadSpan, ResponseError, Route, Values,
    CLOCK_STATUS_SPAN,
};

// Independently transcribed from the SLMP reference manual's 0401/1401
// layout with the iQ-R device specification (four-byte number, two-byte
// code): SM 0091H, SD 00A9H, decimal 210 = D2H.

// 4E serial 3, 0401/0003, SM210, two points.
const STATUS_READ_4E: [u8; 27] = [
    0x54, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0,
    2, 0,
];
// 4E serial 4, 1401/0002, SD210-SD216 = 2026-11-06 12:22:05, Friday (5).
const DATA_WRITE_4E: [u8; 41] = [
    0x54, 0, 4, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 28, 0, 0x10, 0, 1, 0x14, 2, 0, 0xd2, 0, 0, 0, 0xa9,
    0, 7, 0, 0xea, 0x07, 11, 0, 6, 0, 12, 0, 22, 0, 5, 0, 5, 0,
];
// 4E serial 5, 1401/0003, SM210 = ON (one point, high nibble).
const SET_REQUEST_4E: [u8; 28] = [
    0x54, 0, 5, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 15, 0, 0x10, 0, 1, 0x14, 3, 0, 0xd2, 0, 0, 0, 0x91,
    0, 1, 0, 0x10,
];
// The same three requests as 3E (no serial number, no reserved bytes).
const STATUS_READ_3E: [u8; 21] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0,
];
const SET_REQUEST_3E: [u8; 22] = [
    0x50, 0, 0, 0xff, 0xff, 3, 0, 15, 0, 0x10, 0, 1, 0x14, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 1,
];

fn route(frame: Frame, serial: u16) -> Route {
    Route {
        frame,
        serial,
        ..Route::default()
    }
}

#[test]
fn clock_requests_match_the_manual_layout_on_4e() {
    assert_eq!(
        CLOCK_STATUS_SPAN,
        ReadSpan {
            device: Device::SM,
            start: 210,
            count: 2
        }
    );
    assert_eq!(
        encode_r_clock_status_read(route(Frame::E4, 3)),
        STATUS_READ_4E
    );
    assert_eq!(
        encode_r_clock_data_write(route(Frame::E4, 4), [2026, 11, 6, 12, 22, 5, 5]),
        DATA_WRITE_4E
    );
    assert_eq!(
        encode_r_clock_set_request(route(Frame::E4, 5)),
        SET_REQUEST_4E
    );
}

#[test]
fn clock_requests_match_the_manual_layout_on_3e() {
    let mut status = STATUS_READ_3E.to_vec();
    status.extend_from_slice(&[2, 0]);
    assert_eq!(encode_r_clock_status_read(route(Frame::E3, 9)), status);
    let mut set = SET_REQUEST_3E.to_vec();
    set.extend_from_slice(&[0, 0x10]);
    assert_eq!(encode_r_clock_set_request(route(Frame::E3, 9)), set);
    let data = encode_r_clock_data_write(route(Frame::E3, 9), [1980, 1, 1, 0, 0, 0, 2]);
    assert_eq!(&data[..7], &[0x50, 0, 0, 0xff, 0xff, 3, 0]);
    assert_eq!(&data[7..9], &[28, 0]);
    assert_eq!(
        &data[11..23],
        &[1, 0x14, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0, 7, 0]
    );
    assert_eq!(
        &data[23..],
        &[0xbc, 0x07, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0]
    );
}

#[test]
fn clock_status_and_write_responses_decode_with_the_common_decoders() {
    let route = route(Frame::E4, 3);
    // SM210 OFF (high nibble), SM211 ON (low nibble).
    let status = [0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 3, 0, 0, 0, 0x01];
    assert_eq!(
        decode_read(route, CLOCK_STATUS_SPAN, &status),
        Ok(Values::Bits(vec![false, true]))
    );
    let mut bad_nibble = status;
    bad_nibble[15] = 0x20;
    assert_eq!(
        decode_read(route, CLOCK_STATUS_SPAN, &bad_nibble),
        Err(ResponseError::Framing)
    );
    let refused = [0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0x51, 0xc0];
    assert_eq!(
        decode_read(route, CLOCK_STATUS_SPAN, &refused),
        Err(ResponseError::EndCode(0xc051))
    );
    let ack = [0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0, 0];
    assert_eq!(decode_write_ack(route, &ack), Ok(()));
    let mut wrong_serial = ack;
    wrong_serial[2] = 4;
    assert_eq!(
        decode_write_ack(route, &wrong_serial),
        Err(ResponseError::Framing)
    );
}
