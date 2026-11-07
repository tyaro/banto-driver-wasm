//! Optional clock-read-v1 (H3): read the clock of a 0101-confirmed iQ-R or
//! standard Q CPU over the same connection as read and write. Nothing is
//! written to the PLC, so there is no guest setting; the Host decides
//! whether to offer it, as with diagnostics.
//!
//! SD210 onward holds the clock only after SM213 (clock data read request)
//! asks for it, so the PLC program has to keep SM213 ON. Before anything is
//! sent the guest checks, in order: the model (standard R family, which
//! includes the G2b CPUs, or standard Q, which includes QnPH, QnUDPV and L)
//! and the destination (own station only). The run is then two steps, each
//! one `send` matched like any read (4E serial, 3E issue order, route echo):
//!
//! 1. 0401 bit read of SM210-SM213. SM213 OFF means SD210 onward is not the
//!    current clock (an earlier read, or a value written by a clock set);
//!    SM210 ON means a clock set is waiting for the CPU with its value in
//!    SD210 onward. Both end with `rejected`.
//! 2. One 0401 word read of SD210-SD216 (iQ-R, BIN) or SD210-SD213 (Q, BCD).
//!    The values must be a real date and time within 1980-2079 (and BCD
//!    digits on Q), otherwise `verification-failed`. The day of week is not
//!    checked.
//!
//! A PLC end code at either step ends the run with `device` and nothing more
//! is sent. A malformed response is fatal and leaves this connection
//! unusable until the Host reconnects. The Host serializes calls and closes
//! the connection after an abandoned send.
use super::*;
use banto_slmp_read_core::{
    clock_data_span, decode_q_clock_data, decode_r_clock_data, encode_clock_data_read,
    encode_clock_read_status, ClockData, NotBcd, CLOCK_READ_STATUS_SPAN,
};
use clock::Civil;
use exports::banto::driver::clock_read_v1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadStage {
    /// Step 1: SM210-SM213.
    Status,
    /// Step 2: SD210 onward.
    Data,
}

pub(super) struct PendingClockRead {
    route: Route,
    series: Series,
    stage: ReadStage,
}

impl SlmpConnection {
    /// A clock read response is awaited, or one was malformed.
    pub(super) fn clock_read_busy(&self) -> bool {
        self.pending_clock_read.borrow().is_some() || self.clock_read_poisoned.get()
    }
}

/// SD210-SD216 (R) or SD210-SD213 (Q), for messages.
const fn data_name(series: Series) -> &'static str {
    match series {
        Series::R => "SD210-SD216",
        Series::Q => "SD210-SD213",
    }
}

fn finish(
    conn: &SlmpConnection,
    result: Result<clock_read_v1::CivilTime, ItemError>,
) -> clock_read_v1::ClockReadProgress {
    conn.clock_read_poisoned.set(false);
    clock_read_v1::ClockReadProgress::Done(result)
}

fn end_code_error(code: u16) -> ItemError {
    ItemError::Device(banto::driver::types::DeviceError {
        code: u32::from(code),
        message: format!("SLMP end code 0x{code:04X}"),
    })
}

/// Store the next step and return it.
fn send(
    conn: &SlmpConnection,
    series: Series,
    stage: ReadStage,
) -> clock_read_v1::ClockReadProgress {
    let route = conn.route();
    let step = match stage {
        ReadStage::Status => read_step(
            route.frame,
            encode_clock_read_status(route, series),
            CLOCK_READ_STATUS_SPAN,
        ),
        ReadStage::Data => read_step(
            route.frame,
            encode_clock_data_read(route, series),
            clock_data_span(series),
        ),
    };
    *conn.pending_clock_read.borrow_mut() = Some(PendingClockRead {
        route,
        series,
        stage,
    });
    clock_read_v1::ClockReadProgress::Send(step)
}

/// One clock field as `u8`; a larger value names the field.
fn narrow(name: &str, value: u16) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{name} {value} is out of range"))
}

