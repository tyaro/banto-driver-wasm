use banto_slmp_read_core::{
    decode_read, encode_read, encode_read_unit_buffer, Device, Frame, ReadSpan, RequestError,
    Route, Series, UnitBufferSpan, Values,
};

// SLMP Reference Manual SH-080956ENG N appendix 1 (pp.212-216) examples, with a
// word count appended: Q module I/O 0030 (U3) address 1, and R CPU buffer U3E0 address 1.
const Q_U3_G1_TWO: [u8; 32] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 19, 0, 0x10, 0, // 4E header, length, timer
    1, 4, 0x80, 0, // command, subcommand 0080
    0, 0, 1, 0, 0, 0xab, 0, 0, 3, 0, 0xf8, // manual example
    2, 0, // point count
];
const R_U3E0_G1_TWO: [u8; 34] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 21, 0, 0x10, 0, // 4E header, length, timer
    1, 4, 0x82, 0, // command, subcommand 0082
    0, 0, 1, 0, 0, 0, 0xab, 0, 0, 0, 0xe0, 3, 0xfa, // manual example
    2, 0, // point count
];

fn unit(module: u16, start: u32, count: u16) -> UnitBufferSpan {
    UnitBufferSpan {
        module,
        start,
        count,
    }
}

fn e3() -> Route {
    Route {
        frame: Frame::E3,
        ..Route::default()
    }
}

#[test]
fn manual_examples_are_byte_exact_in_4e() {
    assert_eq!(
        encode_read_unit_buffer(Route::default(), Series::Q, unit(3, 1, 2)).unwrap(),
        Q_U3_G1_TWO
    );
    assert_eq!(
        encode_read_unit_buffer(Route::default(), Series::R, unit(0x3e0, 1, 2)).unwrap(),
        R_U3E0_G1_TWO
    );
}

#[test]
fn three_e_drops_the_serial_and_keeps_the_data_length() {
    let q = encode_read_unit_buffer(e3(), Series::Q, unit(3, 1, 2)).unwrap();
    assert_eq!(&q[..2], &[0x50, 0]);
    assert_eq!(&q[7..9], &[19, 0]);
    assert_eq!(&q[9..], &Q_U3_G1_TWO[13..]);
    assert_eq!(q.len() - 9, 19);
    let r = encode_read_unit_buffer(e3(), Series::R, unit(0x3e0, 1, 2)).unwrap();
    assert_eq!(&r[7..9], &[21, 0]);
    assert_eq!(&r[9..], &R_U3E0_G1_TWO[13..]);
    assert_eq!(r.len() - 9, 21);
}

#[test]
fn extension_byte_is_fah_only_for_r_cpu_buffer_memory() {
    for module in [0x3e0u16, 0x3e1, 0x3e2, 0x3e3] {
        let frame =
            encode_read_unit_buffer(Route::default(), Series::R, unit(module, 0, 1)).unwrap();
        assert_eq!(&frame[29..32], &[module as u8, 0x03, 0xfa], "{module:X}");
    }
    for module in [0u16, 3, 0x10, 0xff] {
        for series in [Series::Q, Series::R] {
            let frame =
                encode_read_unit_buffer(Route::default(), series, unit(module, 0, 1)).unwrap();
            let at = frame.len() - 3;
            assert_eq!(
                &frame[at - 2..at + 1],
                &[module as u8, 0, 0xf8],
                "{module:X}"
            );
        }
    }
}

#[test]
fn module_and_number_limits_depend_on_the_series() {
    let route = Route::default();
    for module in [0x100u16, 0x3df, 0x3e0, 0x3e3, 0x3e4, 0xfff] {
        assert_eq!(
            encode_read_unit_buffer(route, Series::Q, unit(module, 0, 1)),
            Err(RequestError::DeviceNumber),
            "Q {module:X}"
        );
    }
    for module in [0x100u16, 0x3df, 0x3e4, 0xfff] {
        assert_eq!(
            encode_read_unit_buffer(route, Series::R, unit(module, 0, 1)),
            Err(RequestError::DeviceNumber),
            "R {module:X}"
        );
    }
    // Q: the last word must fit the 3-byte number; R: it must fit u32.
    assert!(encode_read_unit_buffer(route, Series::Q, unit(0, 0xff_ffff, 1)).is_ok());
    assert!(encode_read_unit_buffer(route, Series::Q, unit(0, 0xff_fffe, 2)).is_ok());
    assert_eq!(
        encode_read_unit_buffer(route, Series::Q, unit(0, 0xff_ffff, 2)),
        Err(RequestError::DeviceNumber)
    );
    assert_eq!(
        encode_read_unit_buffer(route, Series::Q, unit(0, 0x100_0000, 1)),
        Err(RequestError::DeviceNumber)
    );
    assert!(encode_read_unit_buffer(route, Series::R, unit(0, u32::MAX, 1)).is_ok());
    assert_eq!(
        encode_read_unit_buffer(route, Series::R, unit(0, u32::MAX, 2)),
        Err(RequestError::DeviceNumber)
    );
    let frame = encode_read_unit_buffer(route, Series::R, unit(0, u32::MAX, 1)).unwrap();
    assert_eq!(&frame[21..25], &[0xff; 4]);
}

#[test]
fn word_count_is_one_to_960() {
    for series in [Series::Q, Series::R] {
        for count in [0u16, 961] {
            assert_eq!(
                encode_read_unit_buffer(Route::default(), series, unit(0, 0, count)),
                Err(RequestError::Count)
            );
        }
        assert!(encode_read_unit_buffer(Route::default(), series, unit(0, 0, 960)).is_ok());
    }
}

#[test]
fn plain_read_cannot_encode_buffer_memory_and_the_response_is_a_word_read() {
    let span = ReadSpan {
        device: Device::UG,
        start: 1,
        count: 2,
    };
    assert_eq!(
        encode_read(Route::default(), Series::Q, span),
        Err(RequestError::DeviceNumber)
    );
    assert_eq!(unit(3, 1, 2).read_span(), span);
    let response = [
        0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0, 0, 0x34, 0x12, 0xef, 0xbe,
    ];
    assert_eq!(
        decode_read(Route::default(), span, &response),
        Ok(Values::Words(vec![0x1234, 0xbeef]))
    );
}
