//! Optional clock-v1 (H): set the clock of a 0101-confirmed iQ-R CPU from the
//! Host's local civil time over the same connection as read and write.
//!
//! The guest gate is the connection setting `clock_set` (default disabled),
//! independent of the write mode. Before anything is sent the guest checks,
//! in order: the gate, the calendar date (1980-2079, Gregorian leap years,
//! since the CPU stores an impossible day such as 2/30 without SM211), the
//! model (standard R family only) and the destination (own station only).
//! The run is then four steps, each one `send` matched like any write (4E
//! serial, 3E issue order, route echo):
//!
//! 1. 0401 read of SM210-SM211. SM210 ON means an earlier request is still
//!    pending; nothing is written, since only an OFF to ON change is taken.
//! 2. 1401 word write of SD210-SD216 (year, month, day, hour, minute, second,
//!    day of week computed here).
//! 3. 1401 bit write of SM210 ON.
//! 4. 0401 read of SM210-SM211, at most three times: SM211 ON is a CPU
//!    refusal, SM210 and SM211 OFF is success, SM210 still ON after the
//!    third read leaves the outcome unknown. The guest cannot wait, so the
//!    spacing of the reads is the SLMP round trip.
//!
//! A PLC end code at any step ends the run with `device` and nothing more is
//! sent. A malformed response is fatal and leaves this connection unusable
//! until the Host reconnects. The Host serializes calls and closes the
//! connection after an abandoned send.
use super::*;
use banto_slmp_read_core::{
    encode_r_clock_data_write, encode_r_clock_set_request, encode_r_clock_status_read,
    CLOCK_STATUS_SPAN,
};
use exports::banto::driver::clock_v1;

pub(super) const CLOCK_SET_KEY: &str = "clock_set";
/// Step 4 reads SM210-SM211 at most this many times.
const MAX_CONFIRM_READS: u8 = 3;
/// `device` code reported when SM211 (clock data set error) is ON.
const SM211_DEVICE_CODE: u32 = 211;

/// The Host's civil time as passed to `set-clock-next`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Civil {
    pub(super) year: u16,
    pub(super) month: u8,
    pub(super) day: u8,
    pub(super) hour: u8,
    pub(super) minute: u8,
    pub(super) second: u8,
}

impl From<clock_v1::CivilTime> for Civil {
    fn from(time: clock_v1::CivilTime) -> Self {
        Self {
            year: time.year,
            month: time.month,
            day: time.day,
            hour: time.hour,
            minute: time.minute,
            second: time.second,
        }
    }
}

const fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Civil {
    /// The iQ-R and Q clock range (1980-2079) and a real calendar date.
    /// Also checks a clock read (H3).
    pub(super) fn validate(&self) -> Result<(), String> {
        if !(1980..=2079).contains(&self.year) {
            return Err(format!("year {} is outside 1980-2079", self.year));
        }
        if !(1..=12).contains(&self.month) {
            return Err(format!("month {} is outside 1-12", self.month));
        }
        let last = days_in_month(self.year, self.month);
        if !(1..=last).contains(&self.day) {
            return Err(format!(
                "day {} is outside 1-{last} for {}-{:02}",
                self.day, self.year, self.month
            ));
        }
        if self.hour > 23 {
            return Err(format!("hour {} is outside 0-23", self.hour));
        }
        if self.minute > 59 {
            return Err(format!("minute {} is outside 0-59", self.minute));
        }
        if self.second > 59 {
            return Err(format!("second {} is outside 0-59", self.second));
        }
        Ok(())
    }

    /// Day of the week, 0 = Sunday .. 6 = Saturday (Sakamoto's method, valid
    /// for any Gregorian date of a validated time).
    fn weekday(&self) -> u16 {
        const OFFSETS: [u16; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let year = if self.month < 3 {
            self.year - 1
        } else {
            self.year
        };
        (year + year / 4 - year / 100
            + year / 400
            + OFFSETS[usize::from(self.month - 1)]
            + u16::from(self.day))
            % 7
    }

    /// SD210-SD216 as BIN words.
    fn sd_words(&self) -> [u16; 7] {
        [
            self.year,
            u16::from(self.month),
            u16::from(self.day),
            u16::from(self.hour),
            u16::from(self.minute),
            u16::from(self.second),
            self.weekday(),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClockStage {
    /// Step 1: SM210-SM211 before writing.
    CheckIdle,
    /// Step 2: SD210-SD216 write.
    WriteData,
    /// Step 3: SM210 ON.
    SetRequest,
    /// Step 4: the n-th (1-based) SM210-SM211 read after the request.
    Confirm(u8),
}

pub(super) struct PendingClock {
    time: Civil,
    route: Route,
    stage: ClockStage,
}

impl SlmpConnection {
    /// A clock set or clock read (H3) response is awaited, or one was
    /// malformed.
    pub(super) fn clock_busy(&self) -> bool {
        self.pending_clock.borrow().is_some() || self.clock_poisoned.get() || self.clock_read_busy()
    }
}

fn finish(conn: &SlmpConnection, result: Result<(), ItemError>) -> clock_v1::ClockProgress {
    conn.clock_poisoned.set(false);
    clock_v1::ClockProgress::Done(result)
}

fn end_code_error(code: u16, stage: ClockStage) -> ItemError {
    let after = match stage {
        ClockStage::CheckIdle | ClockStage::WriteData => "; the clock was not set",
        ClockStage::SetRequest | ClockStage::Confirm(_) => "; the clock may already be set",
    };
    ItemError::Device(banto::driver::types::DeviceError {
        code: u32::from(code),
        message: format!("SLMP end code 0x{code:04X}{after}"),
    })
}

/// Store the next step and return it.
fn send(conn: &SlmpConnection, time: Civil, stage: ClockStage) -> clock_v1::ClockProgress {
    let route = conn.route();
    let step = match stage {
        ClockStage::CheckIdle | ClockStage::Confirm(_) => read_step(
            route.frame,
            encode_r_clock_status_read(route),
            CLOCK_STATUS_SPAN,
        ),
        ClockStage::WriteData => frame_step(
            route.frame,
            encode_r_clock_data_write(route, time.sd_words()),
        ),
        ClockStage::SetRequest => frame_step(route.frame, encode_r_clock_set_request(route)),
    };
    *conn.pending_clock.borrow_mut() = Some(PendingClock { time, route, stage });
    clock_v1::ClockProgress::Send(step)
}

/// SM210 and SM211 from a status response; `Err` is the PLC end code.
fn status(route: Route, response: &[u8]) -> Result<Result<(bool, bool), u16>, FatalError> {
    match decode_read(route, CLOCK_STATUS_SPAN, response) {
        Ok(Values::Bits(bits)) if bits.len() == 2 => Ok(Ok((bits[0], bits[1]))),
        Ok(_) => Err(FatalError::Protocol(
            "clock status read returned an unexpected unit".into(),
        )),
        Err(ResponseError::EndCode(code)) => Ok(Err(code)),
        Err(error) => Err(response_error(error)),
    }
}

fn respond(
    conn: &SlmpConnection,
    pending: PendingClock,
    response: &[u8],
) -> Result<clock_v1::ClockProgress, FatalError> {
    let PendingClock { time, route, stage } = pending;
    match stage {
        ClockStage::CheckIdle => match status(route, response)? {
            Err(code) => Ok(finish(conn, Err(end_code_error(code, stage)))),
            Ok((true, _)) => Ok(finish(
                conn,
                Err(ItemError::Rejected(
                    "previous clock set request still pending (SM210 ON)".into(),
                )),
            )),
            Ok((false, _)) => Ok(send(conn, time, ClockStage::WriteData)),
        },
        ClockStage::WriteData | ClockStage::SetRequest => match decode_write_ack(route, response) {
            Ok(()) => Ok(send(
                conn,
                time,
                if stage == ClockStage::WriteData {
                    ClockStage::SetRequest
                } else {
                    ClockStage::Confirm(1)
                },
            )),
            Err(ResponseError::EndCode(code)) => Ok(finish(conn, Err(end_code_error(code, stage)))),
            Err(error) => Err(response_error(error)),
        },
        ClockStage::Confirm(reads) => match status(route, response)? {
            Err(code) => Ok(finish(conn, Err(end_code_error(code, stage)))),
            Ok((_, true)) => Ok(finish(
                conn,
                Err(ItemError::Device(banto::driver::types::DeviceError {
                    code: SM211_DEVICE_CODE,
                    message: "SM211 clock data set error: the CPU rejected SD210-SD216".into(),
                })),
            )),
            Ok((false, false)) => Ok(finish(conn, Ok(()))),
            Ok((true, false)) if reads < MAX_CONFIRM_READS => {
                Ok(send(conn, time, ClockStage::Confirm(reads + 1)))
            }
            Ok((true, false)) => Ok(finish(
                conn,
                Err(ItemError::VerificationFailed(
                    "SM210 still ON after 3 reads; unknown whether the clock was set".into(),
                )),
            )),
        },
    }
}

/// Checks made before anything is sent; `Err` is the item error to return.
fn preflight(conn: &SlmpConnection, time: Civil) -> Result<(), ItemError> {
    if !conn.clock_set_enabled {
        return Err(ItemError::Rejected(
            "clock_set is disabled for this connection".into(),
        ));
    }
    time.validate().map_err(ItemError::Rejected)?;
    match &*conn.phase.borrow() {
        Phase::Ready { model, .. } => match model.family {
            // Includes the G2b process, SIL2, safety and MX-R CPUs.
            ModelFamily::StandardR => {}
            ModelFamily::StandardQ => {
                return Err(ItemError::Unsupported(
                    "standard Q clock set is not supported until it is confirmed on a device whether SM210 turns OFF by itself".into(),
                ))
            }
            ModelFamily::Q25PrhWireOnly => {
                return Err(ItemError::Unsupported(
                    "clock set is not supported on the redundant Q25PRH/Q12PRH CPU".into(),
                ))
            }
            ModelFamily::Fx5 => {
                return Err(ItemError::Unsupported(
                    "clock set is not supported on FX5".into(),
                ))
            }
            ModelFamily::Unknown => {
                return Err(ItemError::Unsupported(
                    "clock set requires a 0101-confirmed iQ-R CPU".into(),
                ))
            }
        },
        Phase::NoRange(Some(model)) if model.family == ModelFamily::Fx5 => {
            return Err(ItemError::Unsupported(
                "clock set is not supported on FX5".into(),
            ))
        }
        Phase::NoRange(_) => {
            return Err(ItemError::Unsupported(
                "clock set requires a 0101-confirmed iQ-R CPU".into(),
            ))
        }
        _ => return Err(ItemError::Rejected("discovery is incomplete".into())),
    }
    let own = conn.base_route.network == 0
        && conn.base_route.pc == 0xff
        && conn.base_route.io == 0x03ff
        && conn.base_route.station == 0;
    if !own {
        return Err(ItemError::Unsupported(
            "clock set is supported on the connected station's own CPU only".into(),
        ));
    }
    Ok(())
}

pub(super) fn set_clock_next_on(
    conn: &SlmpConnection,
    time: Civil,
    prev_response: Option<Vec<u8>>,
) -> Result<clock_v1::ClockProgress, FatalError> {
    if let Some(response) = prev_response {
        // Consume before parsing. The poison stays set across the steps of a
        // run; only a final outcome clears it.
        conn.clock_poisoned.set(true);
        let pending = conn.pending_clock.borrow_mut().take().ok_or_else(|| {
            FatalError::Protocol("clock response without a pending request".into())
        })?;
        if pending.time != time {
            return Err(FatalError::Protocol("clock time changed after send".into()));
        }
        return respond(conn, pending, &response);
    }
    if conn.clock_poisoned.get() {
        return Err(FatalError::Protocol(
            "clock outcome is uncertain; reconnect".into(),
        ));
    }
    if conn.pending_clock.borrow().is_some() {
        conn.clock_poisoned.set(true);
        return Err(FatalError::Protocol(
            "clock response is missing; reconnect".into(),
        ));
    }
    if conn.pending_write.borrow().is_some()
        || conn.write_poisoned.get()
        || !conn.pending.borrow().is_empty()
        || conn.pending_diagnostic.borrow().is_some()
        || conn.diagnostic_poisoned.get()
        || conn.clock_read_busy()
    {
        return Err(FatalError::Protocol(
            "another response is pending on this connection".into(),
        ));
    }
    if let Err(error) = preflight(conn, time) {
        return Ok(clock_v1::ClockProgress::Done(Err(error)));
    }
    Ok(send(conn, time, ClockStage::CheckIdle))
}

impl clock_v1::Guest for Component {
    fn set_clock_next(
        conn: clock_v1::ConnectionBorrow<'_>,
        time: clock_v1::CivilTime,
        prev_response: Option<Vec<u8>>,
    ) -> Result<clock_v1::ClockProgress, FatalError> {
        set_clock_next_on(conn.get::<SlmpConnection>(), time.into(), prev_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn civil(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Civil {
        Civil {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    fn date(year: u16, month: u8, day: u8) -> Civil {
        civil(year, month, day, 0, 0, 0)
    }

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
        let mut all = vec![(CLOCK_SET_KEY, "enabled")];
        for &(key, value) in extra {
            all.retain(|&(existing, _)| existing != key);
            all.push((key, value));
        }
        let conn = SlmpConnection::new(&settings(&all)).expect("valid settings");
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

    /// SM210 (high nibble) and SM211 (low nibble) in one response byte.
    fn bits(sm210: bool, sm211: bool) -> [u8; 1] {
        [(u8::from(sm210) << 4) | u8::from(sm211)]
    }

    const TIME: Civil = Civil {
        year: 2026,
        month: 11,
        day: 6,
        hour: 12,
        minute: 22,
        second: 5,
    };

    fn sent(progress: Result<clock_v1::ClockProgress, FatalError>) -> session::Step {
        let Ok(clock_v1::ClockProgress::Send(step)) = progress else {
            panic!("clock step must send: {progress:?}")
        };
        step
    }

    fn done(progress: Result<clock_v1::ClockProgress, FatalError>) -> Result<(), ItemError> {
        let Ok(clock_v1::ClockProgress::Done(result)) = progress else {
            panic!("clock run must finish: {progress:?}")
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

    const STATUS_BODY: [u8; 12] = [1, 4, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 2, 0];
    const DATA_BODY: [u8; 26] = [
        1, 0x14, 2, 0, 0xd2, 0, 0, 0, 0xa9, 0, 7, 0, 0xea, 0x07, 11, 0, 6, 0, 12, 0, 22, 0, 5, 0,
        5, 0,
    ];
    const SET_BODY: [u8; 13] = [1, 0x14, 3, 0, 0xd2, 0, 0, 0, 0x91, 0, 1, 0, 0x10];

    #[test]
    fn calendar_dates_follow_the_gregorian_rules_and_the_iq_r_range() {
        for ok in [
            date(2024, 2, 29),
            date(2000, 2, 29),
            date(2023, 2, 28),
            date(2026, 12, 31),
            date(2026, 4, 30),
            date(1980, 1, 1),
            civil(2079, 12, 31, 23, 59, 59),
        ] {
            assert_eq!(ok.validate(), Ok(()), "{ok:?}");
        }
        for (bad, field) in [
            (date(2023, 2, 29), "day"),
            (date(2024, 2, 30), "day"),
            (date(2026, 4, 31), "day"),
            (date(2026, 1, 0), "day"),
            (date(2026, 1, 32), "day"),
            (date(1979, 12, 31), "year"),
            (date(2080, 1, 1), "year"),
            (date(2026, 0, 1), "month"),
            (date(2026, 13, 1), "month"),
            (civil(2026, 1, 1, 24, 0, 0), "hour"),
            (civil(2026, 1, 1, 0, 60, 0), "minute"),
            (civil(2026, 1, 1, 0, 0, 60), "second"),
        ] {
            let message = bad.validate().expect_err("invalid date");
            assert!(message.starts_with(field), "{bad:?}: {message}");
        }
        // The century rule: 1900 and 2100 are not leap years, 2000 is.
        assert!(!is_leap_year(1900) && !is_leap_year(2100) && is_leap_year(2000));
        assert_eq!(days_in_month(2023, 2), 28);
    }

    #[test]
    fn weekday_is_zero_for_sunday() {
        assert_eq!(date(1980, 1, 1).weekday(), 2); // Tuesday
        assert_eq!(date(2000, 2, 29).weekday(), 2); // Tuesday
        assert_eq!(date(2000, 3, 1).weekday(), 3); // Wednesday
        assert_eq!(date(2024, 2, 29).weekday(), 4); // Thursday
        assert_eq!(date(2026, 11, 6).weekday(), 5); // Friday
        assert_eq!(date(2026, 11, 7).weekday(), 6); // Saturday
        assert_eq!(date(2079, 12, 31).weekday(), 0); // Sunday
        assert_eq!(TIME.sd_words(), [2026, 11, 6, 12, 22, 5, 5]);
        // Every valid date is one day after the previous one.
        let mut expected = 2;
        for year in 1980..=2079 {
            for month in 1..=12 {
                for day in 1..=days_in_month(year, month) {
                    assert_eq!(date(year, month, day).weekday(), expected);
                    expected = (expected + 1) % 7;
                }
            }
        }
    }

    #[test]
    fn r_4e_clock_set_sends_four_exact_steps_and_succeeds() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        let step = sent(set_clock_next_on(&conn, TIME, None));
        assert_eq!(step.send, request(Frame::E4, 3, &STATUS_BODY));
        assert_eq!(step.max_len, 64);
        let step = sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 3, 0, &bits(false, true))),
        ));
        // SM211 left ON by an earlier failure does not block a new set.
        assert_eq!(step.send, request(Frame::E4, 4, &DATA_BODY));
        let step = sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 4, 0, &[])),
        ));
        assert_eq!(step.send, request(Frame::E4, 5, &SET_BODY));
        let step = sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 5, 0, &[])),
        ));
        assert_eq!(step.send, request(Frame::E4, 6, &STATUS_BODY));
        assert!(done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 6, 0, &bits(false, false)))
        ))
        .is_ok());
        assert!(!conn.clock_busy());
        // A new run starts with prev-response none.
        let step = sent(set_clock_next_on(&conn, TIME, None));
        assert_eq!(step.send, request(Frame::E4, 7, &STATUS_BODY));
    }

    #[test]
    fn r_3e_clock_set_uses_issue_order_and_the_same_bodies() {
        let conn = ready_conn(&[("frame", "3e")], ModelFamily::StandardR);
        let step = sent(set_clock_next_on(&conn, TIME, None));
        assert_eq!(step.send, request(Frame::E3, 0, &STATUS_BODY));
        let step = sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 0, 0, &bits(false, false))),
        ));
        assert_eq!(step.send, request(Frame::E3, 0, &DATA_BODY));
        let step = sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 0, 0, &[])),
        ));
        assert_eq!(step.send, request(Frame::E3, 0, &SET_BODY));
        sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 0, 0, &[])),
        ));
        assert!(done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 0, 0, &bits(false, false)))
        ))
        .is_ok());
    }

    /// Drive a 4E run to step 4 and return the next serial.
    fn to_confirm(conn: &SlmpConnection) -> u16 {
        sent(set_clock_next_on(conn, TIME, None));
        sent(set_clock_next_on(
            conn,
            TIME,
            Some(reply(conn, 3, 0, &bits(false, false))),
        ));
        sent(set_clock_next_on(conn, TIME, Some(reply(conn, 4, 0, &[]))));
        sent(set_clock_next_on(conn, TIME, Some(reply(conn, 5, 0, &[]))));
        6
    }

    #[test]
    fn confirm_reads_map_sm210_and_sm211() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        let serial = to_confirm(&conn);
        let Err(ItemError::Device(error)) = done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, serial, 0, &bits(false, true))),
        )) else {
            panic!("SM211 ON is a device error")
        };
        assert_eq!(error.code, 211);
        assert!(error.message.contains("SM211"));

        let conn = ready_conn(&[], ModelFamily::StandardR);
        let serial = to_confirm(&conn);
        sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, serial, 0, &bits(true, false))),
        ));
        sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, serial + 1, 0, &bits(true, false))),
        ));
        assert!(done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, serial + 2, 0, &bits(false, false)))
        ))
        .is_ok());

        let conn = ready_conn(&[], ModelFamily::StandardR);
        let serial = to_confirm(&conn);
        for n in 0..2 {
            sent(set_clock_next_on(
                &conn,
                TIME,
                Some(reply(&conn, serial + n, 0, &bits(true, false))),
            ));
        }
        let Err(ItemError::VerificationFailed(message)) = done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, serial + 2, 0, &bits(true, false))),
        )) else {
            panic!("three SM210 ON reads leave the outcome unknown")
        };
        assert!(message.contains("3 reads"));
        assert!(!conn.clock_busy());
    }

    #[test]
    fn sm210_on_before_writing_is_rejected_after_one_read() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        let Err(ItemError::Rejected(message)) = done(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 3, 0, &bits(true, false))),
        )) else {
            panic!("SM210 ON refuses the run")
        };
        assert!(message.contains("SM210 ON"));
        assert_eq!(conn.next_serial.get(), 4, "only the status read was sent");
    }

    #[test]
    fn end_codes_end_the_run_at_every_step() {
        for stage in 0..4u16 {
            let conn = ready_conn(&[], ModelFamily::StandardR);
            sent(set_clock_next_on(&conn, TIME, None));
            for serial in 3..3 + stage {
                let body = if serial == 3 || serial == 6 {
                    bits(false, false).to_vec()
                } else {
                    Vec::new()
                };
                sent(set_clock_next_on(
                    &conn,
                    TIME,
                    Some(reply(&conn, serial, 0, &body)),
                ));
            }
            let serial = 3 + stage;
            let Err(ItemError::Device(error)) = done(set_clock_next_on(
                &conn,
                TIME,
                Some(reply(&conn, serial, 0xc059, &[])),
            )) else {
                panic!("step {}: end code is a device error", stage + 1)
            };
            assert_eq!(error.code, 0xc059);
            assert!(error.message.contains("0xC059"));
            assert_eq!(conn.next_serial.get(), serial + 1, "nothing more was sent");
            assert!(!conn.clock_busy());
        }
    }

    #[test]
    fn malformed_or_mismatched_responses_are_fatal_and_poison_the_clock() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        assert!(matches!(
            set_clock_next_on(&conn, TIME, Some(reply(&conn, 9, 0, &bits(false, false)))),
            Err(FatalError::Framing(_))
        ));
        assert!(conn.clock_busy());
        assert!(matches!(
            set_clock_next_on(&conn, TIME, None),
            Err(FatalError::Protocol(_))
        ));
        assert!(
            plan_on(&conn, Vec::new()).is_err(),
            "reads wait for reconnect"
        );

        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        let mut short = reply(&conn, 3, 0, &bits(false, false));
        short.pop();
        assert!(matches!(
            set_clock_next_on(&conn, TIME, Some(short)),
            Err(FatalError::Framing(_))
        ));

        // A write ack with data after its end code.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        sent(set_clock_next_on(
            &conn,
            TIME,
            Some(reply(&conn, 3, 0, &bits(false, false))),
        ));
        assert!(matches!(
            set_clock_next_on(&conn, TIME, Some(reply(&conn, 4, 0, &[0, 0]))),
            Err(FatalError::Framing(_))
        ));

        // Another time in the middle of a run.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        let other = Civil { second: 6, ..TIME };
        assert!(matches!(
            set_clock_next_on(&conn, other, Some(reply(&conn, 3, 0, &bits(false, false)))),
            Err(FatalError::Protocol(_))
        ));

        // A send abandoned without its response.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        assert!(matches!(
            set_clock_next_on(&conn, TIME, None),
            Err(FatalError::Protocol(_))
        ));
        assert!(conn.clock_busy());

        // A response without a run.
        let conn = ready_conn(&[], ModelFamily::StandardR);
        assert!(matches!(
            set_clock_next_on(&conn, TIME, Some(reply(&conn, 3, 0, &[]))),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn other_operations_wait_for_a_pending_clock_step() {
        let conn = ready_conn(&[], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));
        assert!(plan_on(&conn, Vec::new()).is_err());
        assert!(write_next_on(&conn, Vec::new(), None).is_err());
        assert!(<SlmpConnection as session::GuestConnection>::handshake_next(&conn, None).is_err());
    }

    fn assert_refused(conn: &SlmpConnection, time: Civil, unsupported: bool, why: &str) {
        let serial = conn.next_serial.get();
        let result = done(set_clock_next_on(conn, time, None));
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
    fn refusals_before_send_follow_gate_date_model_and_route_order() {
        let disabled = ready_conn(&[(CLOCK_SET_KEY, "disabled")], ModelFamily::StandardR);
        assert_refused(&disabled, TIME, false, "disabled");
        let default = SlmpConnection::new(&settings(&[])).expect("valid");
        *default.phase.borrow_mut() = Phase::Ready {
            model: model(ModelFamily::StandardR),
            count: PointCount::Unknown,
        };
        assert_refused(&default, TIME, false, "default is disabled");
        // The gate comes before the model, the date before the model.
        let q_disabled = ready_conn(&[(CLOCK_SET_KEY, "disabled")], ModelFamily::StandardQ);
        assert_refused(&q_disabled, TIME, false, "disabled Q");
        let q = ready_conn(&[], ModelFamily::StandardQ);
        assert_refused(&q, date(2026, 2, 30), false, "bad date on Q");
        let r = ready_conn(&[], ModelFamily::StandardR);
        assert_refused(&r, date(2026, 2, 30), false, "Feb 30");
        assert_refused(&r, date(2080, 1, 1), false, "2080");
        for family in [
            ModelFamily::StandardQ,
            ModelFamily::Q25PrhWireOnly,
            ModelFamily::Fx5,
            ModelFamily::Unknown,
        ] {
            assert_refused(&ready_conn(&[], family), TIME, true, &format!("{family:?}"));
        }
        let no_range = ready_conn(&[], ModelFamily::StandardR);
        *no_range.phase.borrow_mut() = Phase::NoRange(None);
        assert_refused(&no_range, TIME, true, "0101 refused");
        *no_range.phase.borrow_mut() = Phase::NoRange(Some(model(ModelFamily::Fx5)));
        assert_refused(&no_range, TIME, true, "FX5 over 4E");
        *no_range.phase.borrow_mut() = Phase::Start;
        assert_refused(&no_range, TIME, false, "handshake incomplete");
        for route in [
            &[("network_no", "1"), ("station_no", "2")][..],
            &[("target_cpu", "cpu1")][..],
            &[("target_cpu", "cpu2")][..],
        ] {
            assert_refused(
                &ready_conn(route, ModelFamily::StandardR),
                TIME,
                true,
                &format!("{route:?}"),
            );
        }
        let own = ready_conn(&[("target_cpu", "own")], ModelFamily::StandardR);
        sent(set_clock_next_on(&own, TIME, None));
    }

    #[test]
    fn clock_set_setting_is_validated_declared_and_independent_of_write_mode() {
        let messages = |extra: &[(&str, &str)]| {
            validate_settings(&settings(extra))
                .into_iter()
                .map(|error| (error.key, error.message))
                .collect::<Vec<_>>()
        };
        assert!(messages(&[(CLOCK_SET_KEY, "enabled")]).is_empty());
        assert!(messages(&[(CLOCK_SET_KEY, "disabled")]).is_empty());
        for bad in ["Enabled", "on", "", "true"] {
            assert_eq!(
                messages(&[(CLOCK_SET_KEY, bad)]),
                vec![(
                    CLOCK_SET_KEY.to_owned(),
                    "clock_set must be disabled or enabled".to_owned()
                )],
                "{bad}"
            );
        }
        // The write mode stays disabled: clock_set alone is the guest gate.
        let conn = ready_conn(&[("d_u16_write_mode", "disabled")], ModelFamily::StandardR);
        sent(set_clock_next_on(&conn, TIME, None));

        let schema = <Component as config::Guest>::schema();
        let field = schema
            .iter()
            .find(|field| field.key == CLOCK_SET_KEY)
            .expect("clock_set is declared");
        assert!(matches!(field.group, config::FieldGroup::Advanced));
        assert!(!field.required);
        assert!(matches!(
            &field.default,
            Some(config::SettingValue::Text(value)) if value == "disabled"
        ));
        let config::FieldKind::Choice(options) = &field.kind else {
            panic!("clock_set is a choice")
        };
        assert_eq!(
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["disabled", "enabled"]
        );
        let help = field.help.as_deref().expect("help");
        for needle in [
            "iQ-R",
            "SD210-SD216",
            "SM210",
            "24000",
            "SNTP",
            "GOT",
            "Host",
        ] {
            assert!(help.contains(needle), "{needle}");
        }
        assert!(<Component as address::Guest>::help().contains("clock_set"));
    }
}
