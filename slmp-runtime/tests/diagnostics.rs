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
    assert!(matches!(report.tags[5].verdict, TagVerdict::Unknown(_)));
    assert!(matches!(report.tags[6].verdict, TagVerdict::Invalid(_)));
    assert!(
        matches!(&report.tags[7].verdict, TagVerdict::Invalid(reason) if reason == "address-span-overflow")
    );
    assert_eq!(report.counts.len(), 6);
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
        let specs = vec![spec("M10", "bit")];
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