/// Range-check read clock data with the same rules as a clock set.
fn civil(data: ClockData) -> Result<clock_read_v1::CivilTime, String> {
    let time = Civil {
        year: data.year,
        month: narrow("month", data.month)?,
        day: narrow("day", data.day)?,
        hour: narrow("hour", data.hour)?,
        minute: narrow("minute", data.minute)?,
        second: narrow("second", data.second)?,
    };
    time.validate()?;
    Ok(clock_read_v1::CivilTime {
        year: time.year,
        month: time.month,
        day: time.day,
        hour: time.hour,
        minute: time.minute,
        second: time.second,
    })
}

fn respond(
    conn: &SlmpConnection,
    pending: PendingClockRead,
    response: &[u8],
) -> Result<clock_read_v1::ClockReadProgress, FatalError> {
    let PendingClockRead {
        route,
        series,
        stage,
    } = pending;
    match stage {
        ReadStage::Status => match decode_read(route, CLOCK_READ_STATUS_SPAN, response) {
            Ok(Values::Bits(bits)) if bits.len() == 4 => {
                let (sm210, sm213) = (bits[0], bits[3]);
                if !sm213 {
                    return Ok(finish(
                        conn,
                        Err(ItemError::Rejected(format!(
                            "SM213 is OFF, so {} do not hold the current clock; the PLC program must keep SM213 ON",
                            data_name(series)
                        ))),
                    ));
                }
                if sm210 {
                    return Ok(finish(
                        conn,
                        Err(ItemError::Rejected(format!(
                            "a clock set request is pending (SM210 ON), so {} may hold the value being set",
                            data_name(series)
                        ))),
                    ));
                }
                Ok(send(conn, series, ReadStage::Data))
            }
            Ok(_) => Err(FatalError::Protocol(
                "clock status read returned an unexpected unit".into(),
            )),
            Err(ResponseError::EndCode(code)) => Ok(finish(conn, Err(end_code_error(code)))),
            Err(error) => Err(response_error(error)),
        },
        ReadStage::Data => match decode_read(route, clock_data_span(series), response) {
            Ok(Values::Words(words)) => {
                let data = match series {
                    Series::R => <[u16; 7]>::try_from(words.as_slice())
                        .map(decode_r_clock_data)
                        .map_err(|_| ()),
                    Series::Q => match <[u16; 4]>::try_from(words.as_slice()) {
                        Ok(words) => match decode_q_clock_data(words) {
                            Ok(data) => Ok(data),
                            Err(NotBcd(sd)) => {
                                return Ok(finish(
                                    conn,
                                    Err(ItemError::VerificationFailed(format!(
                                        "SD{sd} is not BCD, so SD210-SD213 do not hold a clock"
                                    ))),
                                ))
                            }
                        },
                        Err(_) => Err(()),
                    },
                };
                let Ok(data) = data else {
                    return Err(FatalError::Protocol(
                        "clock data read returned an unexpected length".into(),
                    ));
                };
                Ok(finish(
                    conn,
                    civil(data).map_err(|why| {
                        ItemError::VerificationFailed(format!(
                            "{} do not hold a valid clock: {why}",
                            data_name(series)
                        ))
                    }),
                ))
            }
            Ok(_) => Err(FatalError::Protocol(
                "clock data read returned an unexpected unit".into(),
            )),
            Err(ResponseError::EndCode(code)) => Ok(finish(conn, Err(end_code_error(code)))),
            Err(error) => Err(response_error(error)),
        },
    }
}

