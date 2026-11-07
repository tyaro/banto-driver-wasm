//! H3 clock read fixtures through the built component: the two-step iQ-R
//! (BIN) and Q (BCD) procedure, SM213/SM210 refusals, invalid values, end
//! codes, malformed responses, exclusion with other operations and the
//! refusals made before anything is sent. Most tests bind the small
//! `driver-with-clock-read-v1` world the Hub uses; one binds the full world.

use wasmtime::component::{Component, Linker, ResourceAny};
use wasmtime::{Engine, Store};

mod clock_read_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-clock-read-v1" });
}
mod full_host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-diagnostics-clock-and-clock-read-v1" });
}

use clock_read_host::banto::driver::types::{FatalError, ItemError, Setting};
use clock_read_host::exports::banto::driver::clock_read_v1::{CivilTime, ClockReadProgress};
use clock_read_host::DriverWithClockReadV1 as Guest;

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

fn read(
    store: &mut Store<()>,
    guest: &Guest,
    conn: ResourceAny,
    prev: Option<&[u8]>,
) -> wasmtime::Result<Result<ClockReadProgress, FatalError>> {
    guest
        .banto_driver_clock_read_v1()
        .call_read_clock_next(&mut *store, conn, prev)
}

fn send_of(progress: Result<ClockReadProgress, FatalError>) -> Vec<u8> {
    match progress {
        Ok(ClockReadProgress::Send(step)) => {
            assert_eq!(step.max_len, 64);
            step.send
        }
        other => panic!("clock read step must send: {other:?}"),
    }
}

fn done_of(progress: Result<ClockReadProgress, FatalError>) -> Result<CivilTime, ItemError> {
    match progress {
        Ok(ClockReadProgress::Done(result)) => result,
        other => panic!("clock read must finish: {other:?}"),
    }
}

fn fields(time: &CivilTime) -> (u16, u8, u8, u8, u8, u8) {
    (
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
    )
}

/// SM210-SM213, two points per byte (high nibble first).
fn status(sm210: bool, sm211: bool, sm212: bool, sm213: bool) -> [u8; 2] {
    [
        (u8::from(sm210) << 4) | u8::from(sm211),
        (u8::from(sm212) << 4) | u8::from(sm213),
    ]
}

/// SM213 ON, the others OFF.
const READY: [u8; 2] = [0x00, 0x01];

fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// 2026-11-07 13:04:59, Saturday: BIN (iQ-R) and BCD (Q).
const R_WORDS: [u16; 7] = [2026, 11, 7, 13, 4, 59, 6];
const Q_WORDS: [u16; 4] = [0x2611, 0x0713, 0x0459, 0x2006];
const EXPECTED: (u16, u8, u8, u8, u8, u8) = (2026, 11, 7, 13, 4, 59);

/// Command onward, as in the SLMP manual (iQ-R and Q device specification).
const R_STATUS_BODY: [u8; 12] = [1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 4, 0];
const R_DATA_BODY: [u8; 12] = [1, 4, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0, 7, 0];
const Q_STATUS_BODY: [u8; 10] = [1, 4, 1, 0, 0xd2, 0, 0, 0x91, 4, 0];
const Q_DATA_BODY: [u8; 10] = [1, 4, 0, 0, 0xd2, 0, 0, 0xa9, 4, 0];

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

/// Run both steps with SM213 ON and return the outcome.
fn run(
    store: &mut Store<()>,
    guest: &Guest,
    conn: ResourceAny,
    data: &[u8],
) -> wasmtime::Result<Result<CivilTime, ItemError>> {
    let check = send_of(read(store, guest, conn, None)?);
    let request = send_of(read(
        store,
        guest,
        conn,
        Some(&reply_to(&check, 0, &READY)),
    )?);
    Ok(done_of(read(
        store,
        guest,
        conn,
        Some(&reply_to(&request, 0, data)),
    )?))
}

const R: (&str, u16) = ("R04CPU", 0x4800);
const Q: (&str, u16) = ("Q02UCPU", 0x0263);

