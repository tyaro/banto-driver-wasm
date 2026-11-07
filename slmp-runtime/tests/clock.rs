//! H clock set fixtures through the built component: the four-step iQ-R
//! procedure, SM210/SM211 outcomes, end codes, malformed responses and the
//! refusals made before anything is sent. Most tests bind the small
//! `driver-with-clock-v1` world the Hub uses; one binds the full world.

use wasmtime::component::{Component, Linker, ResourceAny};
use wasmtime::{Engine, Store};

mod clock_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-clock-v1" });
}
mod full_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-diagnostics-and-clock-v1" });
}

use clock_host::banto::driver::types::{FatalError, ItemError, Setting};
use clock_host::exports::banto::driver::clock_v1::{CivilTime, ClockProgress};
use clock_host::DriverWithClockV1 as Guest;

fn component() -> wasmtime::Result<(Engine, Component)> {
    let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
    let engine = Engine::default();
    let component = Component::from_file(&engine, path)?;
    Ok((engine, component))
}

fn load() -> wasmtime::Result<(Store<()>, Guest)> {
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest = Guest::instantiate(&mut store, &component, &linker)?;
    Ok((store, guest))
}

fn header_len(request: &[u8]) -> usize {
    if request[0] == 0x54 {
        15
    } else {
        11
    }
}

/// A response to `request`: 4E echoes its serial; both echo its route.
fn reply_to(request: &[u8], end_code: u16, body: &[u8]) -> Vec<u8> {
    let (mut out, route_at) = if request[0] == 0x54 {
        (vec![0xd4, 0, request[2], request[3], 0, 0], 6)
    } else {
        (vec![0xd0, 0], 2)
    };
    out.extend_from_slice(&request[route_at..route_at + 5]);
    out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_le_bytes());
    out.extend_from_slice(&end_code.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Settings for the fixture host plus `extra`.
fn settings(extra: &[(&str, &str)]) -> Vec<Setting> {
    [("host", "fixture.invalid"), ("port", "5000")]
        .iter()
        .chain(extra)
        .map(|(key, value)| Setting {
            key: (*key).into(),
            value: (*value).into(),
        })
        .collect()
}

/// Answer 0101 with `name`/`code` and every SD word read with zeros until
/// the handshake completes.
fn connect(
    store: &mut Store<()>,
    guest: &Guest,
    extra: &[(&str, &str)],
    name: &str,
    code: u16,
) -> wasmtime::Result<ResourceAny> {
    let session = guest.banto_driver_session().connection();
    let conn = session
        .call_create(&mut *store, &settings(extra))?
        .expect("create");
    let mut prev = None;
    while let Some(step) = session
        .call_handshake_next(&mut *store, conn, prev.as_deref())?
        .expect("handshake")
    {
        let h = header_len(&step.send);
        let body = if step.send[h..h + 2] == [1, 1] {
            let mut model = vec![b' '; 18];
            model[..name.len()].copy_from_slice(name.as_bytes());
            model[16..].copy_from_slice(&code.to_le_bytes());
            model
        } else {
            let n = step.send.len();
            let count = u16::from_le_bytes([step.send[n - 2], step.send[n - 1]]);
            vec![0; usize::from(count) * 2]
        };
        prev = Some(reply_to(&step.send, 0, &body));
    }
    Ok(conn)
}

fn clock(
    store: &mut Store<()>,
    guest: &Guest,
    conn: ResourceAny,
    time: CivilTime,
    prev: Option<&[u8]>,
) -> wasmtime::Result<Result<ClockProgress, FatalError>> {
    guest
        .banto_driver_clock_v1()
        .call_set_clock_next(&mut *store, conn, time, prev)
}

fn send_of(progress: Result<ClockProgress, FatalError>) -> Vec<u8> {
    match progress {
        Ok(ClockProgress::Send(step)) => step.send,
        other => panic!("clock step must send: {other:?}"),
    }
}

fn done_of(progress: Result<ClockProgress, FatalError>) -> Result<(), ItemError> {
    match progress {
        Ok(ClockProgress::Done(result)) => result,
        other => panic!("clock run must finish: {other:?}"),
    }
}

/// SM210 (high nibble) and SM211 (low nibble).
fn status(sm210: bool, sm211: bool) -> [u8; 1] {
    [(u8::from(sm210) << 4) | u8::from(sm211)]
}

const TIME: CivilTime = CivilTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 12,
    minute: 22,
    second: 5,
};

