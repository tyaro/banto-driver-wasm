use banto_slmp_read_core::{
    clock_data_span, decode_q_clock_data, decode_r_clock_data, decode_read, encode_clock_data_read,
    encode_clock_read_status, ClockData, Device, Frame, NotBcd, ReadSpan, ResponseError, Route,
    Series, Values, CLOCK_READ_STATUS_SPAN, Q_CLOCK_DATA_SPAN, R_CLOCK_DATA_SPAN,
};

// Independently transcribed from the SLMP reference manual's 0401 layout:
// iQ-R uses a four-byte number and two-byte code (subcommands 0002/0003),
// Q a three-byte number and one-byte code (0000/0001). SM 91H, SD A9H,
// decimal 210 = D2H.

// 4E serial 3, R 0401/0003, SM210, four points.
const R_STATUS_4E: [u8; 27] = [
    0x54, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0,
    4, 0,
];
// 4E serial 4, R 0401/0002, SD210, seven words.
const R_DATA_4E: [u8; 27] = [
    0x54, 0, 4, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0,
    7, 0,
];
// 4E serial 3, Q 0401/0001, SM210, four points.
const Q_STATUS_4E: [u8; 25] = [
    0x54, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 12, 0, 0x10, 0, 1, 4, 1, 0, 0xd2, 0, 0, 0x91, 4, 0,
];
// 4E serial 4, Q 0401/0000, SD210, four words.
const Q_DATA_4E: [u8; 25] = [
    0x54, 0, 4, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 12, 0, 0x10, 0, 1, 4, 0, 0, 0xd2, 0, 0, 0xa9, 4, 0,
];

fn route(frame: Frame, serial: u16) -> Route {
    Route {
        frame,
        serial,
        ..Route::default()
    }
}

/// The 4E request without its serial number and reserved bytes.
fn as_3e(request: &[u8]) -> Vec<u8> {
    let mut out = vec![0x50, 0];
    out.extend_from_slice(&request[6..]);
    out
}

#[test]
fn clock_read_requests_match_the_manual_layout_on_4e() {
    assert_eq!(
        CLOCK_READ_STATUS_SPAN,
        ReadSpan {
            device: Device::SM,
            start: 210,
            count: 4
        }
    );
    assert_eq!(
        (
            R_CLOCK_DATA_SPAN.device,
            R_CLOCK_DATA_SPAN.start,
            R_CLOCK_DATA_SPAN.count
        ),
        (Device::SD, 210, 7)
    );
    assert_eq!(
        (
            Q_CLOCK_DATA_SPAN.device,
            Q_CLOCK_DATA_SPAN.start,
            Q_CLOCK_DATA_SPAN.count
        ),
        (Device::SD, 210, 4)
    );
    assert_eq!(clock_data_span(Series::R), R_CLOCK_DATA_SPAN);
    assert_eq!(clock_data_span(Series::Q), Q_CLOCK_DATA_SPAN);
    assert_eq!(
        encode_clock_read_status(route(Frame::E4, 3), Series::R),
        R_STATUS_4E
    );
    assert_eq!(
        encode_clock_data_read(route(Frame::E4, 4), Series::R),
        R_DATA_4E
    );
    assert_eq!(
        encode_clock_read_status(route(Frame::E4, 3), Series::Q),
        Q_STATUS_4E
    );
    assert_eq!(
        encode_clock_data_read(route(Frame::E4, 4), Series::Q),
        Q_DATA_4E
    );
}

#[test]
fn clock_read_requests_match_the_manual_layout_on_3e() {
    for (series, status, data) in [
        (Series::R, &R_STATUS_4E[..], &R_DATA_4E[..]),
        (Series::Q, &Q_STATUS_4E[..], &Q_DATA_4E[..]),
    ] {
        assert_eq!(
            encode_clock_read_status(route(Frame::E3, 9), series),
            as_3e(status),
            "{series:?}"
        );
        assert_eq!(
            encode_clock_data_read(route(Frame::E3, 9), series),
            as_3e(data),
            "{series:?}"
        );
    }
    assert_eq!(
        &encode_clock_read_status(route(Frame::E3, 9), Series::Q)[..9],
        &[0x50, 0, 0, 0xff, 0xff, 3, 0, 12, 0]
    );
}

