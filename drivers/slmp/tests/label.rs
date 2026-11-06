use banto_slmp_read_core::{
    decode_label_read, encode_label_read, encode_label_write, label_read_data_len,
    label_write_data_len, Frame, LabelData, LabelWrite, RequestError, ResponseError, Route,
    LABEL_SEND_DATA_LIMIT,
};

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// SLMP reference manual SH-080956ENG N printed p.112: 041C binary request
/// data for LabelB, LabelW and Sw.led (no abbreviation definition).
const MANUAL_041C: [u8; 50] = [
    0x1c, 0x04, 0, 0, 3, 0, 0, 0, // command, subcommand, 3 labels, 0 abbreviations
    6, 0, 0x4c, 0, 0x61, 0, 0x62, 0, 0x65, 0, 0x6c, 0, 0x42, 0, // LabelB
    6, 0, 0x4c, 0, 0x61, 0, 0x62, 0, 0x65, 0, 0x6c, 0, 0x57, 0, // LabelW
    6, 0, 0x53, 0, 0x77, 0, 0x2e, 0, 0x6c, 0, 0x65, 0, 0x64, 0, // Sw.led
];

/// Printed p.118: 141B binary request data writing LabelB=1, LabelW=0031H,
/// Sw.led=0001H.
const MANUAL_141B: [u8; 62] = [
    0x1b, 0x14, 0, 0, 3, 0, 0, 0, // command, subcommand, 3 labels, 0 abbreviations
    6, 0, 0x4c, 0, 0x61, 0, 0x62, 0, 0x65, 0, 0x6c, 0, 0x42, 0, 2, 0, 1, 0, // LabelB
    6, 0, 0x4c, 0, 0x61, 0, 0x62, 0, 0x65, 0, 0x6c, 0, 0x57, 0, 2, 0, 0x31, 0, // LabelW
    6, 0, 0x53, 0, 0x77, 0, 0x2e, 0, 0x6c, 0, 0x65, 0, 0x64, 0, 2, 0, 1, 0, // Sw.led
];

fn header_4e(serial: u16, data_len: u16) -> Vec<u8> {
    let mut out = vec![0x54, 0];
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&[0x10, 0]);
    out
}

fn response(route: Route, end_code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = match route.frame {
        Frame::E4 => {
            let mut head = vec![0xd4, 0];
            head.extend_from_slice(&route.serial.to_le_bytes());
            head.extend_from_slice(&[0, 0]);
            head
        }
        Frame::E3 => vec![0xd0, 0],
    };
    out.extend_from_slice(&[route.network, route.pc]);
    out.extend_from_slice(&route.io.to_le_bytes());
    out.push(route.station);
    out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_le_bytes());
    out.extend_from_slice(&end_code.to_le_bytes());
    out.extend_from_slice(body);
    out
}

#[test]
fn manual_041c_and_141b_examples_are_byte_exact() {
    let names = [utf16("LabelB"), utf16("LabelW"), utf16("Sw.led")];
    let refs = names.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let read = encode_label_read(Route::default(), &refs).unwrap();
    let mut expected = header_4e(1, 52);
    expected.extend_from_slice(&MANUAL_041C);
    assert_eq!(read, expected);
    assert_eq!(label_read_data_len(names.iter().map(Vec::len)), 52);

    let writes = [
        LabelWrite {
            name: &names[0],
            data: &[1, 0],
        },
        LabelWrite {
            name: &names[1],
            data: &[0x31, 0],
        },
        LabelWrite {
            name: &names[2],
            data: &[1, 0],
        },
    ];
    let write = encode_label_write(Route::default(), &writes).unwrap();
    let mut expected = header_4e(1, 64);
    expected.extend_from_slice(&MANUAL_141B);
    assert_eq!(write, expected);
}

#[test]
fn three_e_and_routed_requests_keep_the_label_data() {
    let name = utf16("Tank1.Level");
    let route = Route {
        frame: Frame::E3,
        network: 2,
        pc: 3,
        io: 0x03e1,
        ..Route::default()
    };
    let read = encode_label_read(route, &[&name]).unwrap();
    assert_eq!(&read[..9], &[0x50, 0, 2, 3, 0xe1, 3, 0, 34, 0]);
    assert_eq!(&read[9..11], &[0x10, 0]);
    assert_eq!(&read[11..19], &[0x1c, 4, 0, 0, 1, 0, 0, 0]);
    assert_eq!(&read[19..21], &[11, 0]);
    assert_eq!(read.len() - 9, 34);
}