/// Command onward, as in the SLMP manual (iQ-R device specification).
const STATUS_BODY: [u8; 12] = [1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 2, 0];
const DATA_BODY: [u8; 26] = [
    1, 0x14, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0, 7, 0, 0xea, 0x07, 11, 0, 6, 0, 12, 0, 22, 0, 5, 0, 5, 0,
];
const SET_BODY: [u8; 13] = [1, 0x14, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 1, 0, 0x10];

/// Check the header (frame, serial, own route, length, timer 0010H) and
/// return the body from the command onward.
fn body(request: &[u8], serial: Option<u16>) -> &[u8] {
    let h = header_len(request);
    let route_at = if let Some(serial) = serial {
        assert_eq!(
            &request[..6],
            &[0x54, 0, serial as u8, (serial >> 8) as u8, 0, 0]
        );
        6
    } else {
        assert_eq!(&request[..2], &[0x50, 0]);
        2
    };
    assert_eq!(&request[route_at..route_at + 5], &[0, 0xff, 0xff, 3, 0]);
    let length = u16::from_le_bytes([request[route_at + 5], request[route_at + 6]]);
    assert_eq!(usize::from(length), request.len() - (h - 2));
    assert_eq!(&request[h - 2..h], &[0x10, 0]);
    &request[h..]
}

/// Run a 4E R connection to step 4 and return that request.
fn to_confirm(
    store: &mut Store<()>,
    guest: &Guest,
    conn: ResourceAny,
) -> wasmtime::Result<Vec<u8>> {
    let check = send_of(clock(store, guest, conn, TIME, None)?);
    let data = send_of(clock(
        store,
        guest,
        conn,
        TIME,
        Some(&reply_to(&check, 0, &status(false, false))),
    )?);
    let set = send_of(clock(
        store,
        guest,
        conn,
        TIME,
        Some(&reply_to(&data, 0, &[])),
    )?);
    Ok(send_of(clock(
        store,
        guest,
        conn,
        TIME,
        Some(&reply_to(&set, 0, &[])),
    )?))
}

const R: (&str, u16) = ("R04CPU", 0x4800);
const ENABLED: (&str, &str) = ("clock_set", "enabled");

#[test]
fn full_world_sets_the_clock_with_four_exact_4e_steps() -> wasmtime::Result<()> {
    use full_host::banto::driver::types::Setting;
    use full_host::exports::banto::driver::clock_v1::{CivilTime, ClockProgress};
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest = full_host::DriverWithWriteDiagnosticsAndClockV1::instantiate(
        &mut store, &component, &linker,
    )?;
    let settings =
        [("host", "fixture.invalid"), ("port", "5000"), ENABLED].map(|(key, value)| Setting {
            key: key.into(),
            value: value.into(),
        });
    let session = guest.banto_driver_session().connection();
    let conn = session.call_create(&mut store, &settings)?.expect("create");
    let step = session
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101")
        .expect("send");
    let mut model = vec![b' '; 18];
    model[..6].copy_from_slice(b"R04CPU");
    model[16..].copy_from_slice(&0x4800u16.to_le_bytes());
    let step = session
        .call_handshake_next(&mut store, conn, Some(&reply_to(&step.send, 0, &model)))?
        .expect("SD280-283")
        .expect("send");
    assert!(session
        .call_handshake_next(&mut store, conn, Some(&reply_to(&step.send, 0, &[0; 8])))?
        .expect("ready")
        .is_none());
    let time = CivilTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 12,
        minute: 22,
        second: 5,
    };
    let clock = guest.banto_driver_clock_v1();
    let mut prev: Option<Vec<u8>> = None;
    let expected: [(&[u8], &[u8]); 4] = [
        (&STATUS_BODY, &status(false, false)),
        (&DATA_BODY, &[]),
        (&SET_BODY, &[]),
        (&STATUS_BODY, &status(false, false)),
    ];
    for (serial, (request, answer)) in (3u16..).zip(expected) {
        let ClockProgress::Send(step) = clock
            .call_set_clock_next(&mut store, conn, time, prev.as_deref())?
            .expect("clock step")
        else {
            panic!("step {serial} must send")
        };
        assert_eq!(body(&step.send, Some(serial)), request);
        assert_eq!(step.max_len, 64);
        prev = Some(reply_to(&step.send, 0, answer));
    }
    let done = clock
        .call_set_clock_next(&mut store, conn, time, prev.as_deref())?
        .expect("clock result");
    assert!(matches!(done, ClockProgress::Done(Ok(()))), "{done:?}");
    // The same resource still reads through the full world.
    assert!(guest
        .banto_driver_address()
        .call_parse(&mut store, "D0", "u16", None)?
        .is_ok());
    Ok(())
}