#[test]
fn full_world_reads_the_r_clock_and_excludes_other_operations() -> wasmtime::Result<()> {
    use full_host::banto::driver::types::{FatalError, Setting, TagSpec};
    use full_host::exports::banto::driver::clock_read_v1::ClockReadProgress;
    use full_host::exports::banto::driver::clock_v1::CivilTime;
    let (engine, component) = component()?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    let guest = full_host::DriverWithWriteDiagnosticsClockAndClockReadV1::instantiate(
        &mut store, &component, &linker,
    )?;
    let settings = [
        ("host", "fixture.invalid"),
        ("port", "5000"),
        ("clock_set", "enabled"),
    ]
    .map(|(key, value)| Setting {
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
    let clock_read = guest.banto_driver_clock_read_v1();
    let ClockReadProgress::Send(check) = clock_read
        .call_read_clock_next(&mut store, conn, None)?
        .expect("status step")
    else {
        panic!("step 1 must send")
    };
    assert_eq!(body(&check.send, Some(3)), R_STATUS_BODY);

    // While the clock read waits, every other operation is refused.
    let d0 = TagSpec {
        address: "D0".into(),
        data_type: "u16".into(),
        string_words: None,
    };
    assert!(matches!(
        guest
            .banto_driver_read()
            .call_plan(&mut store, conn, std::slice::from_ref(&d0))?,
        Err(FatalError::Protocol(_))
    ));
    assert!(matches!(
        guest.banto_driver_diagnostics_v1().call_diagnose_next(
            &mut store,
            conn,
            std::slice::from_ref(&d0),
            None
        )?,
        Err(FatalError::Protocol(_))
    ));
    let time = CivilTime {
        year: 2026,
        month: 11,
        day: 7,
        hour: 13,
        minute: 4,
        second: 59,
    };
    assert!(matches!(
        guest
            .banto_driver_clock_v1()
            .call_set_clock_next(&mut store, conn, time, None)?,
        Err(FatalError::Protocol(_))
    ));
    assert!(session
        .call_handshake_next(&mut store, conn, None)?
        .is_err());

    let ClockReadProgress::Send(data) = clock_read
        .call_read_clock_next(&mut store, conn, Some(&reply_to(&check.send, 0, &READY)))?
        .expect("data step")
    else {
        panic!("step 2 must send")
    };
    assert_eq!(body(&data.send, Some(4)), R_DATA_BODY);
    assert_eq!(data.max_len, 64);
    let done = clock_read
        .call_read_clock_next(
            &mut store,
            conn,
            Some(&reply_to(&data.send, 0, &words(&R_WORDS))),
        )?
        .expect("clock read result");
    let ClockReadProgress::Done(Ok(read_time)) = done else {
        panic!("clock read must succeed: {done:?}")
    };
    assert_eq!(
        (
            read_time.year,
            read_time.month,
            read_time.day,
            read_time.hour,
            read_time.minute,
            read_time.second
        ),
        EXPECTED
    );
    // Afterwards the same resource plans reads again.
    assert!(guest
        .banto_driver_read()
        .call_plan(&mut store, conn, &[d0])?
        .is_ok());
    Ok(())
}

#[test]
fn small_world_reads_iq_r_bin_over_4e_and_3e() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    // R04CPU over 4E, and the G2b process CPU R08PCPU over 3E.
    let conn = connect(&mut store, &guest, &[], R.0, R.1)?;
    let check = send_of(read(&mut store, &guest, conn, None)?);
    assert_eq!(body(&check, Some(3)), R_STATUS_BODY);
    let data = send_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&check, 0, &status(false, true, true, true))),
    )?);
    assert_eq!(body(&data, Some(4)), R_DATA_BODY);
    let time = done_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&data, 0, &words(&R_WORDS))),
    )?)
    .expect("R clock");
    assert_eq!(fields(&time), EXPECTED);
    // A new run starts at step 1 with the next serial.
    let again = send_of(read(&mut store, &guest, conn, None)?);
    assert_eq!(body(&again, Some(5)), R_STATUS_BODY);

    let conn = connect(&mut store, &guest, &[("frame", "3e")], "R08PCPU", 0x4841)?;
    let check = send_of(read(&mut store, &guest, conn, None)?);
    assert_eq!(body(&check, None), R_STATUS_BODY);
    let data = send_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&check, 0, &READY)),
    )?);
    assert_eq!(body(&data, None), R_DATA_BODY);
    let time = done_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&data, 0, &words(&R_WORDS))),
    )?)
    .expect("R08P clock");
    assert_eq!(fields(&time), EXPECTED);
    Ok(())
}

