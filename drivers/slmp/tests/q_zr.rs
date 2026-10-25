use banto_slmp_read_core::{
    decode_q_zr_count, decode_type_name, encode_read, plan_q_zr, PointCount, QZrEvidence, Route,
    Series, TypeName,
};

fn model(name: &str, code: u16) -> TypeName {
    let mut frame = [0u8; 33];
    frame[..15].copy_from_slice(&[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0]);
    frame[15..31].fill(b' ');
    frame[15..15 + name.len()].copy_from_slice(name.as_bytes());
    frame[31..33].copy_from_slice(&code.to_le_bytes());
    decode_type_name(Route::default(), &frame).unwrap()
}

fn confirmed() -> QZrEvidence {
    QZrEvidence {
        route_confirmed: true,
        firmware_compatible: Some(true),
        extended_d_or_w_at_least_1k: Some(true),
    }
}

#[test]
fn confirmed_qnu_condition_plans_exact_sd306_pair() {
    let q = model("Q02UCPU", 0x0263);
    let span = plan_q_zr(&q, confirmed()).unwrap();
    assert_eq!((span.start, span.count), (306, 2));
    let request = encode_read(Route::default(), Series::Q, span).unwrap();
    assert_eq!(&request[17..25], &[0, 0, 0x32, 0x01, 0, 0xa9, 2, 0]);
    assert_eq!(
        decode_q_zr_count(&q, confirmed(), Some(&[0x1234, 0x5678])),
        PointCount::Exact(0x5678_1234)
    );
    assert_eq!(
        decode_q_zr_count(&q, confirmed(), Some(&[0, 0])),
        PointCount::Exact(0)
    );
    let fast = model("Q03UDVCPU", 0x0366);
    assert!(plan_q_zr(&fast, confirmed()).is_some());
}

#[test]
fn q00uj_older_q_r_and_unknown_models_never_use_this_map() {
    for (name, code) in [
        ("Q00UJCPU", 0x0260),
        ("Q00JCPU", 0x0250),
        ("R04CPU", 0x4800),
        ("RCPU", 0x0360),
    ] {
        let model = model(name, code);
        assert_eq!(plan_q_zr(&model, confirmed()), None);
        assert_eq!(
            decode_q_zr_count(&model, confirmed(), Some(&[100, 0])),
            PointCount::Unknown
        );
    }
}

#[test]
fn missing_or_negative_applicability_evidence_stays_unknown() {
    let q = model("Q02UCPU", 0x0263);
    let base = confirmed();
    for evidence in [
        QZrEvidence::default(),
        QZrEvidence {
            route_confirmed: false,
            ..base
        },
        QZrEvidence {
            firmware_compatible: None,
            ..base
        },
        QZrEvidence {
            firmware_compatible: Some(false),
            ..base
        },
        QZrEvidence {
            extended_d_or_w_at_least_1k: None,
            ..base
        },
        QZrEvidence {
            extended_d_or_w_at_least_1k: Some(false),
            ..base
        },
    ] {
        assert_eq!(plan_q_zr(&q, evidence), None);
        assert_eq!(
            decode_q_zr_count(&q, evidence, Some(&[100, 0])),
            PointCount::Unknown
        );
    }
}

#[test]
fn missing_or_partial_sd_pair_and_sd647_single_word_cannot_set_zr_limit() {
    let q = model("Q02UCPU", 0x0263);
    for words in [None, Some(&[][..]), Some(&[64][..]), Some(&[1, 2, 3][..])] {
        assert_eq!(
            decode_q_zr_count(&q, confirmed(), words),
            PointCount::Unknown
        );
    }
    // SD647=64 means a truncated, shared file capacity in 1K-word units;
    // passing it as one word must not yield 64*1024 ZR points.
    assert_eq!(
        decode_q_zr_count(&q, confirmed(), Some(&[64])),
        PointCount::Unknown
    );
}