#[test]
fn clock_read_responses_decode_with_the_common_decoder() {
    let route = route(Frame::E4, 3);
    // SM210 OFF, SM211 ON, SM212 OFF, SM213 ON.
    let status = [
        0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 4, 0, 0, 0, 0x01, 0x01,
    ];
    assert_eq!(
        decode_read(route, CLOCK_READ_STATUS_SPAN, &status),
        Ok(Values::Bits(vec![false, true, false, true]))
    );
    // 2026-11-07 13:04:59, Saturday (6), BIN.
    let mut data = vec![0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0, 0];
    for word in [2026u16, 11, 7, 13, 4, 59, 6] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    assert_eq!(
        decode_read(route, R_CLOCK_DATA_SPAN, &data),
        Ok(Values::Words(vec![2026, 11, 7, 13, 4, 59, 6]))
    );
    // Q: four words; seven would not match the length.
    assert_eq!(
        decode_read(route, Q_CLOCK_DATA_SPAN, &data),
        Err(ResponseError::Framing)
    );
    let refused = [0xd4, 0, 3, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0x51, 0xc0];
    assert_eq!(
        decode_read(route, R_CLOCK_DATA_SPAN, &refused),
        Err(ResponseError::EndCode(0xc051))
    );
}

#[test]
fn r_clock_words_are_bin_and_the_day_of_week_is_ignored() {
    assert_eq!(
        decode_r_clock_data([2026, 11, 7, 13, 4, 59, 6]),
        ClockData {
            year: 2026,
            month: 11,
            day: 7,
            hour: 13,
            minute: 4,
            second: 59
        }
    );
    // Values are passed through unchecked, the day of week is dropped.
    assert_eq!(
        decode_r_clock_data([0xffff, 300, 0, 99, 60, 61, 0xffff]),
        ClockData {
            year: 0xffff,
            month: 300,
            day: 0,
            hour: 99,
            minute: 60,
            second: 61
        }
    );
}

#[test]
fn q_clock_words_are_bcd_with_the_manual_byte_order() {
    // The QCPU manual's examples: H9307, H3110, H3548, H1905.
    assert_eq!(
        decode_q_clock_data([0x9307, 0x3110, 0x3548, 0x1905]),
        Ok(ClockData {
            year: 1993,
            month: 7,
            day: 31,
            hour: 10,
            minute: 35,
            second: 48
        })
    );
    assert_eq!(
        decode_q_clock_data([0x2611, 0x0713, 0x0459, 0x2006]),
        Ok(ClockData {
            year: 2026,
            month: 11,
            day: 7,
            hour: 13,
            minute: 4,
            second: 59
        })
    );
    // The day-of-week byte is not decoded: a non-BCD value passes.
    assert!(decode_q_clock_data([0x2611, 0x0713, 0x0459, 0x20ff]).is_ok());
    // Every other nibble must be 0-9.
    for (words, sd) in [
        ([0x2a11, 0x0713, 0x0459, 0x2006], 210),
        ([0x261a, 0x0713, 0x0459, 0x2006], 210),
        ([0x2611, 0xf713, 0x0459, 0x2006], 211),
        ([0x2611, 0x071b, 0x0459, 0x2006], 211),
        ([0x2611, 0x0713, 0x0c59, 0x2006], 212),
        ([0x2611, 0x0713, 0x045e, 0x2006], 212),
        ([0x2611, 0x0713, 0x0459, 0xa006], 213),
        ([0x2611, 0x0713, 0x0459, 0x2d06], 213),
    ] {
        assert_eq!(decode_q_clock_data(words), Err(NotBcd(sd)), "{words:04x?}");
    }
    // BCD but outside the clock range is still decoded; the guest checks it.
    assert_eq!(
        decode_q_clock_data([0x9913, 0x3224, 0x6060, 0x1900])
            .map(|data| (data.year, data.month, data.day, data.hour)),
        Ok((1999, 13, 32, 24))
    );
}