/// Checks made before anything is sent: the wire series, or the item error.
fn preflight(conn: &SlmpConnection) -> Result<Series, ItemError> {
    let series = match &*conn.phase.borrow() {
        Phase::Ready { model, .. } => match model.family {
            // Includes the G2b process, SIL2, safety and MX-R CPUs.
            ModelFamily::StandardR => Series::R,
            // Includes the G2a QnPH, QnUDPV and L CPUs.
            ModelFamily::StandardQ => Series::Q,
            ModelFamily::Q25PrhWireOnly => {
                return Err(ItemError::Unsupported(
                    "clock read is not supported on the redundant Q25PRH/Q12PRH CPU".into(),
                ))
            }
            ModelFamily::Fx5 => {
                return Err(ItemError::Unsupported(
                    "clock read is not supported on FX5".into(),
                ))
            }
            ModelFamily::Unknown => {
                return Err(ItemError::Unsupported(
                    "clock read requires a 0101-confirmed iQ-R or Q CPU".into(),
                ))
            }
        },
        Phase::NoRange(Some(model)) if model.family == ModelFamily::Fx5 => {
            return Err(ItemError::Unsupported(
                "clock read is not supported on FX5".into(),
            ))
        }
        Phase::NoRange(_) => {
            return Err(ItemError::Unsupported(
                "clock read requires a 0101-confirmed iQ-R or Q CPU".into(),
            ))
        }
        _ => return Err(ItemError::Rejected("discovery is incomplete".into())),
    };
    let own = conn.base_route.network == 0
        && conn.base_route.pc == 0xff
        && conn.base_route.io == 0x03ff
        && conn.base_route.station == 0;
    if !own {
        return Err(ItemError::Unsupported(
            "clock read is supported on the connected station's own CPU only".into(),
        ));
    }
    Ok(series)
}

pub(super) fn read_clock_next_on(
    conn: &SlmpConnection,
    prev_response: Option<Vec<u8>>,
) -> Result<clock_read_v1::ClockReadProgress, FatalError> {
    if let Some(response) = prev_response {
        // Consume before parsing. The poison stays set across the steps of a
        // run; only a final outcome clears it.
        conn.clock_read_poisoned.set(true);
        let pending = conn.pending_clock_read.borrow_mut().take().ok_or_else(|| {
            FatalError::Protocol("clock read response without a pending request".into())
        })?;
        return respond(conn, pending, &response);
    }
    if conn.clock_read_poisoned.get() {
        return Err(FatalError::Protocol(
            "clock read response was invalid; reconnect".into(),
        ));
    }
    if conn.pending_clock_read.borrow().is_some() {
        conn.clock_read_poisoned.set(true);
        return Err(FatalError::Protocol(
            "clock read response is missing; reconnect".into(),
        ));
    }
    if conn.pending_write.borrow().is_some()
        || conn.write_poisoned.get()
        || !conn.pending.borrow().is_empty()
        || conn.pending_diagnostic.borrow().is_some()
        || conn.diagnostic_poisoned.get()
        || conn.pending_clock.borrow().is_some()
        || conn.clock_poisoned.get()
    {
        return Err(FatalError::Protocol(
            "another response is pending on this connection".into(),
        ));
    }
    match preflight(conn) {
        Ok(series) => Ok(send(conn, series, ReadStage::Status)),
        Err(error) => Ok(clock_read_v1::ClockReadProgress::Done(Err(error))),
    }
}