#[test]
fn small_world_reads_standard_q_bcd_including_qnph_qnudpv_and_l() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for (extra, name, code) in [
        (&[][..], Q.0, Q.1),
        (&[("frame", "3e")][..], "Q12PHCPU", 0x0043),
        (&[][..], "Q04UDPVCPU", 0x0367),
        (&[("frame", "3e")][..], "L02CPU", 0x0541),
    ] {
        let conn = connect(&mut store, &guest, extra, name, code)?;
        let e4 = extra.is_empty();
        let check = send_of(read(&mut store, &guest, conn, None)?);
        let serial = e4.then(|| u16::from_le_bytes([check[2], check[3]]));
        assert_eq!(body(&check, serial), Q_STATUS_BODY, "{name}");
        let data = send_of(read(
            &mut store,
            &guest,
            conn,
            Some(&reply_to(&check, 0, &READY)),
        )?);
        assert_eq!(
            body(&data, serial.map(|serial| serial + 1)),
            Q_DATA_BODY,
            "{name}"
        );
        let time = done_of(read(
            &mut store,
            &guest,
            conn,
            Some(&reply_to(&data, 0, &words(&Q_WORDS))),
        )?)
        .expect("Q clock");
        assert_eq!(fields(&time), EXPECTED, "{name}");
    }
    // The QCPU manual's examples H9307, H3110, H3548, H1905.
    let conn = connect(&mut store, &guest, &[], Q.0, Q.1)?;
    let time = run(
        &mut store,
        &guest,
        conn,
        &words(&[0x9307, 0x3110, 0x3548, 0x1905]),
    )?
    .expect("manual example");
    assert_eq!(fields(&time), (1993, 7, 31, 10, 35, 48));
    Ok(())
}

#[test]
fn sm213_off_or_sm210_on_is_rejected_after_the_status_read() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for (model, name) in [(R, "SD210-SD216"), (Q, "SD210-SD213")] {
        for (bits, needle) in [
            (status(false, false, false, false), "SM213 is OFF"),
            (status(true, false, false, true), "SM210 ON"),
        ] {
            let conn = connect(&mut store, &guest, &[], model.0, model.1)?;
            let check = send_of(read(&mut store, &guest, conn, None)?);
            let result = done_of(read(
                &mut store,
                &guest,
                conn,
                Some(&reply_to(&check, 0, &bits)),
            )?);
            assert!(
                matches!(&result, Err(ItemError::Rejected(message)) if message.contains(needle) && message.contains(name)),
                "{}: {result:?}",
                model.0
            );
            // Nothing is pending: the next run starts at step 1 with the next serial.
            let again = send_of(read(&mut store, &guest, conn, None)?);
            assert_eq!(
                u16::from_le_bytes([again[2], again[3]]),
                u16::from_le_bytes([check[2], check[3]]) + 1
            );
        }
    }
    Ok(())
}

#[test]
fn values_that_are_not_a_clock_are_verification_failed() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let mut feb_30 = R_WORDS;
    feb_30[1] = 2;
    feb_30[2] = 30;
    let mut year_1979 = R_WORDS;
    year_1979[0] = 1979;
    let mut hour_24 = R_WORDS;
    hour_24[3] = 24;
    let mut month_256 = R_WORDS;
    month_256[1] = 256;
    for (data, needle) in [
        (feb_30, "day 30"),
        (year_1979, "year 1979"),
        (hour_24, "hour 24"),
        (month_256, "month 256"),
    ] {
        let conn = connect(&mut store, &guest, &[], R.0, R.1)?;
        let result = run(&mut store, &guest, conn, &words(&data))?;
        assert!(
            matches!(&result, Err(ItemError::VerificationFailed(message)) if message.contains("SD210-SD216") && message.contains(needle)),
            "{data:?}: {result:?}"
        );
    }
    for (data, needle) in [
        ([0x2a11, 0x0713, 0x0459, 0x2006], "SD210 is not BCD"),
        ([0x2611, 0x0713, 0x0459, 0xb006], "SD213 is not BCD"),
        ([0x8011, 0x0713, 0x0459, 0x2006], "year 2080"),
        ([0x2613, 0x0713, 0x0459, 0x2006], "month 13"),
    ] {
        let conn = connect(&mut store, &guest, &[], Q.0, Q.1)?;
        let result = run(&mut store, &guest, conn, &words(&data))?;
        assert!(
            matches!(&result, Err(ItemError::VerificationFailed(message)) if message.contains(needle)),
            "{data:04x?}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn an_end_code_at_either_step_is_a_device_error_and_stops_sending() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for model in [R, Q] {
        for failing in 0..2 {
            let conn = connect(&mut store, &guest, &[], model.0, model.1)?;
            let mut request = send_of(read(&mut store, &guest, conn, None)?);
            let first = u16::from_le_bytes([request[2], request[3]]);
            if failing == 1 {
                request = send_of(read(
                    &mut store,
                    &guest,
                    conn,
                    Some(&reply_to(&request, 0, &READY)),
                )?);
            }
            let result = done_of(read(
                &mut store,
                &guest,
                conn,
                Some(&reply_to(&request, 0xc059, &[])),
            )?);
            assert!(
                matches!(&result, Err(ItemError::Device(error)) if error.code == 0xc059),
                "{} step {}: {result:?}",
                model.0,
                failing + 1
            );
            let again = send_of(read(&mut store, &guest, conn, None)?);
            assert_eq!(
                u16::from_le_bytes([again[2], again[3]]),
                first + 1 + failing
            );
        }
    }
    Ok(())
}