#[test]
fn small_world_sets_the_clock_over_3e_in_issue_order() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = connect(&mut store, &guest, &[ENABLED, ("frame", "3e")], R.0, R.1)?;
    let check = send_of(clock(&mut store, &guest, conn, TIME, None)?);
    assert_eq!(body(&check, None), STATUS_BODY);
    let data = send_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&check, 0, &status(false, false))),
    )?);
    assert_eq!(body(&data, None), DATA_BODY);
    let set = send_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&data, 0, &[])),
    )?);
    assert_eq!(body(&set, None), SET_BODY);
    let confirm = send_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&set, 0, &[])),
    )?);
    assert_eq!(body(&confirm, None), STATUS_BODY);
    assert!(done_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&confirm, 0, &status(false, false))),
    )?)
    .is_ok());
    Ok(())
}

#[test]
fn sm210_already_on_is_rejected_without_writing() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let check = send_of(clock(&mut store, &guest, conn, TIME, None)?);
    let result = done_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&check, 0, &status(true, false))),
    )?);
    assert!(
        matches!(&result, Err(ItemError::Rejected(message)) if message.contains("SM210 ON")),
        "{result:?}"
    );
    // A new run starts at step 1 with the next serial: nothing is pending.
    let again = send_of(clock(&mut store, &guest, conn, TIME, None)?);
    assert_eq!(body(&again, Some(4)), STATUS_BODY);
    Ok(())
}

#[test]
fn confirm_reads_report_sm211_retry_sm210_and_give_up_after_three() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    // SM211 ON after the set request.
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let confirm = to_confirm(&mut store, &guest, conn)?;
    let result = done_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&confirm, 0, &status(false, true))),
    )?);
    assert!(
        matches!(&result, Err(ItemError::Device(error)) if error.code == 211 && error.message.contains("SM211")),
        "{result:?}"
    );

    // SM210 ON twice, then OFF.
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let mut request = to_confirm(&mut store, &guest, conn)?;
    for _ in 0..2 {
        request = send_of(clock(
            &mut store,
            &guest,
            conn,
            TIME,
            Some(&reply_to(&request, 0, &status(true, false))),
        )?);
        assert_eq!(
            body(&request, Some(u16::from_le_bytes([request[2], request[3]]))),
            STATUS_BODY
        );
    }
    assert!(done_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&request, 0, &status(false, false))),
    )?)
    .is_ok());

    // SM210 ON three times.
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let mut request = to_confirm(&mut store, &guest, conn)?;
    for _ in 0..2 {
        request = send_of(clock(
            &mut store,
            &guest,
            conn,
            TIME,
            Some(&reply_to(&request, 0, &status(true, false))),
        )?);
    }
    let result = done_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&request, 0, &status(true, false))),
    )?);
    assert!(
        matches!(&result, Err(ItemError::VerificationFailed(message)) if message.contains("3 reads")),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn an_end_code_at_any_step_is_a_device_error_and_stops_sending() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for failing in 0..4 {
        let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
        let mut request = send_of(clock(&mut store, &guest, conn, TIME, None)?);
        for step in 0..failing {
            let answer: &[u8] = if step == 0 {
                &status(false, false)
            } else {
                &[]
            };
            request = send_of(clock(
                &mut store,
                &guest,
                conn,
                TIME,
                Some(&reply_to(&request, 0, answer)),
            )?);
        }
        let result = done_of(clock(
            &mut store,
            &guest,
            conn,
            TIME,
            Some(&reply_to(&request, 0xc059, &[])),
        )?);
        assert!(
            matches!(&result, Err(ItemError::Device(error)) if error.code == 0xc059),
            "step {}: {result:?}",
            failing + 1
        );
        // Nothing is pending: the next run starts over at step 1.
        let again = send_of(clock(&mut store, &guest, conn, TIME, None)?);
        assert_eq!(body(&again, Some(4 + failing)), STATUS_BODY);
    }
    Ok(())
}