impl clock_read_v1::Guest for Component {
    fn read_clock_next(
        conn: clock_read_v1::ConnectionBorrow<'_>,
        prev_response: Option<Vec<u8>>,
    ) -> Result<clock_read_v1::ClockReadProgress, FatalError> {
        read_clock_next_on(conn.get::<SlmpConnection>(), prev_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::CLOCK_SET_KEY;

    fn settings(extra: &[(&str, &str)]) -> Vec<session::Setting> {
        let mut out = vec![("host", "fixture.invalid"), ("port", "5000")];
        out.extend_from_slice(extra);
        out.into_iter()
            .map(|(key, value)| session::Setting {
                key: key.into(),
                value: value.into(),
            })
            .collect()
    }

    fn model(family: ModelFamily) -> TypeName {
        TypeName {
            raw_name: [b' '; 16],
            name: String::new(),
            code: 0,
            family,
        }
    }

    /// A connection that finished discovery as `family`; the next serial is 3.
    fn ready_conn(extra: &[(&str, &str)], family: ModelFamily) -> SlmpConnection {
        let conn = SlmpConnection::new(&settings(extra)).expect("valid settings");
        *conn.phase.borrow_mut() = Phase::Ready {
            model: model(family),
            count: PointCount::Unknown,
        };
        conn.next_serial.set(3);
        conn
    }

    /// Response to `conn`'s route with `serial` (ignored by 3E).
    fn reply(conn: &SlmpConnection, serial: u16, end_code: u16, body: &[u8]) -> Vec<u8> {
        let route = conn.base_route;
        let mut frame = match route.frame {
            Frame::E4 => {
                let mut head = vec![0xd4, 0];
                head.extend_from_slice(&serial.to_le_bytes());
                head.extend_from_slice(&[0, 0]);
                head
            }
            Frame::E3 => vec![0xd0, 0],
        };
        frame.extend_from_slice(&[route.network, route.pc]);
        frame.extend_from_slice(&route.io.to_le_bytes());
        frame.push(route.station);
        frame.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_le_bytes());
        frame.extend_from_slice(&end_code.to_le_bytes());
        frame.extend_from_slice(body);
        frame
    }

    /// SM210-SM213, two points per byte (high nibble first).
    fn bits(sm210: bool, sm211: bool, sm212: bool, sm213: bool) -> [u8; 2] {
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

    /// 2026-11-07 13:04:59, Saturday.
    const R_WORDS: [u16; 7] = [2026, 11, 7, 13, 4, 59, 6];
    const Q_WORDS: [u16; 4] = [0x2611, 0x0713, 0x0459, 0x2006];
    type Fields = (u16, u8, u8, u8, u8, u8);
    const EXPECTED: Fields = (2026, 11, 7, 13, 4, 59);

    /// The returned time as a tuple; any item error fails the test.
    fn fields(result: Result<clock_read_v1::CivilTime, ItemError>) -> Fields {
        let time = result.unwrap_or_else(|error| panic!("clock read must succeed: {error:?}"));
        (
            time.year,
            time.month,
            time.day,
            time.hour,
            time.minute,
            time.second,
        )
    }

    fn sent(progress: Result<clock_read_v1::ClockReadProgress, FatalError>) -> session::Step {
        let Ok(clock_read_v1::ClockReadProgress::Send(step)) = progress else {
            panic!("clock read step must send: {progress:?}")
        };
        step
    }

    fn done(
        progress: Result<clock_read_v1::ClockReadProgress, FatalError>,
    ) -> Result<clock_read_v1::CivilTime, ItemError> {
        let Ok(clock_read_v1::ClockReadProgress::Done(result)) = progress else {
            panic!("clock read must finish: {progress:?}")
        };
        result
    }

    /// 4E or 3E own-station request with the default timer.
    fn request(frame: Frame, serial: u16, body: &[u8]) -> Vec<u8> {
        let mut out = match frame {
            Frame::E4 => {
                let mut head = vec![0x54, 0];
                head.extend_from_slice(&serial.to_le_bytes());
                head.extend_from_slice(&[0, 0]);
                head
            }
            Frame::E3 => vec![0x50, 0],
        };
        out.extend_from_slice(&[0, 0xff, 0xff, 3, 0]);
        out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_le_bytes());
        out.extend_from_slice(&[0x10, 0]);
        out.extend_from_slice(body);
        out
    }

    const R_STATUS_BODY: [u8; 12] = [1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 4, 0];
    const R_DATA_BODY: [u8; 12] = [1, 4, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0, 7, 0];
    const Q_STATUS_BODY: [u8; 10] = [1, 4, 1, 0, 0xd2, 0, 0, 0x91, 4, 0];
    const Q_DATA_BODY: [u8; 10] = [1, 4, 0, 0, 0xd2, 0, 0, 0xa9, 4, 0];

    fn run(conn: &SlmpConnection, data: &[u8]) -> Result<clock_read_v1::CivilTime, ItemError> {
        let serial = conn.next_serial.get();
        sent(read_clock_next_on(conn, None));
        sent(read_clock_next_on(
            conn,
            Some(reply(conn, serial, 0, &READY)),
        ));
        done(read_clock_next_on(
            conn,
            Some(reply(conn, serial + 1, 0, data)),
        ))
    }

    #[test]
    fn r_4e_clock_read_sends_two_exact_steps_and_returns_the_bin_clock() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        let step = sent(read_clock_next_on(&conn, None));
        assert_eq!(step.send, request(Frame::E4, 3, &R_STATUS_BODY));
        assert_eq!(step.max_len, 64);
        // SM211 and SM212 are not looked at.
        let step = sent(read_clock_next_on(
            &conn,
            Some(reply(&conn, 3, 0, &bits(false, true, true, true))),
        ));
        assert_eq!(step.send, request(Frame::E4, 4, &R_DATA_BODY));
        assert_eq!(step.max_len, 64);
        assert_eq!(
            fields(done(read_clock_next_on(
                &conn,
                Some(reply(&conn, 4, 0, &words(&R_WORDS)))
            ))),
            EXPECTED
        );
        assert!(!conn.clock_busy());
        // A new run starts with prev-response none; no clock_set is needed.
        assert!(!conn.clock_set_enabled);
        let step = sent(read_clock_next_on(&conn, None));
        assert_eq!(step.send, request(Frame::E4, 5, &R_STATUS_BODY));
    }

