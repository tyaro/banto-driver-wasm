use banto_slmp_read_core::{
    basic_sd_count, decode_type_name, encode_read, plan_basic_sd, BasicArea as A, BasicEvidence,
    ModelFamily, PointCount, Route, Series, TypeName,
};

// Independent 0101 response wrapper. Pairs below are from the SLMP model-code
// table; SD numbers/widths are literal Q Appendix 3 and R Appendix 5 fixtures.
fn model(name: &str, code: u16) -> TypeName {
    let mut frame = [0u8; 33];
    frame[..15].copy_from_slice(&[0xd4, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 20, 0, 0, 0]);
    frame[15..31].fill(b' ');
    frame[15..15 + name.len()].copy_from_slice(name.as_bytes());
    frame[31..33].copy_from_slice(&code.to_le_bytes());
    decode_type_name(Route::default(), &frame).unwrap()
}

fn confirmed() -> BasicEvidence {
    BasicEvidence {
        route_confirmed: true,
        r_firmware: None,
    }
}

#[test]
fn q25prh_sd302_and_sd303_are_separate_internal_count_maps() {
    let q = model("Q25PRHCPU", 0x004c);
    assert_eq!(q.family, ModelFamily::Q25PrhWireOnly);
    let plan = plan_basic_sd(&q, A::DInternal).expect("QnPRH SD302");
    assert_eq!(
        (plan.device, plan.start, plan.count),
        (banto_slmp_read_core::Device::SD, 302, 1)
    );
    assert_eq!(
        basic_sd_count(&q, A::DInternal, confirmed(), Some(&[3014])),
        PointCount::Exact(3014)
    );
    assert_eq!(
        basic_sd_count(&q, A::DInternal, confirmed(), Some(&[32768])),
        PointCount::AtLeast(32768)
    );
    assert_eq!(
        basic_sd_count(&q, A::DInternal, confirmed(), None),
        PointCount::Unknown
    );
    let w = plan_basic_sd(&q, A::WInternal).expect("QnPRH SD303");
    assert_eq!((w.start, w.count), (303, 1));
    assert_eq!(
        basic_sd_count(&q, A::WInternal, confirmed(), Some(&[8192])),
        PointCount::Exact(8192)
    );
    for area in [A::D, A::W, A::M, A::ZR] {
        assert_eq!(plan_basic_sd(&q, area), None);
    }
}

#[test]
fn q_basic_sd_map_and_wire_plans_match_the_official_single_word_table() {
    let q = model("Q02UCPU", 0x0263);
    assert_eq!(q.family, ModelFamily::StandardQ);
    let fields = [
        (A::X, 290),
        (A::Y, 291),
        (A::M, 292),
        (A::L, 293),
        (A::B, 294),
        (A::F, 295),
        (A::SB, 296),
        (A::V, 297),
        (A::S, 298),
        (A::T, 299),
        (A::ST, 300),
        (A::C, 301),
        (A::DInternal, 302),
        (A::WInternal, 303),
        (A::SW, 304),
    ];
    for (area, sd) in fields {
        let plan = plan_basic_sd(&q, area).unwrap();
        assert_eq!(
            (plan.device, plan.start, plan.count),
            (banto_slmp_read_core::Device::SD, sd, 1)
        );
    }
    let request = encode_read(
        Route::default(),
        Series::Q,
        plan_basic_sd(&q, A::M).unwrap(),
    )
    .unwrap();
    assert_eq!(&request[17..25], &[0, 0, 0x24, 0x01, 0, 0xa9, 1, 0]);
    for area in [A::D, A::W, A::ZR, A::Z, A::LZ, A::RD, A::LT] {
        assert_eq!(plan_basic_sd(&q, area), None);
        assert_eq!(
            basic_sd_count(&q, area, confirmed(), Some(&[100])),
            PointCount::Unknown
        );
    }
}

