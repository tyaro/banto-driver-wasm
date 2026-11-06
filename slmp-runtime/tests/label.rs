//! G3 global label fixtures through the built component: 041C reads, the
//! two-step 041C/141B write, model gating and diagnostics.

use wasmtime::component::{Component, Linker, ResourceAny};
use wasmtime::{Engine, Store};

mod host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-and-diagnostics-v1" });
}

use host::banto::driver::types::{ItemError, Setting, TagSpec, Value};
use host::exports::banto::driver::diagnostics_v1::{Progress, TagVerdict};
use host::exports::banto::driver::write_v1::{WriteItem, WriteProgress};
use host::DriverWithWriteAndDiagnosticsV1 as Guest;

fn load() -> wasmtime::Result<(Store<()>, Guest)> {
    let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
    let engine = Engine::default();
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest = Guest::instantiate(&mut store, &component, &linker)?;
    Ok((store, guest))
}

fn response(serial: u16, end_code: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![0xd4, 0];
    frame.extend_from_slice(&serial.to_le_bytes());
    frame.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
    frame.extend_from_slice(&(u16::try_from(body.len() + 2).unwrap()).to_le_bytes());
    frame.extend_from_slice(&end_code.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

fn model_response(name: &str, code: u16) -> Vec<u8> {
    let mut data = [b' '; 18];
    data[..name.len()].copy_from_slice(name.as_bytes());
    data[16..].copy_from_slice(&code.to_le_bytes());
    response(1, 0, &data)
}

fn spec(address: &str, data_type: &str) -> TagSpec {
    TagSpec {
        address: address.into(),
        data_type: data_type.into(),
        string_words: None,
    }
}

fn wire_name(name: &str) -> Vec<u8> {
    let units = name.encode_utf16().collect::<Vec<_>>();
    let mut out = (units.len() as u16).to_le_bytes().to_vec();
    out.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
    out
}

fn request(serial: u16, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0x54, 0];
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
    out.extend_from_slice(&(u16::try_from(body.len() + 2).unwrap()).to_le_bytes());
    out.extend_from_slice(&[0x10, 0]);
    out.extend_from_slice(body);
    out
}

fn entries(list: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = (list.len() as u16).to_le_bytes().to_vec();
    for (type_id, data) in list {
        out.extend_from_slice(&[*type_id, 0]);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

const ALLOW: &str = "@Tank1.Level;@Arr[0-9]";

/// 4E own-station connection that has finished discovery as `name`/`code`.
/// The next request serial is 3 (R) or 4 (QnU, which also reads SD310-311).
fn ready(
    store: &mut Store<()>,
    guest: &Guest,
    name: &str,
    code: u16,
) -> wasmtime::Result<ResourceAny> {
    let settings = [
        ("host", "fixture.invalid"),
        ("port", "5000"),
        ("d_u16_write_mode", "enabled"),
        ("label_write_allowlist", ALLOW),
    ]
    .map(|(key, value)| Setting {
        key: key.into(),
        value: value.into(),
    });
    let session = guest.banto_driver_session().connection();
    let conn = session
        .call_create(&mut *store, &settings)?
        .expect("create");
    session
        .call_handshake_next(&mut *store, conn, None)?
        .expect("0101")
        .expect("send");
    session
        .call_handshake_next(&mut *store, conn, Some(&model_response(name, code)))?
        .expect("SD")
        .expect("send");
    // R reads SD280-283 (two u32), Q SD302-303 (two u16).
    let sd = vec![0; if name.starts_with('R') { 8 } else { 4 }];
    let next = session
        .call_handshake_next(&mut *store, conn, Some(&response(2, 0, &sd)))?
        .expect("after SD");
    if next.is_some() {
        assert!(session
            .call_handshake_next(&mut *store, conn, Some(&response(3, 0, &[0; 4])))?
            .expect("SD310")
            .is_none());
    }
    Ok(conn)
}

#[test]
fn label_addresses_parse_through_the_component() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let address = guest.banto_driver_address();
    let parsed = address
        .call_parse(&mut store, "@タンク[01,2]", "f64", None)?
        .expect("label");
    assert_eq!(parsed.canonical, "@タンク[1,2]");
    assert_eq!(parsed.area, "@");
    assert_eq!(parsed.index, None);
    assert_eq!(parsed.word_count, 4);
    assert!(parsed.writable_area);
    for (text, data_type) in [
        ("@X.3", "bit"),
        ("@A", "i64"),
        ("@", "u16"),
        ("@A B", "u16"),
    ] {
        assert!(address
            .call_parse(&mut store, text, data_type, None)?
            .is_err());
    }
    assert_eq!(
        address.call_offset(&mut store, "@M[1,2]", "u16", None, 3)?,
        Ok("@M[1,5]".into())
    );
    assert!(address
        .call_offset(&mut store, "@Tank1.Level", "u16", None, 1)?
        .is_err());
    Ok(())
}

#[test]
fn r_reads_labels_with_041c_and_rejects_type_mismatches_per_item() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = ready(&mut store, &guest, "R04CPU", 0x4800)?;
    let specs = vec![
        spec("@Tank1.Level", "f32"),
        spec("@Arr[3]", "bit"),
        spec("@Count", "u16"),
    ];
    let read = guest.banto_driver_read();
    let plans = read.call_plan(&mut store, conn, &specs)?.expect("plan");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].covers, vec![0, 1, 2]);
    let mut body = vec![0x1c, 0x04, 0, 0, 3, 0, 0, 0];
    for name in ["Tank1.Level", "Arr[3]", "Count"] {
        body.extend(wire_name(name));
    }
    assert_eq!(plans[0].step.send, request(3, &body));
    let reply = response(
        3,
        0,
        &entries(&[(6, &2.5f32.to_le_bytes()), (1, &[1, 0]), (4, &[1, 0])]),
    );
    let results = read
        .call_decode(&mut store, conn, &plans[0], &specs, &reply)?
        .expect("decode");
    assert!(matches!(results[0], Ok(Value::Float(v)) if v == 2.5));
    assert!(matches!(results[1], Ok(Value::Bool(true))));
    assert!(matches!(&results[2], Err(ItemError::Rejected(m)) if m.contains("ID is 4")));
    // A refused 041C is a device error for every item of the frame.
    let plans = read.call_plan(&mut store, conn, &specs)?.expect("plan");
    let results = read
        .call_decode(
            &mut store,
            conn,
            &plans[0],
            &specs,
            &response(4, 0x40c0, &[]),
        )?
        .expect("decode");
    assert!(results
        .iter()
        .all(|r| matches!(r, Err(ItemError::Device(e)) if e.code == 0x40c0)));
    // A malformed response is fatal.
    let plans = read.call_plan(&mut store, conn, &specs)?.expect("plan");
    assert!(read
        .call_decode(
            &mut store,
            conn,
            &plans[0],
            &specs,
            &response(5, 0, &[3, 0])
        )?
        .is_err());
    Ok(())
}