    #[test]
    fn q_clock_read_uses_the_q_wire_and_bcd_on_4e_and_3e() {
        let conn = ready_conn(&[], ModelFamily::StandardQ);
        let step = sent(read_clock_next_on(&conn, None));
        assert_eq!(step.send, request(Frame::E4, 3, &Q_STATUS_BODY));
        let step = sent(read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &READY))));
        assert_eq!(step.send, request(Frame::E4, 4, &Q_DATA_BODY));
        assert_eq!(
            fields(done(read_clock_next_on(
                &conn,
                Some(reply(&conn, 4, 0, &words(&Q_WORDS)))
            ))),
            EXPECTED
        );

        let conn = ready_conn(&[("frame", "3e")], ModelFamily::StandardQ);
        let step = sent(read_clock_next_on(&conn, None));
        assert_eq!(step.send, request(Frame::E3, 0, &Q_STATUS_BODY));
        let step = sent(read_clock_next_on(&conn, Some(reply(&conn, 0, 0, &READY))));
        assert_eq!(step.send, request(Frame::E3, 0, &Q_DATA_BODY));
        // The QCPU manual's examples H9307, H3110, H3548, H1905.
        assert_eq!(
            fields(done(read_clock_next_on(
                &conn,
                Some(reply(
                    &conn,
                    0,
                    0,
                    &words(&[0x9307, 0x3110, 0x3548, 0x1905])
                ))
            ))),
            (1993, 7, 31, 10, 35, 48)
        );
    }

    #[test]
    fn sm213_off_or_sm210_on_is_rejected_after_one_read() {
        for (status, needle) in [
            (bits(false, false, false, false), "SM213 is OFF"),
            (bits(true, false, false, false), "SM213 is OFF"),
            (bits(true, false, false, true), "SM210 ON"),
            (bits(true, true, true, true), "SM210 ON"),
        ] {
            for family in [ModelFamily::StandardR, ModelFamily::StandardQ] {
                let conn = ready_conn(&[], family);
                sent(read_clock_next_on(&conn, None));
                let Err(ItemError::Rejected(message)) =
                    done(read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &status))))
                else {
                    panic!("{status:?} refuses the read")
                };
                assert!(message.contains(needle), "{message}");
                let name = if family == ModelFamily::StandardR {
                    "SD210-SD216"
                } else {
                    "SD210-SD213"
                };
                assert!(message.contains(name), "{message}");
                assert_eq!(conn.next_serial.get(), 4, "only the status read was sent");
                assert!(!conn.clock_busy());
            }
        }
    }

    #[test]
    fn values_outside_the_clock_are_verification_failed() {
        let mut bad_r = Vec::new();
        for (index, value, field) in [
            (0, 1979, "year"),
            (0, 2080, "year"),
            (1, 0, "month"),
            (1, 13, "month"),
            (1, 300, "month"),
            (2, 0, "day"),
            (2, 31, "day"), // November has 30 days.
            (3, 24, "hour"),
            (4, 60, "minute"),
            (5, 60, "second"),
            (5, 0x8000, "second"),
        ] {
            let mut data = R_WORDS;
            data[index] = value;
            bad_r.push((data, field));
        }
        let mut feb = R_WORDS;
        feb[1] = 2;
        feb[2] = 29;
        bad_r.push((feb, "day")); // 2026 is not a leap year.
        for (data, field) in bad_r {
            let conn = ready_conn(&[], ModelFamily::StandardR);
            let Err(ItemError::VerificationFailed(message)) = run(&conn, &words(&data)) else {
                panic!("{data:?} is not a clock")
            };
            assert!(
                message.starts_with("SD210-SD216 do not hold a valid clock: ")
                    && message.contains(field),
                "{data:?}: {message}"
            );
            assert!(!conn.clock_busy());
        }
        // The day of week is read but not checked; a leap day is a date.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        let mut leap = R_WORDS;
        leap[0] = 2024;
        leap[1] = 2;
        leap[2] = 29;
        leap[6] = 0xffff;
        assert_eq!(fields(run(&conn, &words(&leap))), (2024, 2, 29, 13, 4, 59));

        for (data, needle) in [
            ([0x2a11, 0x0713, 0x0459, 0x2006], "SD210 is not BCD"),
            ([0x2611, 0x07f3, 0x0459, 0x2006], "SD211 is not BCD"),
            ([0x2611, 0x0713, 0x045a, 0x2006], "SD212 is not BCD"),
            ([0x2611, 0x0713, 0x0459, 0xb006], "SD213 is not BCD"),
            ([0x2613, 0x0713, 0x0459, 0x2006], "month 13"),
            ([0x7911, 0x0713, 0x0459, 0x1906], "year 1979"),
            ([0x8011, 0x0713, 0x0459, 0x2006], "year 2080"),
            ([0x2602, 0x2913, 0x0459, 0x2006], "day 29"),
            ([0x2611, 0x0724, 0x0459, 0x2006], "hour 24"),
            ([0x2611, 0x0713, 0x6059, 0x2006], "minute 60"),
        ] {
            let conn = ready_conn(&[], ModelFamily::StandardQ);
            let Err(ItemError::VerificationFailed(message)) = run(&conn, &words(&data)) else {
                panic!("{data:04x?} is not a clock")
            };
            assert!(message.contains(needle), "{data:04x?}: {message}");
        }
        // The day-of-week byte is not decoded.
        let conn = ready_conn(&[], ModelFamily::StandardQ);
        assert_eq!(
            fields(run(&conn, &words(&[0x2611, 0x0713, 0x0459, 0x20ff]))),
            EXPECTED
        );
    }

    #[test]
    fn end_codes_end_the_run_at_both_steps() {
        for family in [ModelFamily::StandardR, ModelFamily::StandardQ] {
            let conn = ready_conn(&[], family);
            sent(read_clock_next_on(&conn, None));
            let Err(ItemError::Device(error)) = done(read_clock_next_on(
                &conn,
                Some(reply(&conn, 3, 0xc059, &[])),
            )) else {
                panic!("step 1: end code is a device error")
            };
            assert_eq!(error.code, 0xc059);
            assert!(error.message.contains("0xC059"));
            assert_eq!(conn.next_serial.get(), 4, "nothing more was sent");
            assert!(!conn.clock_busy());

            let conn = ready_conn(&[], family);
            sent(read_clock_next_on(&conn, None));
            sent(read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &READY))));
            let Err(ItemError::Device(error)) = done(read_clock_next_on(
                &conn,
                Some(reply(&conn, 4, 0x4031, &[])),
            )) else {
                panic!("step 2: end code is a device error")
            };
            assert_eq!(error.code, 0x4031);
            assert_eq!(conn.next_serial.get(), 5, "nothing more was sent");
            assert!(!conn.clock_busy());
        }
    }

    #[test]
    fn malformed_or_mismatched_responses_are_fatal_and_poison_the_clock_read() {
        // A wrong 4E serial.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        assert!(matches!(
            read_clock_next_on(&conn, Some(reply(&conn, 9, 0, &READY))),
            Err(FatalError::Framing(_))
        ));
        assert!(conn.clock_busy());
        assert!(matches!(
            read_clock_next_on(&conn, None),
            Err(FatalError::Protocol(_))
        ));
        assert!(
            plan_on(&conn, Vec::new()).is_err(),
            "reads wait for reconnect"
        );

        // A short status response, and an invalid bit nibble.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        let mut short = reply(&conn, 3, 0, &READY);
        short.pop();
        assert!(matches!(
            read_clock_next_on(&conn, Some(short)),
            Err(FatalError::Framing(_))
        ));
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        assert!(matches!(
            read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &[0x00, 0x21]))),
            Err(FatalError::Framing(_))
        ));

        // Q data with the R length, and R data with the Q length.
        for (family, data) in [
            (ModelFamily::StandardQ, words(&R_WORDS)),
            (ModelFamily::StandardR, words(&Q_WORDS)),
        ] {
            let conn = ready_conn(&[], family);
            sent(read_clock_next_on(&conn, None));
            sent(read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &READY))));
            assert!(matches!(
                read_clock_next_on(&conn, Some(reply(&conn, 4, 0, &data))),
                Err(FatalError::Framing(_))
            ));
            assert!(conn.clock_busy());
        }

        // A route that does not echo the request.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        let mut other_route = reply(&conn, 3, 0, &READY);
        other_route[7] = 0x01;
        assert!(matches!(
            read_clock_next_on(&conn, Some(other_route)),
            Err(FatalError::Framing(_))
        ));

        // A send abandoned without its response.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        assert!(matches!(
            read_clock_next_on(&conn, None),
            Err(FatalError::Protocol(_))
        ));
        assert!(conn.clock_busy());

        // A response without a run.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        assert!(matches!(
            read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &READY))),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn other_operations_wait_for_a_pending_clock_read() {
        let conn = ready_conn(&[(CLOCK_SET_KEY, "enabled")], ModelFamily::StandardR);
        sent(read_clock_next_on(&conn, None));
        assert!(plan_on(&conn, Vec::new()).is_err());
        assert!(write_next_on(&conn, Vec::new(), None).is_err());
        assert!(<SlmpConnection as session::GuestConnection>::handshake_next(&conn, None).is_err());
        let time = Civil {
            year: 2026,
            month: 11,
            day: 7,
            hour: 13,
            minute: 4,
            second: 59,
        };
        assert!(matches!(
            clock::set_clock_next_on(&conn, time, None),
            Err(FatalError::Protocol(_))
        ));
        // The read itself goes on and frees the connection afterwards.
        sent(read_clock_next_on(&conn, Some(reply(&conn, 3, 0, &READY))));
        assert!(done(read_clock_next_on(
            &conn,
            Some(reply(&conn, 4, 0, &words(&R_WORDS)))
        ))
        .is_ok());
        assert!(!conn.clock_busy());
        assert!(matches!(
            clock::set_clock_next_on(&conn, time, None),
            Ok(exports::banto::driver::clock_v1::ClockProgress::Send(_))
        ));
    }

    #[test]
    fn a_clock_read_waits_for_other_pending_or_uncertain_operations() {
        let refused = |conn: &SlmpConnection, why: &str| {
            let serial = conn.next_serial.get();
            assert!(
                matches!(read_clock_next_on(conn, None), Err(FatalError::Protocol(_))),
                "{why}"
            );
            assert_eq!(conn.next_serial.get(), serial, "{why}: nothing sent");
        };
        let conn = ready_conn(&[], ModelFamily::StandardR);
        conn.write_poisoned.set(true);
        refused(&conn, "uncertain write");
        let conn = ready_conn(&[], ModelFamily::StandardR);
        conn.diagnostic_poisoned.set(true);
        refused(&conn, "uncertain diagnostic");
        let conn = ready_conn(&[], ModelFamily::StandardR);
        conn.clock_poisoned.set(true);
        refused(&conn, "uncertain clock set");
        // A pending tag read.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        let plans = plan_on(
            &conn,
            vec![TagSpec {
                address: "D0".into(),
                data_type: "u16".into(),
                string_words: None,
            }],
        )
        .expect("plan D0");
        assert_eq!(plans.len(), 1);
        refused(&conn, "pending read");
        // A pending clock set step.
        let conn = ready_conn(&[(CLOCK_SET_KEY, "enabled")], ModelFamily::StandardR);
        let time = Civil {
            year: 2026,
            month: 11,
            day: 7,
            hour: 13,
            minute: 4,
            second: 59,
        };
        let serial = conn.next_serial.get();
        let Ok(exports::banto::driver::clock_v1::ClockProgress::Send(_)) =
            clock::set_clock_next_on(&conn, time, None)
        else {
            panic!("clock set starts")
        };
        assert_eq!(conn.next_serial.get(), serial + 1);
        refused(&conn, "pending clock set");
    }

    fn assert_refused(conn: &SlmpConnection, unsupported: bool, why: &str) {
        let serial = conn.next_serial.get();
        let result = done(read_clock_next_on(conn, None));
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
        assert_eq!(conn.next_serial.get(), serial, "{why}: nothing sent");
        assert!(!conn.clock_busy(), "{why}");
    }

    #[test]
    fn refusals_before_send_follow_model_and_route() {
        for family in [
            ModelFamily::Q25PrhWireOnly,
            ModelFamily::Fx5,
            ModelFamily::Unknown,
        ] {
            assert_refused(&ready_conn(&[], family), true, &format!("{family:?}"));
        }
        let no_range = ready_conn(&[], ModelFamily::StandardR);
        *no_range.phase.borrow_mut() = Phase::NoRange(None);
        assert_refused(&no_range, true, "0101 refused");
        *no_range.phase.borrow_mut() = Phase::NoRange(Some(model(ModelFamily::Fx5)));
        assert_refused(&no_range, true, "FX5 over 4E");
        *no_range.phase.borrow_mut() = Phase::NoRange(Some(model(ModelFamily::Unknown)));
        assert_refused(&no_range, true, "unknown model");
        *no_range.phase.borrow_mut() = Phase::Start;
        assert_refused(&no_range, false, "handshake incomplete");
        for family in [ModelFamily::StandardR, ModelFamily::StandardQ] {
            for route in [
                &[("network_no", "1"), ("station_no", "2")][..],
                &[("target_cpu", "cpu1")][..],
                &[("target_cpu", "cpu4")][..],
            ] {
                assert_refused(
                    &ready_conn(route, family),
                    true,
                    &format!("{family:?} {route:?}"),
                );
            }
            // The own CPU, with or without clock_set, reads.
            for extra in [
                &[("target_cpu", "own")][..],
                &[(CLOCK_SET_KEY, "disabled")][..],
            ] {
                sent(read_clock_next_on(&ready_conn(extra, family), None));
            }
        }
        assert!(<Component as address::Guest>::help().contains("SM213"));
    }
}
