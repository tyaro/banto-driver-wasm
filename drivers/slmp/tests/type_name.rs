use banto_slmp_read_core::{
    basic_sd_count, decode_type_name, encode_type_name, plan_basic_sd, BasicArea, BasicEvidence,
    ModelFamily, PointCount, ResponseError, Route,
};

// Literal binary 4E bytes assembled from SLMP Reference Manual pp. 20-30,
// 138-141. The Q02UCPU name/code pair is its printed binary example (p. 141).
const REQUEST: [u8; 19] = [
    0x54, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0x10, 0, 1, 1, 0, 0,
];
const Q02UCPU: [u8; 33] = [
    0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0, b'Q', b'0', b'2', b'U', b'C', b'P',
    b'U', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', 0x63, 0x02,
];
const R04ENCPU: [u8; 33] = [
    0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0, b'R', b'0', b'4', b'E', b'N', b'C',
    b'P', b'U', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', 0x05, 0x48,
];

#[test]
fn request_and_known_q_r_responses_match_wire_fixtures() {
    assert_eq!(encode_type_name(Route::default()), REQUEST);
    let q = decode_type_name(Route::default(), &Q02UCPU).unwrap();
    assert_eq!(q.name, "Q02UCPU");
    assert_eq!(q.raw_name, Q02UCPU[15..31]);
    assert_eq!(q.code, 0x0263);
    assert_eq!(q.family, ModelFamily::StandardQ);
    let r = decode_type_name(Route::default(), &R04ENCPU).unwrap();
    assert_eq!(r.name, "R04ENCPU");
    assert_eq!(r.code, 0x4805);
    assert_eq!(r.family, ModelFamily::StandardR);
}

#[test]
fn q25prh_exact_0101_pair_proves_q_wire_and_separate_sd302_map() {
    // SLMP Reference Manual, printed p.139, lists Q25PRHCPU / 004CH.
    let mut frame = Q02UCPU;
    frame[15..31].copy_from_slice(b"Q25PRHCPU       ");
    frame[31..33].copy_from_slice(&[0x4c, 0x00]);
    let model = decode_type_name(Route::default(), &frame).unwrap();
    assert_eq!(model.name, "Q25PRHCPU");
    assert_eq!(model.raw_name, *b"Q25PRHCPU       ");
    assert_eq!(model.code, 0x004c);
    assert_eq!(model.family, ModelFamily::Q25PrhWireOnly);
    let sd = plan_basic_sd(&model, BasicArea::DInternal).expect("QnPRH SD302");
    assert_eq!(
        (sd.device, sd.start, sd.count),
        (banto_slmp_read_core::Device::SD, 302, 1)
    );
    assert_eq!(plan_basic_sd(&model, BasicArea::X), None);
    assert_eq!(
        basic_sd_count(
            &model,
            BasicArea::DInternal,
            BasicEvidence {
                route_confirmed: true,
                r_firmware: None,
            },
            Some(&[1024]),
        ),
        PointCount::Exact(1024)
    );

    frame[31..33].copy_from_slice(&[0x4b, 0x00]);
    assert_eq!(
        decode_type_name(Route::default(), &frame).unwrap().family,
        ModelFamily::Unknown
    );
    frame[31..33].copy_from_slice(&[0x4c, 0x00]);
    frame[15..31].copy_from_slice(b"Q12PRHCPU       ");
    assert_eq!(
        decode_type_name(Route::default(), &frame).unwrap().family,
        ModelFamily::Unknown
    );
}

#[test]
fn universal_q_udeh_aliases_use_the_codes_in_the_official_table() {
    // Both pairs share a code with their UDH counterpart (printed p. 139).
    for (raw_name, code) in [
        (b"Q10UDEHCPU      ", [0x66, 0x02]),
        (b"Q20UDEHCPU      ", [0x67, 0x02]),
    ] {
        let mut response = Q02UCPU;
        response[15..31].copy_from_slice(raw_name);
        response[31..33].copy_from_slice(&code);
        let parsed = decode_type_name(Route::default(), &response).unwrap();
        assert_eq!(parsed.raw_name, *raw_name);
        assert_eq!(parsed.code, u16::from_le_bytes(code));
        assert_eq!(parsed.family, ModelFamily::StandardQ);
        response[31..33].copy_from_slice(&[0x68, 0x02]);
        assert_eq!(
            decode_type_name(Route::default(), &response)
                .unwrap()
                .family,
            ModelFamily::Unknown
        );
    }
    for (raw_name, code) in [
        (b"Q10UDHCPU       ", [0x66, 0x02]),
        (b"Q20UDHCPU       ", [0x67, 0x02]),
    ] {
        let mut response = Q02UCPU;
        response[15..31].copy_from_slice(raw_name);
        response[31..33].copy_from_slice(&code);
        assert_eq!(
            decode_type_name(Route::default(), &response)
                .unwrap()
                .family,
            ModelFamily::StandardQ
        );
    }
}

