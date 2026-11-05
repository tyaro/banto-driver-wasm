use banto_slmp_read_core::{
    decode_q_extension_count, decode_q_w_total, decode_type_name, encode_read, plan_q_extension,
    plan_q_w_total, PointCount, QExtensionArea as A, QExtensionEvidence, Route, Series, TypeName,
};

// Independent 0101 response fixture; QCPU Appendix 3 supplies the SD pairs.
fn model(name: &str, code: u16) -> TypeName {
    let mut frame = [0u8; 33];
    frame[..15].copy_from_slice(&[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0]);
    frame[15..31].fill(b' ');
    frame[15..15 + name.len()].copy_from_slice(name.as_bytes());
    frame[31..33].copy_from_slice(&code.to_le_bytes());
    decode_type_name(Route::default(), &frame).unwrap()
}

fn evidence() -> QExtensionEvidence {
    QExtensionEvidence {
        route_confirmed: true,
        version_compatible: Some(true),
        serial_first_five: Some(10042),
    }
}

#[test]
fn qnu_m_b_uses_sd286_289_only_with_confirmed_serial_and_route() {
    for (name, code) in [
        ("Q00UJCPU", 0x0260),
        ("Q02UCPU", 0x0263),
        ("Q03UDVCPU", 0x0366),
    ] {
        let q = model(name, code);
        for (area, start, address) in [
            (A::MTotal, 286, [0x1e, 0x01]),
            (A::BTotal, 288, [0x20, 0x01]),
        ] {
            let span = plan_q_extension(&q, area, evidence()).unwrap();
            assert_eq!((span.start, span.count), (start, 2));
            let request = encode_read(Route::default(), Series::Q, span).unwrap();
            assert_eq!(
                &request[17..25],
                &[0, 0, address[0], address[1], 0, 0xa9, 2, 0]
            );
            assert_eq!(
                decode_q_extension_count(&q, area, evidence(), Some(&[0x8000, 0])),
                PointCount::Exact(32768)
            );
        }
    }
}

#[test]
fn serial_and_version_uncertainty_never_becomes_a_known_m_b_count() {
    let q = model("Q02UCPU", 0x0263);
    for ev in [
        QExtensionEvidence::default(),
        QExtensionEvidence {
            route_confirmed: false,
            ..evidence()
        },
        QExtensionEvidence {
            version_compatible: None,
            ..evidence()
        },
        QExtensionEvidence {
            version_compatible: Some(false),
            ..evidence()
        },
        QExtensionEvidence {
            serial_first_five: None,
            ..evidence()
        },
        QExtensionEvidence {
            serial_first_five: Some(10041),
            ..evidence()
        },
    ] {
        assert_eq!(plan_q_extension(&q, A::MTotal, ev), None);
        assert_eq!(
            decode_q_extension_count(&q, A::BTotal, ev, Some(&[100, 0])),
            PointCount::Unknown
        );
    }
}

#[test]
fn only_classified_qnudv_models_use_fast_internal_d_w_pairs() {
    for (name, code) in [
        ("Q03UDVCPU", 0x0366),
        ("Q04UDVCPU", 0x0367),
        ("Q06UDVCPU", 0x0368),
        ("Q13UDVCPU", 0x036a),
        ("Q26UDVCPU", 0x036c),
    ] {
        let q = model(name, code);
        for (area, start) in [(A::FastDInternal, 282), (A::FastWInternal, 284)] {
            let span = plan_q_extension(&q, area, evidence()).unwrap();
            assert_eq!((span.start, span.count), (start, 2));
            // Nonzero high word is a codec fixture, not a claimed possible PLC setting.
            assert_eq!(
                decode_q_extension_count(&q, area, evidence(), Some(&[0, 1])),
                PointCount::Exact(65536)
            );
            assert_eq!(
                decode_q_extension_count(&q, area, evidence(), Some(&[0])),
                PointCount::Unknown
            );
        }
    }
    for (name, code) in [
        ("Q02UCPU", 0x0263),
        ("Q04UDPVCPU", 0x0367),
        ("Q06PHCPU", 0x0042),
        ("L26CPU", 0x0545),
        ("Q00JCPU", 0x0250),
        ("R04CPU", 0x4800),
        ("RCPU", 0x0360),
    ] {
        let q = model(name, code);
        // QnU (code-based) models, including QnUDPV, may plan SD286-289; the
        // SD282-285 fast D/W pair stays limited to the QnUDV names.
        if !matches!(name, "Q02UCPU" | "Q04UDPVCPU") {
            assert_eq!(plan_q_extension(&q, A::MTotal, evidence()), None);
        }
        assert_eq!(plan_q_extension(&q, A::FastDInternal, evidence()), None);
        assert_eq!(
            decode_q_extension_count(&q, A::FastWInternal, evidence(), Some(&[100, 0])),
            PointCount::Unknown
        );
    }
}

