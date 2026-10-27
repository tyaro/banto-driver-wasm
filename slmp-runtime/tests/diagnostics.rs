use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

mod diagnostic_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-diagnostics-v1" });
}
mod combined_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-and-diagnostics-v1" });
}
mod old_read_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver" });
}
mod old_write_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-v1" });
}

fn component() -> wasmtime::Result<(Engine, Component)> {
    let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
    let engine = Engine::default();
    let component = Component::from_file(&engine, path)?;
    Ok((engine, component))
}

fn response(serial: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![0xd4, 0];
    frame.extend_from_slice(&serial.to_le_bytes());
    frame.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
    frame.extend_from_slice(&(u16::try_from(body.len() + 2).unwrap()).to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
}

fn model_response(name: &str, code: u16) -> Vec<u8> {
    let mut data = [b' '; 18];
    data[..name.len()].copy_from_slice(name.as_bytes());
    data[16..].copy_from_slice(&code.to_le_bytes());
    response(1, &data)
}

fn spec(address: &str, data_type: &str) -> diagnostic_host::banto::driver::types::TagSpec {
    diagnostic_host::banto::driver::types::TagSpec {
        address: address.into(),
        data_type: data_type.into(),
        string_words: None,
    }
}

#[test]
fn all_four_worlds_bind_without_changing_old_exports() -> wasmtime::Result<()> {
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    old_read_host::Driver::instantiate(&mut store, &component, &linker)?;
    old_write_host::DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
    diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    combined_host::DriverWithWriteAndDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    if let Ok(path) = std::env::var("BANTO_SLMP_LEGACY_COMPONENT_WASM") {
        let old = Component::from_file(&engine, path)?;
        old_read_host::Driver::instantiate(&mut store, &old, &linker)?;
        assert!(
            diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &old, &linker)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn same_connection_diagnosis_reads_one_bounded_bit_sd_and_preserves_ranges() -> wasmtime::Result<()>
{
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};

    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("create");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101")
        .expect("step");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
        .expect("SD280-283")
        .expect("step");
    let mut dw = Vec::new();
    for word in [100u16, 0, 50, 0] {
        dw.extend_from_slice(&word.to_le_bytes());
    }
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&response(2, &dw)))?
        .expect("ready");
    let specs = vec![
        spec("D98", "u32"),
        spec("D99", "u32"),
        spec("W1", "u16"),
        spec("M10", "bit"),
        spec("M32", "bit"),
        spec("F0", "bit"),
        spec("V14", "bit"),
        spec("bad", "u16"),
        spec("LTN4294967295", "i32"),
    ];
    let send = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("diagnose");
    let Progress::Send(step) = send else {
        panic!("bit count SD should be requested");
    };
    assert_eq!(&step.send[15..17], &[0x01, 0x04]);
    assert_eq!(&step.send[17..19], &[0x02, 0x00]);
    assert_eq!(&step.send[19..23], &[6, 1, 0, 0]);
    assert!(guest
        .banto_driver_read()
        .call_plan(&mut store, conn, &specs)?
        .is_err());
    let mut bit_sd = Vec::new();
    let mut words = [0u16; 14];
    words[2] = 32; // M SD264-265
    words[4] = 16; // B SD266-267
    words[8] = 8; // F SD270-271
    for word in words {
        bit_sd.extend_from_slice(&word.to_le_bytes());
    }
    let done = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, Some(&response(3, &bit_sd)))?
        .expect("diagnose result");
    let Progress::Done(report) = done else {
        panic!("one extra SD read is enough");
    };
    assert_eq!(report.model.as_ref().expect("model").name, "R04CPU");
    assert_eq!(report.model.as_ref().expect("model").code, 0x4800);
    assert_eq!(report.model.as_ref().expect("model").raw_name.len(), 16);
    assert_eq!(report.tags.len(), specs.len());
    assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[1].verdict, TagVerdict::Outside(_)));
    assert!(matches!(report.tags[2].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[3].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[4].verdict, TagVerdict::Outside(_)));
    assert!(matches!(report.tags[5].verdict, TagVerdict::Inside(_)));
    assert!(
        matches!(&report.tags[6].verdict, TagVerdict::Invalid(reason) if reason.contains("one-scan edge"))
    );
    assert!(matches!(report.tags[7].verdict, TagVerdict::Invalid(_)));
    assert!(
        matches!(&report.tags[8].verdict, TagVerdict::Invalid(reason) if reason == "address-span-overflow")
    );
    assert_eq!(report.counts.len(), 8);
    let repeat = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("repeat");
    assert!(
        matches!(repeat, Progress::Done(_)),
        "SD is cached on this connection"
    );
    Ok(())
}