#[test]
fn unknown_pairs_remain_unknown_with_exact_name_and_code_preserved() {
    let mut mismatch = Q02UCPU;
    mismatch[31..33].copy_from_slice(&[0x00, 0x48]);
    let parsed = decode_type_name(Route::default(), &mismatch).unwrap();
    assert_eq!(parsed.name, "Q02UCPU");
    assert_eq!(parsed.code, 0x4800);
    assert_eq!(parsed.family, ModelFamily::Unknown);

    let mut route_fallback = Q02UCPU;
    route_fallback[15..31].copy_from_slice(b"RCPU            ");
    route_fallback[31..33].copy_from_slice(&[0x60, 0x03]);
    let parsed = decode_type_name(Route::default(), &route_fallback).unwrap();
    assert_eq!(parsed.name, "RCPU");
    assert_eq!(parsed.code, 0x0360);
    assert_eq!(parsed.family, ModelFamily::Unknown);

    let mut special_q = Q02UCPU;
    special_q[15..31].copy_from_slice(b"Q02PHCPU        ");
    special_q[31..33].copy_from_slice(&[0x41, 0x00]);
    assert_eq!(
        decode_type_name(Route::default(), &special_q)
            .unwrap()
            .family,
        ModelFamily::Unknown
    );

    let mut special_r = Q02UCPU;
    special_r[15..31].copy_from_slice(b"R08PCPU         ");
    special_r[31..33].copy_from_slice(&[0x41, 0x48]);
    assert_eq!(
        decode_type_name(Route::default(), &special_r)
            .unwrap()
            .family,
        ModelFamily::Unknown
    );
}

#[test]
fn malformed_names_and_payload_lengths_are_distinct_from_unknown_models() {
    for bad_byte in [0, 0x7f, 0x80, 0xff] {
        let mut invalid = Q02UCPU;
        invalid[16] = bad_byte;
        assert_eq!(
            decode_type_name(Route::default(), &invalid),
            Err(ResponseError::MalformedName)
        );
    }
    let mut blank = Q02UCPU;
    blank[15..31].fill(b' ');
    assert_eq!(
        decode_type_name(Route::default(), &blank),
        Err(ResponseError::MalformedName)
    );

    let mut short = Q02UCPU.to_vec();
    short.pop();
    short[11] = 19;
    assert_eq!(
        decode_type_name(Route::default(), &short),
        Err(ResponseError::Framing)
    );
    let mut extra = Q02UCPU.to_vec();
    extra.push(0);
    extra[11] = 21;
    assert_eq!(
        decode_type_name(Route::default(), &extra),
        Err(ResponseError::Framing)
    );
}

#[test]
fn route_length_and_nonzero_end_code_are_checked_before_model_data() {
    for index in [0, 2, 4, 6, 7, 8, 10, 11] {
        let mut invalid = Q02UCPU;
        invalid[index] ^= 1;
        assert_eq!(
            decode_type_name(Route::default(), &invalid),
            Err(ResponseError::Framing)
        );
    }
    let mut refusal = Q02UCPU[..15].to_vec();
    refusal[11] = 2;
    refusal[13..15].copy_from_slice(&[0x59, 0xc0]);
    assert_eq!(
        decode_type_name(Route::default(), &refusal),
        Err(ResponseError::EndCode(0xc059))
    );
    refusal[2] ^= 1;
    assert_eq!(
        decode_type_name(Route::default(), &refusal),
        Err(ResponseError::Framing)
    );
}

#[test]
fn request_uses_the_selected_route_and_keeps_command_fixed() {
    let route = Route {
        serial: 0x1234,
        network: 2,
        pc: 0xfe,
        io: 0x0123,
        station: 5,
        timer: 0x0099,
        ..Route::default()
    };
    let req = encode_type_name(route);
    assert_eq!(
        &req[..15],
        &[0x54, 0, 0x34, 0x12, 0, 0, 2, 0xfe, 0x23, 1, 5, 6, 0, 0x99, 0]
    );
    assert_eq!(&req[15..], &[1, 1, 0, 0]);
}