#[test]
fn short_or_mismatched_responses_are_fatal() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let check = send_of(clock(&mut store, &guest, conn, TIME, None)?);
    let mut short = reply_to(&check, 0, &status(false, false));
    short.pop();
    assert!(matches!(
        clock(&mut store, &guest, conn, TIME, Some(&short))?,
        Err(FatalError::Framing(_))
    ));
    assert!(clock(&mut store, &guest, conn, TIME, None)?.is_err());

    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    let check = send_of(clock(&mut store, &guest, conn, TIME, None)?);
    let data = send_of(clock(
        &mut store,
        &guest,
        conn,
        TIME,
        Some(&reply_to(&check, 0, &status(false, false))),
    )?);
    let mut wrong_serial = reply_to(&data, 0, &[]);
    wrong_serial[2] ^= 0xff;
    assert!(matches!(
        clock(&mut store, &guest, conn, TIME, Some(&wrong_serial))?,
        Err(FatalError::Framing(_))
    ));
    assert!(clock(&mut store, &guest, conn, TIME, None)?.is_err());
    Ok(())
}

fn assert_refused_before_send(
    store: &mut Store<()>,
    guest: &Guest,
    conn: ResourceAny,
    time: CivilTime,
    unsupported: bool,
    why: &str,
) -> wasmtime::Result<()> {
    let result = done_of(clock(store, guest, conn, time, None)?);
    if unsupported {
        assert!(
            matches!(result, Err(ItemError::Unsupported(_))),
            "{why}: {result:?}"
        );
    } else {
        assert!(
            matches!(result, Err(ItemError::Rejected(_))),
            "{why}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn disabled_invalid_dates_models_and_routes_are_refused_before_send() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for extra in [&[][..], &[("clock_set", "disabled")][..]] {
        let conn = connect(&mut store, &guest, extra, R.0, R.1)?;
        assert_refused_before_send(&mut store, &guest, conn, TIME, false, "disabled")?;
    }
    let conn = connect(&mut store, &guest, &[ENABLED], R.0, R.1)?;
    for (time, why) in [
        (
            CivilTime {
                day: 30,
                month: 2,
                ..TIME
            },
            "Feb 30",
        ),
        (
            CivilTime {
                day: 31,
                month: 4,
                ..TIME
            },
            "Apr 31",
        ),
        (CivilTime { year: 2080, ..TIME }, "2080"),
        (CivilTime { year: 1979, ..TIME }, "1979"),
        (CivilTime { hour: 24, ..TIME }, "hour 24"),
    ] {
        assert_refused_before_send(&mut store, &guest, conn, time, false, why)?;
    }
    // The leap day is accepted and the run starts.
    let leap = CivilTime {
        year: 2024,
        month: 2,
        day: 29,
        ..TIME
    };
    send_of(clock(&mut store, &guest, conn, leap, None)?);

    for (extra, name, code, why) in [
        (&[ENABLED][..], "Q02UCPU", 0x0263, "standard Q"),
        (&[ENABLED][..], "Q25PRHCPU", 0x004c, "Q25PRH"),
        (
            &[ENABLED, ("frame", "3e")][..],
            "FX5U-32MR/ES",
            0x4a21,
            "FX5",
        ),
        (&[ENABLED][..], "FX5U-32MR/ES", 0x4a21, "FX5 over 4E"),
        (&[ENABLED][..], "Q02UCPU", 0x9999, "unknown model"),
        (
            &[ENABLED, ("network_no", "1"), ("station_no", "2")][..],
            R.0,
            R.1,
            "other station",
        ),
        (
            &[ENABLED, ("target_cpu", "cpu2")][..],
            R.0,
            R.1,
            "multi-CPU No.2",
        ),
    ] {
        let conn = connect(&mut store, &guest, extra, name, code)?;
        assert_refused_before_send(&mut store, &guest, conn, TIME, true, why)?;
    }
    Ok(())
}