#[test]
fn q_internal_prefix_does_not_invent_an_extended_upper_limit() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{
        CountBound, Progress, TagVerdict,
    };
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("create");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
        .expect("SD302-303");
    let mut sd = Vec::new();
    sd.extend_from_slice(&32768u16.to_le_bytes());
    sd.extend_from_slice(&100u16.to_le_bytes());
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&response(2, &sd)))?
        .expect("ready");
    let specs = vec![
        spec("D32767", "u16"),
        spec("D32767", "u32"),
        spec("D32768", "u16"),
        spec("W62", "u32"),
        spec("W63", "u32"),
    ];
    let result = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("diagnose");
    let Progress::Done(report) = result else {
        panic!("D/W evidence is already in handshake");
    };
    assert!(matches!(report.counts[0].bound, CountBound::AtLeast(32768)));
    assert_eq!(report.counts[0].scope, "internal");
    assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[1].verdict, TagVerdict::Unknown(_)));
    assert!(matches!(report.tags[2].verdict, TagVerdict::Unknown(_)));
    assert!(matches!(report.tags[3].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[4].verdict, TagVerdict::Unknown(_)));
    Ok(())
}

#[test]
fn sd_refusal_is_unknown_and_an_abandoned_step_poisoned_until_reconnect() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    for refused in [true, false] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("SD280-283");
        let mut dw = Vec::new();
        for word in [100u16, 0, 50, 0] {
            dw.extend_from_slice(&word.to_le_bytes());
        }
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &dw)))?
            .expect("ready");
        let specs = vec![spec("F0", "bit")];
        let first = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("diagnose");
        assert!(matches!(first, Progress::Send(_)));
        if refused {
            let mut reply = response(3, &[]);
            reply[13..15].copy_from_slice(&0xc051u16.to_le_bytes());
            let result = guest
                .banto_driver_diagnostics_v1()
                .call_diagnose_next(&mut store, conn, &specs, Some(&reply))?
                .expect("refusal is item unknown");
            let Progress::Done(report) = result else {
                panic!("refusal must complete");
            };
            assert!(
                matches!(&report.tags[0].verdict, TagVerdict::Unknown(reason) if reason == "sd-refused")
            );
            assert!(matches!(
                guest
                    .banto_driver_diagnostics_v1()
                    .call_diagnose_next(&mut store, conn, &specs, None)?
                    .expect("cached refusal"),
                Progress::Done(_)
            ));
        } else {
            // No response after send is an abandoned step. Do not reuse its serial or resource.
            assert!(guest
                .banto_driver_diagnostics_v1()
                .call_diagnose_next(&mut store, conn, &specs, None)?
                .is_err());
            assert!(guest
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .is_err());
        }
    }
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("fresh connection");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
        .expect("D/W");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(
            &mut store,
            conn,
            Some(&response(2, &[100, 0, 0, 0, 0, 0, 0, 0])),
        )?
        .expect("ready");
    let specs = vec![spec("F0", "bit")];
    assert!(matches!(
        guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("send"),
        Progress::Send(_)
    ));
    // A wrong serial must be fatal and cannot be retried on this resource.
    assert!(guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, Some(&response(4, &[0; 28])))?
        .is_err());
    assert!(guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .is_err());
    let fresh = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("reconnect");
    assert!(guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, fresh, None)?
        .expect("fresh 0101")
        .is_some());
    Ok(())
}