#[test]
fn r_writes_labels_after_a_type_check_and_never_on_a_mismatch() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = ready(&mut store, &guest, "R04CPU", 0x4800)?;
    let write = guest.banto_driver_write_v1();
    let items = vec![
        WriteItem {
            spec: spec("@Tank1.Level", "f32"),
            value: Value::Float(2.5),
        },
        WriteItem {
            spec: spec("@Arr[9]", "bit"),
            value: Value::Bool(true),
        },
    ];
    let WriteProgress::Send(check) = write
        .call_write_next(&mut store, conn, &items, None)?
        .expect("check")
    else {
        panic!("041C check first");
    };
    let mut body = vec![0x1c, 0x04, 0, 0, 2, 0, 0, 0];
    body.extend(wire_name("Tank1.Level"));
    body.extend(wire_name("Arr[9]"));
    assert_eq!(check.step.send, request(3, &body));
    let types = response(3, 0, &entries(&[(6, &[0; 4]), (1, &[0, 0])]));
    let WriteProgress::Send(step) = write
        .call_write_next(&mut store, conn, &items, Some(&types))?
        .expect("write")
    else {
        panic!("141B after a matching check");
    };
    let mut body = vec![0x1b, 0x14, 0, 0, 2, 0, 0, 0];
    body.extend(wire_name("Tank1.Level"));
    body.extend_from_slice(&[4, 0]);
    body.extend_from_slice(&2.5f32.to_le_bytes());
    body.extend(wire_name("Arr[9]"));
    body.extend_from_slice(&[2, 0, 1, 0]);
    assert_eq!(step.step.send, request(4, &body));
    let WriteProgress::Done(results) = write
        .call_write_next(&mut store, conn, &items, Some(&response(4, 0, &[])))?
        .expect("ack")
    else {
        panic!("done");
    };
    assert!(results.iter().all(Result::is_ok));

    // A mismatched type at the check writes nothing: the next request is
    // serial 6, so no 141B took serial 6 in between.
    write
        .call_write_next(&mut store, conn, &items, None)?
        .expect("check");
    let wrong = response(5, 0, &entries(&[(5, &[0; 4]), (1, &[0, 0])]));
    let WriteProgress::Done(results) = write
        .call_write_next(&mut store, conn, &items, Some(&wrong))?
        .expect("refused")
    else {
        panic!("a mismatch must not send 141B");
    };
    assert!(results
        .iter()
        .all(|r| matches!(r, Err(ItemError::Rejected(_)))));
    let WriteProgress::Send(next) = write
        .call_write_next(&mut store, conn, &items, None)?
        .expect("check again")
    else {
        panic!("check");
    };
    assert_eq!(&next.step.send[2..4], &6u16.to_le_bytes());

    // Outside the allowlist or mixed with a device tag: refused before send.
    let (mut store, guest) = load()?;
    let conn = ready(&mut store, &guest, "R04CPU", 0x4800)?;
    let write = guest.banto_driver_write_v1();
    for items in [
        vec![WriteItem {
            spec: spec("@Arr[10]", "u16"),
            value: Value::Uint(1),
        }],
        vec![
            WriteItem {
                spec: spec("@Arr[1]", "u16"),
                value: Value::Uint(1),
            },
            WriteItem {
                spec: spec("D0", "u16"),
                value: Value::Uint(1),
            },
        ],
    ] {
        let WriteProgress::Done(results) = write
            .call_write_next(&mut store, conn, &items, None)?
            .expect("refused")
        else {
            panic!("must not send");
        };
        assert!(results
            .iter()
            .all(|r| matches!(r, Err(ItemError::Rejected(_)))));
    }
    Ok(())
}