#[test]
fn r_sd_pairs_and_single_words_match_the_official_map() {
    let r = model("R04ENCPU", 0x4805);
    assert_eq!(r.family, ModelFamily::StandardR);
    let fields = [
        (A::X, 260),
        (A::Y, 262),
        (A::M, 264),
        (A::B, 266),
        (A::SB, 268),
        (A::F, 270),
        (A::V, 272),
        (A::L, 274),
        (A::S, 276),
        (A::D, 280),
        (A::W, 282),
        (A::SW, 284),
        (A::T, 288),
        (A::ST, 290),
        (A::C, 292),
        (A::LT, 294),
        (A::LST, 296),
        (A::LC, 298),
        (A::ZR, 306),
        (A::RD, 308),
    ];
    for (area, sd) in fields {
        let plan = plan_basic_sd(&r, area).unwrap();
        assert_eq!((plan.start, plan.count), (sd, 2));
    }
    assert_eq!(
        (
            plan_basic_sd(&r, A::Z).unwrap().start,
            plan_basic_sd(&r, A::Z).unwrap().count
        ),
        (300, 1)
    );
    assert_eq!(
        (
            plan_basic_sd(&r, A::LZ).unwrap().start,
            plan_basic_sd(&r, A::LZ).unwrap().count
        ),
        (302, 1)
    );
    assert_eq!(plan_basic_sd(&r, A::DInternal), None);
    assert_eq!(plan_basic_sd(&r, A::WInternal), None);
    let request = encode_read(
        Route::default(),
        Series::R,
        plan_basic_sd(&r, A::S).unwrap(),
    )
    .unwrap();
    assert_eq!(&request[17..27], &[2, 0, 0x14, 0x01, 0, 0, 0xa9, 0, 2, 0]);
}

#[test]
fn q_legacy_saturation_is_a_lower_bound_not_an_exact_limit() {
    let q = model("Q02UCPU", 0x0263);
    for area in [A::M, A::B, A::DInternal, A::WInternal] {
        assert_eq!(
            basic_sd_count(&q, area, confirmed(), Some(&[32767])),
            PointCount::Exact(32767)
        );
        assert_eq!(
            basic_sd_count(&q, area, confirmed(), Some(&[32768])),
            PointCount::AtLeast(32768)
        );
    }
    assert_eq!(
        basic_sd_count(&q, A::X, confirmed(), Some(&[32768])),
        PointCount::Exact(32768)
    );
}

#[test]
fn r_pairs_are_low_word_first_and_z_lz_are_one_word() {
    let r = model("R04CPU", 0x4800);
    assert_eq!(
        basic_sd_count(&r, A::D, confirmed(), Some(&[0x1234, 0x5678])),
        PointCount::Exact(0x5678_1234)
    );
    assert_eq!(
        basic_sd_count(&r, A::Z, confirmed(), Some(&[0xffff])),
        PointCount::Exact(65535)
    );
    assert_eq!(
        basic_sd_count(&r, A::LZ, confirmed(), Some(&[42])),
        PointCount::Exact(42)
    );
    assert_eq!(
        basic_sd_count(&r, A::D, confirmed(), Some(&[1])),
        PointCount::Unknown
    );
    assert_eq!(
        basic_sd_count(&r, A::Z, confirmed(), Some(&[1, 2])),
        PointCount::Unknown
    );
}

#[test]
fn model_route_firmware_and_missing_reads_keep_counts_unknown() {
    let q = model("Q02UCPU", 0x0263);
    assert_eq!(
        basic_sd_count(&q, A::M, BasicEvidence::default(), Some(&[100])),
        PointCount::Unknown
    );
    assert_eq!(
        basic_sd_count(&q, A::M, confirmed(), None),
        PointCount::Unknown
    );
    let unknown = model("RCPU", 0x0360);
    assert_eq!(plan_basic_sd(&unknown, A::D), None);
    assert_eq!(
        basic_sd_count(&unknown, A::D, confirmed(), Some(&[1, 0])),
        PointCount::Unknown
    );

    let r = model("R04ENCPU", 0x4805);
    assert_eq!(
        basic_sd_count(&r, A::S, confirmed(), Some(&[128, 0])),
        PointCount::Unknown
    );
    for version in [0, 11] {
        let evidence = BasicEvidence {
            route_confirmed: true,
            r_firmware: Some(version),
        };
        assert_eq!(
            basic_sd_count(&r, A::S, evidence, Some(&[128, 0])),
            PointCount::Unknown
        );
    }
    let evidence = BasicEvidence {
        route_confirmed: true,
        r_firmware: Some(12),
    };
    assert_eq!(
        basic_sd_count(&r, A::S, evidence, Some(&[128, 0])),
        PointCount::Exact(128)
    );
    let r00 = model("R00CPU", 0x48a0);
    assert_eq!(
        basic_sd_count(&r00, A::S, confirmed(), Some(&[128, 0])),
        PointCount::Exact(128)
    );
}