#[test]
fn q_exact_bit_boundary_and_saturated_m_prefix_are_distinct() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{
        CountBound, Progress, TagVerdict,
    };
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("create");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
        .expect("SD302-303");
    let mut dw = Vec::new();
    for word in [32768u16, 100] {
        dw.extend_from_slice(&word.to_le_bytes());
    }
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&response(2, &dw)))?
        .expect("ready");
    let specs = vec![
        spec("YF", "bit"),
        spec("Y10", "bit"),
        spec("M32767", "bit"),
        spec("M32768", "bit"),
    ];
    let first = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("bit SD request");
    assert!(matches!(first, Progress::Send(_)));
    let mut bit_sd = Vec::new();
    for word in [0u16, 16, 32768, 0, 0] {
        bit_sd.extend_from_slice(&word.to_le_bytes());
    }
    let result = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, Some(&response(3, &bit_sd)))?
        .expect("bit SD result");
    let Progress::Done(report) = result else {
        panic!("one Q bit SD request expected");
    };
    assert!(matches!(report.counts[2].bound, CountBound::AtLeast(32768)));
    assert!(matches!(report.counts[3].bound, CountBound::Exact(16)));
    assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[1].verdict, TagVerdict::Outside(_)));
    assert!(matches!(report.tags[2].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[3].verdict, TagVerdict::Unknown(_)));
    Ok(())
}

#[test]
fn q_x_exact_count_uses_the_existing_five_word_request() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{
        CountBound, Progress, TagVerdict,
    };
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
        Setting {
            key: "word_order".into(),
            value: "high_low".into(),
        },
    ];
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("create");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
        .expect("D/W SD");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&response(2, &[100, 0, 100, 0])))?
        .expect("ready");
    let specs = vec![spec("X7FFF", "bit"), spec("X8000", "bit")];
    let Progress::Send(step) = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("send")
    else {
        panic!("Q X needs a count")
    };
    assert_eq!(&step.send[19..25], &[0x22, 1, 0, 0xa9, 5, 0]); // SD290, five words.
    let mut words = Vec::new();
    for word in [32768u16, 0, 0, 0, 0] {
        words.extend_from_slice(&word.to_le_bytes());
    }
    let Progress::Done(report) = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, Some(&response(3, &words)))?
        .expect("done")
    else {
        panic!("one request")
    };
    let x = report
        .counts
        .iter()
        .find(|count| count.area == "X")
        .expect("X count");
    assert_eq!(x.source, "SD290");
    assert!(matches!(x.bound, CountBound::Exact(32768)));
    assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[1].verdict, TagVerdict::Outside(_)));
    assert!(matches!(
        guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("cached"),
        Progress::Done(_)
    ));
    Ok(())
}

#[test]
fn r_sb_f_counts_are_low_high_and_family_specific() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{
        CountBound, Progress, TagVerdict,
    };
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
        Setting {
            key: "word_order".into(),
            value: "high_low".into(),
        },
    ];
    let conn = guest
        .banto_driver_session()
        .connection()
        .call_create(&mut store, &settings)?
        .expect("create");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
        .expect("D/W SD");
    guest
        .banto_driver_session()
        .connection()
        .call_handshake_next(
            &mut store,
            conn,
            Some(&response(2, &[100, 0, 0, 0, 0, 0, 0, 0])),
        )?
        .expect("ready");
    let mut specs = vec![
        spec("SB10001", "bit"),
        spec("SB10002", "bit"),
        spec("F2", "bit"),
        spec("F3", "bit"),
        spec("X0", "bit"),
    ];
    specs.extend((0..251).map(|_| spec("F0", "bit")));
    let mut too_many = specs.clone();
    too_many.push(spec("F0", "bit"));
    assert!(guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &too_many, None)?
        .is_err());
    let Progress::Send(step) = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("send")
    else {
        panic!("R SB/F need counts")
    };
    assert_eq!(&step.send[19..27], &[6, 1, 0, 0, 0xa9, 0, 14, 0]); // SD262, fourteen words.
    let mut words = [0u16; 14];
    words[6] = 2;
    words[7] = 1; // SB count 0x00010002, independent of tag word_order.
    words[8] = 3; // F decimal count.
    let mut data = Vec::new();
    for word in words {
        data.extend_from_slice(&word.to_le_bytes());
    }
    let Progress::Done(report) = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, Some(&response(3, &data)))?
        .expect("done")
    else {
        panic!("one request")
    };
    let sb = report
        .counts
        .iter()
        .find(|count| count.area == "SB")
        .expect("SB count");
    let f = report
        .counts
        .iter()
        .find(|count| count.area == "F")
        .expect("F count");
    assert_eq!(sb.source, "SD268-269");
    assert_eq!(f.source, "SD270-271");
    assert!(matches!(sb.bound, CountBound::Exact(65538)));
    assert!(matches!(f.bound, CountBound::Exact(3)));
    assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[1].verdict, TagVerdict::Outside(_)));
    assert!(matches!(report.tags[2].verdict, TagVerdict::Inside(_)));
    assert!(matches!(report.tags[3].verdict, TagVerdict::Outside(_)));
    assert!(matches!(report.tags[4].verdict, TagVerdict::Unknown(_)));
    assert_eq!(report.tags.len(), 256);
    assert!(matches!(
        guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("cached"),
        Progress::Done(_)
    ));
    Ok(())
}