#[test]
fn short_or_mismatched_responses_are_fatal() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    let conn = connect(&mut store, &guest, &[], R.0, R.1)?;
    let check = send_of(read(&mut store, &guest, conn, None)?);
    let mut short = reply_to(&check, 0, &READY);
    short.pop();
    assert!(matches!(
        read(&mut store, &guest, conn, Some(&short))?,
        Err(FatalError::Framing(_))
    ));
    assert!(read(&mut store, &guest, conn, None)?.is_err());

    let conn = connect(&mut store, &guest, &[], Q.0, Q.1)?;
    let check = send_of(read(&mut store, &guest, conn, None)?);
    let data = send_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&check, 0, &READY)),
    )?);
    let mut wrong_serial = reply_to(&data, 0, &words(&Q_WORDS));
    wrong_serial[2] ^= 0xff;
    assert!(matches!(
        read(&mut store, &guest, conn, Some(&wrong_serial))?,
        Err(FatalError::Framing(_))
    ));
    assert!(read(&mut store, &guest, conn, None)?.is_err());

    // Seven R words for a Q read.
    let conn = connect(&mut store, &guest, &[], Q.0, Q.1)?;
    let check = send_of(read(&mut store, &guest, conn, None)?);
    let data = send_of(read(
        &mut store,
        &guest,
        conn,
        Some(&reply_to(&check, 0, &READY)),
    )?);
    assert!(matches!(
        read(
            &mut store,
            &guest,
            conn,
            Some(&reply_to(&data, 0, &words(&R_WORDS)))
        )?,
        Err(FatalError::Framing(_))
    ));
    Ok(())
}

#[test]
fn models_routes_and_an_unfinished_handshake_are_refused_before_send() -> wasmtime::Result<()> {
    let (mut store, guest) = load()?;
    for (extra, name, code, why) in [
        (&[][..], "Q25PRHCPU", 0x004c, "Q25PRH"),
        (&[("frame", "3e")][..], "FX5U-32MR/ES", 0x4a21, "FX5"),
        (&[][..], "FX5U-32MR/ES", 0x4a21, "FX5 over 4E"),
        (&[][..], "Q02UCPU", 0x9999, "unknown model"),
        (
            &[("network_no", "1"), ("station_no", "2")][..],
            R.0,
            R.1,
            "other station",
        ),
        (&[("target_cpu", "cpu2")][..], R.0, R.1, "multi-CPU No.2"),
        (&[("target_cpu", "cpu1")][..], Q.0, Q.1, "Q multi-CPU No.1"),
    ] {
        let conn = connect(&mut store, &guest, extra, name, code)?;
        let result = done_of(read(&mut store, &guest, conn, None)?);
        assert!(
            matches!(result, Err(ItemError::Unsupported(_))),
            "{why}: {result:?}"
        );
    }
    let session = guest.banto_driver_session().connection();
    let conn = session
        .call_create(&mut store, &settings(&[]))?
        .expect("create");
    let result = done_of(read(&mut store, &guest, conn, None)?);
    assert!(
        matches!(&result, Err(ItemError::Rejected(message)) if message.contains("discovery")),
        "{result:?}"
    );
    // The handshake still starts with 0101: nothing was sent before it.
    let step = session
        .call_handshake_next(&mut store, conn, None)?
        .expect("0101")
        .expect("send");
    assert_eq!(body(&step.send, Some(1))[..2], [1, 1]);
    Ok(())
}