#[test]
fn standard_q_does_not_send_label_commands() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = ready(&mut store, &guest, "Q02UCPU", 0x0263)?;
    let specs = vec![spec("@Tank1.Level", "u16")];
    let plans = guest
        .banto_driver_read()
        .call_plan(&mut store, conn, &specs)?
        .expect("plan");
    assert!(plans.is_empty(), "the label stays uncovered (Unsupported)");
    let items = vec![WriteItem {
        spec: spec("@Tank1.Level", "u16"),
        value: Value::Uint(1),
    }];
    let WriteProgress::Done(results) = guest
        .banto_driver_write_v1()
        .call_write_next(&mut store, conn, &items, None)?
        .expect("refused")
    else {
        panic!("must not send");
    };
    assert!(matches!(results[0], Err(ItemError::Unsupported(_))));
    Ok(())
}

#[test]
fn diagnostics_report_labels_as_unknown_without_plc_access() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = ready(&mut store, &guest, "R04CPU", 0x4800)?;
    let specs = vec![spec("@Tank1.Level", "u16"), spec("@X.3", "bit")];
    let Progress::Done(report) = guest
        .banto_driver_diagnostics_v1()
        .call_diagnose_next(&mut store, conn, &specs, None)?
        .expect("diagnose")
    else {
        panic!("labels need no PLC access");
    };
    assert!(matches!(&report.tags[0].verdict, TagVerdict::Unknown(r) if r == "label"));
    assert!(matches!(&report.tags[1].verdict, TagVerdict::Invalid(_)));
    Ok(())
}