#[test]
fn r_sb_and_f_each_trigger_one_existing_bounded_request() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    for area in ["SB", "F"] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("D/W");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[100, 0, 0, 0, 0, 0, 0, 0])),
            )?
            .expect("ready");
        let specs = vec![spec(&format!("{area}0"), "bit")];
        let Progress::Send(step) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("send")
        else {
            panic!("{area} must trigger SD")
        };
        assert_eq!(&step.send[19..27], &[6, 1, 0, 0, 0xa9, 0, 14, 0]);
        let mut words = [0u16; 14];
        words[if area == "SB" { 6 } else { 8 }] = 1;
        let mut data = Vec::new();
        for word in words {
            data.extend_from_slice(&word.to_le_bytes());
        }
        let Progress::Done(report) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, Some(&response(3, &data)))?
            .expect("done")
        else {
            panic!("one request")
        };
        assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
        assert!(matches!(
            guest
                .banto_driver_diagnostics_v1()
                .call_diagnose_next(&mut store, conn, &specs, None)?
                .expect("cached"),
            Progress::Done(_)
        ));
    }
    Ok(())
}

#[test]
fn handshake_acquired_bit_sd_is_reused_for_new_diagnostic_areas() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{
        CountBound, Progress, TagVerdict,
    };
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
        Setting {
            key: "d_u16_write_mode".into(),
            value: "enabled".into(),
        },
        Setting {
            key: "d_u16_write_allowlist".into(),
            value: "M0-M0".into(),
        },
    ];
    for (name, code, q) in [("Q02UCPU", 0x0263, true), ("R04CPU", 0x4800, false)] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
            .expect("D/W");
        let dw = if q {
            vec![100, 0, 100, 0]
        } else {
            vec![100, 0, 0, 0, 0, 0, 0, 0]
        };
        let step = guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &dw)))?
            .expect("bit SD")
            .expect("step");
        assert_eq!(
            &step.send[19..22],
            if q { &[0x22, 1, 0] } else { &[6, 1, 0] }
        );
        let mut words = vec![0u16; if q { 5 } else { 14 }];
        if q {
            words[0] = 8;
        } else {
            words[8] = 3;
        }
        let mut data = Vec::new();
        for word in words {
            data.extend_from_slice(&word.to_le_bytes());
        }
        assert!(guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(3, &data)))??
            .is_none());
        let specs = if q {
            vec![spec("X7", "bit"), spec("X8", "bit")]
        } else {
            vec![spec("SB0", "bit"), spec("F2", "bit"), spec("F3", "bit")]
        };
        let Progress::Done(report) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("cached diagnosis")
        else {
            panic!("handshake SD must be reused")
        };
        if q {
            assert!(matches!(
                report
                    .counts
                    .iter()
                    .find(|count| count.area == "X")
                    .unwrap()
                    .bound,
                CountBound::Exact(8)
            ));
            assert!(matches!(report.tags[0].verdict, TagVerdict::Inside(_)));
            assert!(matches!(report.tags[1].verdict, TagVerdict::Outside(_)));
        } else {
            assert!(matches!(
                report
                    .counts
                    .iter()
                    .find(|count| count.area == "SB")
                    .unwrap()
                    .bound,
                CountBound::Exact(0)
            ));
            assert!(matches!(report.tags[0].verdict, TagVerdict::Outside(_)));
            assert!(matches!(report.tags[1].verdict, TagVerdict::Inside(_)));
            assert!(matches!(report.tags[2].verdict, TagVerdict::Outside(_)));
        }
    }
    Ok(())
}

