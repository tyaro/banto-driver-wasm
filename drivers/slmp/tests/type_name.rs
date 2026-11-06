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

fn classify_pair(name: &str, code: u16) -> ModelFamily {
    let mut frame = Q02UCPU;
    frame[15..31].fill(b' ');
    frame[15..15 + name.len()].copy_from_slice(name.as_bytes());
    frame[31..33].copy_from_slice(&code.to_le_bytes());
    let parsed = decode_type_name(Route::default(), &frame).unwrap();
    assert_eq!(parsed.name, name);
    assert_eq!(parsed.code, code);
    parsed.family
}

#[test]
fn g2a_process_universal_process_and_l_pairs_are_standard_q() {
    // SLMP Reference Manual SH-080956ENG N, printed p.139.
    for (name, code) in [
        ("Q02PHCPU", 0x0041),
        ("Q06PHCPU", 0x0042),
        ("Q12PHCPU", 0x0043),
        ("Q25PHCPU", 0x0044),
        ("Q04UDPVCPU", 0x0367),
        ("Q06UDPVCPU", 0x0368),
        ("Q13UDPVCPU", 0x036a),
        ("Q26UDPVCPU", 0x036c),
        ("L02SCPU", 0x0543),
        ("L02SCPU-P", 0x0543),
        ("L02CPU", 0x0541),
        ("L02CPU-P", 0x0541),
        ("L06CPU", 0x0544),
        ("L06CPU-P", 0x0544),
        ("L26CPU", 0x0545),
        ("L26CPU-P", 0x0545),
        ("L26CPU-BT", 0x0542),
        ("L26CPU-PBT", 0x0542),
    ] {
        assert_eq!(classify_pair(name, code), ModelFamily::StandardQ, "{name}");
    }
}

#[test]
fn g2a_q12prh_pair_is_wire_only_like_q25prh() {
    assert_eq!(
        classify_pair("Q12PRHCPU", 0x004b),
        ModelFamily::Q25PrhWireOnly
    );
    assert_eq!(
        classify_pair("Q25PRHCPU", 0x004c),
        ModelFamily::Q25PrhWireOnly
    );
}

#[test]
fn g2a_shared_code_name_mismatches_stay_unknown_and_old_pairs_are_unchanged() {
    for (name, code) in [
        ("Q02PHCPU", 0x0042),
        ("Q06PHCPU", 0x0041),
        ("Q02PHCPU", 0x0263),
        ("Q04UDPVCPU", 0x0368),
        ("Q06UDPVCPU", 0x0367),
        ("Q04UDPVCPU", 0x0366),
        ("L02CPU", 0x0543),
        ("L02SCPU", 0x0541),
        ("L26CPU", 0x0542),
        ("L26CPU-BT", 0x0545),
        ("L02CPU", 0x0263),
        ("Q12PRHCPU", 0x004c),
        ("Q25PRHCPU", 0x004b),
        ("Q12PRHCPU", 0x0043),
        // Not in the supported table.
        ("L04HCPU", 0x48c0),
        ("QS001CPU", 0x0230),
        ("L02CPU-X", 0x0541),
    ] {
        assert_eq!(classify_pair(name, code), ModelFamily::Unknown, "{name}");
    }
    for (name, code) in [
        ("Q02HCPU", 0x0041),
        ("Q06HCPU", 0x0042),
        ("Q12HCPU", 0x0043),
        ("Q25HCPU", 0x0044),
        ("Q04UDVCPU", 0x0367),
        ("Q06UDVCPU", 0x0368),
        ("Q13UDVCPU", 0x036a),
        ("Q26UDVCPU", 0x036c),
    ] {
        assert_eq!(classify_pair(name, code), ModelFamily::StandardQ, "{name}");
    }
}

const G2B_PAIRS: [(&str, u16); 21] = [
    ("R08PCPU", 0x4841),
    ("R16PCPU", 0x4842),
    ("R32PCPU", 0x4843),
    ("R120PCPU", 0x4844),
    ("R08PSFCPU", 0x4851),
    ("R16PSFCPU", 0x4852),
    ("R32PSFCPU", 0x4853),
    ("R120PSFCPU", 0x4854),
    ("R08SFCPU", 0x4891),
    ("R16SFCPU", 0x4892),
    ("R32SFCPU", 0x4893),
    ("R120SFCPU", 0x4894),
    ("MXR300-16", 0x48e9),
    ("MXR300-32", 0x48ea),
    ("MXR300-64", 0x48eb),
    ("MXR500-128", 0x48ee),
    ("MXR500-256", 0x48ef),
    // Standard R pairs next to them stay as they were.
    ("R08CPU", 0x4801),
    ("R120CPU", 0x4804),
    ("R04ENCPU", 0x4805),
    ("R120ENCPU", 0x4809),
];

#[test]
fn g2b_process_sil2_safety_and_mxr_pairs_are_standard_r() {
    for (name, code) in G2B_PAIRS {
        assert_eq!(classify_pair(name, code), ModelFamily::StandardR, "{name}");
    }
}

#[test]
fn g2b_mismatched_pairs_and_route_fallbacks_stay_unknown() {
    for (name, code) in [
        // Name and code from different groups.
        ("R08PCPU", 0x4851),
        ("R08PCPU", 0x4842),
        ("R08PSFCPU", 0x4841),
        ("R08SFCPU", 0x4801),
        ("R16SFCPU", 0x4891),
        ("R120PSFCPU", 0x4894),
        ("MXR300-16", 0x48ea),
        ("MXR500-256", 0x48ee),
        ("MXR300-16", 0x4801),
        // Not in the table.
        ("MXR500-64", 0x48ec),
        ("R04PCPU", 0x4840),
        // Route fallbacks when the connected station is not iQ-R / MX.
        ("MX-R", 0x037e),
        ("RCPU", 0x0360),
    ] {
        assert_eq!(classify_pair(name, code), ModelFamily::Unknown, "{name}");
    }
}