#[test]
fn non_ascii_names_are_utf16le_with_code_unit_lengths() {
    // U+30BF U+30F3 U+30AF and one surrogate pair (U+1F600): five code units.
    let name = utf16("タンク😀");
    let read = encode_label_read(Route::default(), &[&name]).unwrap();
    assert_eq!(
        &read[23..],
        &[5, 0, 0xbf, 0x30, 0xf3, 0x30, 0xaf, 0x30, 0x3d, 0xd8, 0x00, 0xde]
    );
}

#[test]
fn the_1920_byte_limit_counts_the_request_data_length_field() {
    // 10 fixed bytes + 2 + 2n: n = 954 is exactly 1920.
    let fits = vec![u16::from(b'A'); 954];
    let read = encode_label_read(Route::default(), &[&fits]).unwrap();
    assert_eq!(u16::from_le_bytes([read[11], read[12]]), 1920);
    let over = vec![u16::from(b'A'); 955];
    assert_eq!(
        encode_label_read(Route::default(), &[&over]),
        Err(RequestError::TooLarge)
    );
    // 141B: 10 + 4 + 2n + data; n = 949 with 8 data bytes is 1920.
    let name = vec![u16::from(b'A'); 949];
    let data = [0u8; 8];
    assert!(encode_label_write(
        Route::default(),
        &[LabelWrite {
            name: &name,
            data: &data
        }]
    )
    .is_ok());
    assert_eq!(label_write_data_len([(949, 8)]), LABEL_SEND_DATA_LIMIT);
    let data = [0u8; 10];
    assert_eq!(
        encode_label_write(
            Route::default(),
            &[LabelWrite {
                name: &name,
                data: &data
            }]
        ),
        Err(RequestError::TooLarge)
    );
}

#[test]
fn malformed_requests_are_refused() {
    assert_eq!(
        encode_label_read(Route::default(), &[]),
        Err(RequestError::Count)
    );
    assert_eq!(
        encode_label_read(Route::default(), &[&[]]),
        Err(RequestError::LabelName)
    );
    let name = utf16("A");
    for data in [&[][..], &[1][..], &[1, 0, 0][..]] {
        assert_eq!(
            encode_label_write(Route::default(), &[LabelWrite { name: &name, data }]),
            Err(RequestError::Count)
        );
    }
    assert_eq!(
        encode_label_write(Route::default(), &[]),
        Err(RequestError::Count)
    );
}

#[test]
fn manual_response_decodes_per_label_and_ignores_the_spare_byte() {
    let route = Route {
        serial: 7,
        ..Route::default()
    };
    // Printed p.112 with spare bytes 0xAA/0x55/0x00.
    let body = [
        3, 0, 1, 0xaa, 2, 0, 1, 0, 2, 0x55, 2, 0, 0x44, 0, 2, 0, 2, 0, 0x31, 0,
    ];
    let labels = decode_label_read(route, 3, &response(route, 0, &body)).unwrap();
    assert_eq!(
        labels,
        vec![
            LabelData {
                type_id: 1,
                data: vec![1, 0]
            },
            LabelData {
                type_id: 2,
                data: vec![0x44, 0]
            },
            LabelData {
                type_id: 2,
                data: vec![0x31, 0]
            },
        ]
    );
}

#[test]
fn response_shape_errors_are_framing_and_end_codes_pass_through() {
    let route = Route::default();
    assert_eq!(
        decode_label_read(route, 1, &response(route, 0x4031, &[0; 9])),
        Err(ResponseError::EndCode(0x4031))
    );
    for body in [
        &[][..],
        &[1][..],
        &[2, 0, 2, 0, 2, 0, 1, 0][..],       // count echo differs
        &[1, 0, 2, 0, 4, 0, 1, 0][..],       // truncated data
        &[1, 0, 2, 0, 3, 0, 1, 0, 0][..],    // odd length
        &[1, 0, 2, 0, 2, 0, 1, 0, 0, 0][..], // trailing bytes
        &[1, 0, 2, 0][..],                   // truncated entry head
    ] {
        assert_eq!(
            decode_label_read(route, 1, &response(route, 0, body)),
            Err(ResponseError::Framing),
            "{body:?}"
        );
    }
    let other = Route {
        serial: 2,
        ..Route::default()
    };
    assert_eq!(
        decode_label_read(route, 1, &response(other, 0, &[1, 0, 2, 0, 2, 0, 1, 0])),
        Err(ResponseError::Framing)
    );
}