#[test]
fn inapplicable_and_unrecognized_models_do_not_request_bit_sd() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::Setting;
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    for (name, code, specs, dw) in [
        (
            "Q02UCPU",
            0x0263,
            vec![spec("SB0", "bit"), spec("F0", "bit")],
            Some(vec![100, 0, 100, 0]),
        ),
        (
            "R04CPU",
            0x4800,
            vec![spec("X0", "bit")],
            Some(vec![100, 0, 0, 0, 0, 0, 0, 0]),
        ),
        (
            "Q25PRHCPU",
            0x004c,
            vec![spec("X0", "bit")],
            Some(vec![100, 0, 100, 0]),
        ),
        ("Q02UCPU", 0x9999, vec![spec("X0", "bit")], None),
    ] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        let step = guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
            .expect("model");
        if let Some(dw) = dw {
            assert!(step.is_some());
            assert!(guest
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&response(2, &dw)))??
                .is_none());
        } else {
            assert!(step.is_none());
        }
        let Progress::Done(report) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("no SD for inapplicable area")
        else {
            panic!("unexpected bit SD request")
        };
        assert!(report
            .tags
            .iter()
            .all(|tag| matches!(tag.verdict, TagVerdict::Unknown(_))));
    }
    Ok(())
}

#[test]
fn multiword_and_string_tags_crossing_area_end_never_report_inside() -> wasmtime::Result<()> {
    use diagnostic_host::banto::driver::types::{Setting, TagSpec};
    use diagnostic_host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest =
        diagnostic_host::DriverWithDiagnosticsV1::instantiate(&mut store, &component, &linker)?;
    let settings = vec![
        Setting {
            key: "host".into(),
            value: "fixture.invalid".into(),
        },
        Setting {
            key: "port".into(),
            value: "5000".into(),
        },
    ];
    let string = |address: &str, words: u32| TagSpec {
        address: address.into(),
        data_type: "string".into(),
        string_words: Some(words),
    };
    // Every tag starts inside the configured count; all but D96/f64 and
    // W1E/u32 end beyond it. Strings are not a supported SLMP guest type.
    let specs = vec![
        spec("D99", "u32"),
        spec("D97", "u64"),
        spec("D99", "f32"),
        spec("D96", "f64"),
        spec("W1F", "u32"),
        spec("W1E", "u32"),
        string("D98", 4),
        string("W1E", 4),
    ];
    // (model, SD reply, expected numeric verdicts and reasons for D99..W1E)
    for (name, code, sd, expected) in [
        (
            "R04CPU",
            0x4800,
            Some(vec![100u16, 0, 32, 0]),
            [
                ("outside", "complete-configured-count"),
                ("outside", "complete-configured-count"),
                ("outside", "complete-configured-count"),
                ("inside", "same-connection-sd-guaranteed-span"),
                ("outside", "complete-configured-count"),
                ("inside", "same-connection-sd-guaranteed-span"),
            ],
        ),
        (
            "R04CPU",
            0x4800,
            None,
            [
                ("unknown", "sd-refused"),
                ("unknown", "sd-refused"),
                ("unknown", "sd-refused"),
                ("unknown", "sd-refused"),
                ("unknown", "sd-refused"),
                ("unknown", "sd-refused"),
            ],
        ),
        (
            "Q02UCPU",
            0x0263,
            Some(vec![100, 32]),
            [
                ("unknown", "configured-range-unproven"),
                ("unknown", "configured-range-unproven"),
                ("unknown", "configured-range-unproven"),
                ("inside", "same-connection-sd-guaranteed-span"),
                (
                    "unknown",
                    "internal-w-prefix-unproven; guest-read-path-unavailable",
                ),
                ("inside", "same-connection-sd-guaranteed-span"),
            ],
        ),
    ] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
            .expect("D/W SD request");
        let reply = match &sd {
            Some(words) => {
                let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
                response(2, &bytes)
            }
            None => {
                let mut refused = response(2, &[]);
                refused[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
                refused
            }
        };
        assert!(guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&reply))?
            .expect("ready")
            .is_none());
        let Progress::Done(report) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("diagnose")
        else {
            panic!("D/W evidence is already in handshake");
        };
        assert_eq!(report.tags.len(), specs.len());
        for (index, (kind, expected_reason)) in expected.iter().enumerate() {
            let (actual, reason) = match &report.tags[index].verdict {
                TagVerdict::Inside(reason) => ("inside", reason),
                TagVerdict::Outside(reason) => ("outside", reason),
                TagVerdict::Unknown(reason) => ("unknown", reason),
                TagVerdict::Invalid(reason) => ("invalid", reason),
                TagVerdict::Unsupported(reason) => ("unsupported", reason),
            };
            assert_eq!(
                (actual, reason.as_str()),
                (*kind, *expected_reason),
                "{name} {:?} {}",
                sd,
                specs[index].address
            );
        }
        for tag in &report.tags[6..] {
            assert!(
                matches!(&tag.verdict, TagVerdict::Invalid(reason) if !reason.is_empty()),
                "{name}: string {} must not be judged from a word prefix",
                tag.address
            );
        }
        // The read path never plans a tag this report found outside. Unknown
        // tags on a known wire may still be read explicitly by contract.
        let plans = guest
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs[..6])?
            .expect("plan");
        for plan in &plans {
            for &cover in &plan.covers {
                assert!(
                    !matches!(report.tags[cover as usize].verdict, TagVerdict::Outside(_)),
                    "{name}: {} planned although outside",
                    specs[cover as usize].address
                );
            }
        }
        conn.resource_drop(&mut store)?;
    }

    // Without a usable model no span is judged, even one that crosses an end.
    for (reply, model_reason, tag_reason) in [
        (
            Some(model_response("RCPU", 0x0360)),
            "unsupported-model-name-code-pair",
            "unsupported-model-name-code-pair",
        ),
        (
            Some(model_response("Q02UCPU", 0x9999)),
            "unsupported-model-name-code-pair",
            "unsupported-model-name-code-pair",
        ),
        (
            None,
            "0101-refused-or-unavailable",
            "0101-model-unavailable",
        ),
    ] {
        let conn = guest
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("create");
        guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        let reply = reply.unwrap_or_else(|| {
            let mut refused = response(1, &[]);
            refused[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
            refused
        });
        assert!(guest
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&reply))?
            .expect("no SD without a known model")
            .is_none());
        let Progress::Done(report) = guest
            .banto_driver_diagnostics_v1()
            .call_diagnose_next(&mut store, conn, &specs, None)?
            .expect("diagnose")
        else {
            panic!("no SD request without a known model");
        };
        assert_eq!(report.model_reason.as_deref(), Some(model_reason));
        assert_eq!(
            report.model.is_none(),
            model_reason == "0101-refused-or-unavailable"
        );
        for tag in &report.tags[..6] {
            assert!(
                matches!(&tag.verdict, TagVerdict::Unknown(reason) if reason == tag_reason),
                "{model_reason}: {} {:?}",
                tag.address,
                tag.verdict
            );
        }
        for tag in &report.tags[6..] {
            assert!(matches!(tag.verdict, TagVerdict::Invalid(_)));
        }
        conn.resource_drop(&mut store)?;
    }
    Ok(())
}