#[test]
fn exact_pair_is_required_and_no_d_w_total_or_basic_limit_is_inferred() {
    let q = model("Q03UDVCPU", 0x0366);
    for words in [None, Some(&[][..]), Some(&[1][..]), Some(&[1, 2, 3][..])] {
        assert_eq!(
            decode_q_extension_count(&q, A::FastDInternal, evidence(), words),
            PointCount::Unknown
        );
    }
    assert_eq!(
        decode_q_extension_count(&q, A::FastDInternal, evidence(), Some(&[0, 0])),
        PointCount::Exact(0)
    );
    // High word here exercises the codec; it does not prove an actual PLC capacity.
    // The pair is internal D, not SD302's saturated legacy word or SD308's total.
    assert_eq!(
        decode_q_extension_count(&q, A::FastDInternal, evidence(), Some(&[0xffff, 1])),
        PointCount::Exact(0x1ffff)
    );
}

#[test]
fn sd310_w_total_is_planned_for_classified_qnu_except_q00uj() {
    for (name, code) in [
        ("Q00UCPU", 0x0261),
        ("Q01UCPU", 0x0262),
        ("Q02UCPU", 0x0263),
        ("Q10UDHCPU", 0x0266),
        ("Q20UDEHCPU", 0x0267),
        ("Q03UDECPU", 0x0268),
        ("Q04UDHCPU", 0x0269),
        ("Q06UDEHCPU", 0x026a),
        ("Q13UDHCPU", 0x026b),
        ("Q26UDEHCPU", 0x026c),
        ("Q50UDEHCPU", 0x026d),
        ("Q100UDEHCPU", 0x026e),
        ("Q03UDVCPU", 0x0366),
        ("Q04UDVCPU", 0x0367),
        ("Q06UDVCPU", 0x0368),
        ("Q13UDVCPU", 0x036a),
        ("Q26UDVCPU", 0x036c),
        // G2a: universal process CPU is QnU.
        ("Q04UDPVCPU", 0x0367),
        ("Q06UDPVCPU", 0x0368),
        ("Q13UDPVCPU", 0x036a),
        ("Q26UDPVCPU", 0x036c),
    ] {
        let q = model(name, code);
        let span = plan_q_w_total(&q).unwrap();
        assert_eq!((span.start, span.count), (310, 2));
        let request = encode_read(Route::default(), Series::Q, span).unwrap();
        assert_eq!(&request[17..25], &[0, 0, 0x36, 0x01, 0, 0xa9, 2, 0]);
        // Low word first.
        assert_eq!(decode_q_w_total(&q, Some(&[0x3000, 1])), Some(0x13000));
        assert_eq!(decode_q_w_total(&q, Some(&[0x3000])), None);
        assert_eq!(decode_q_w_total(&q, Some(&[1, 2, 3])), None);
        assert_eq!(decode_q_w_total(&q, None), None);
    }
}

#[test]
fn sd310_w_total_is_not_planned_for_other_models() {
    for (name, code) in [
        ("Q00UJCPU", 0x0260),
        ("Q00JCPU", 0x0250),
        ("Q00CPU", 0x0251),
        ("Q06HCPU", 0x0042),
        ("Q25HCPU", 0x0044),
        ("Q25PRHCPU", 0x004c),
        ("Q12PRHCPU", 0x004b),
        // G2a: process CPU (non-U) and MELSEC-L get no SD310-311 plan.
        ("Q02PHCPU", 0x0041),
        ("Q25PHCPU", 0x0044),
        ("L02SCPU", 0x0543),
        ("L02CPU-P", 0x0541),
        ("L06CPU", 0x0544),
        ("L26CPU", 0x0545),
        ("L26CPU-PBT", 0x0542),
        ("R04CPU", 0x4800),
        ("Q03UDVCPU", 0x0367),
        ("RCPU", 0x0360),
    ] {
        let q = model(name, code);
        assert_eq!(plan_q_w_total(&q), None, "{name}");
        assert_eq!(decode_q_w_total(&q, Some(&[0x3000, 0])), None, "{name}");
    }
}

#[test]
fn g2a_qnudpv_plans_m_b_totals_but_not_the_fast_d_w_pair_and_l_plans_nothing() {
    let q = model("Q06UDPVCPU", 0x0368);
    assert_eq!(plan_q_extension(&q, A::FastDInternal, evidence()), None);
    assert_eq!(plan_q_extension(&q, A::FastWInternal, evidence()), None);
    let m = plan_q_extension(&q, A::MTotal, evidence()).unwrap();
    assert_eq!((m.start, m.count), (286, 2));
    for (name, code) in [
        ("L02CPU", 0x0541),
        ("L26CPU-BT", 0x0542),
        ("Q12PHCPU", 0x0043),
    ] {
        let q = model(name, code);
        for area in [A::MTotal, A::BTotal, A::FastDInternal, A::FastWInternal] {
            assert_eq!(plan_q_extension(&q, area, evidence()), None, "{name}");
        }
    }
}
