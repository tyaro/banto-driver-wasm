//! Binary 4E SLMP guest for numeric D and standard iQ-R W reads, plus restricted D/u16 write.
//! The host owns transport. Each connection owns its discovery evidence.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_register_codec::{
    decode_f32, decode_f64, decode_i16, decode_i32, decode_i64, decode_u32, decode_u64, WordOrder,
};
use banto_slmp_read_core::{
    basic_sd_count, decode_read, decode_type_name, decode_write_ack, encode_r_d_words, encode_read,
    encode_type_name, plan_basic_sd, BasicArea, BasicEvidence, Device, ModelFamily, PointCount,
    ReadSpan, ResponseError, Route, Series, TypeName, Values,
};
use exports::banto::driver::{address, config, read, session, write_v1};

wit_bindgen::generate!({ path: "../wit", world: "driver-with-write-v1" });

struct Component;

enum Phase {
    Start,
    AwaitType {
        route: Route,
    },
    AwaitSd {
        route: Route,
        model: TypeName,
        span: ReadSpan,
    },
    Ready {
        model: TypeName,
        count: PointCount,
    },
    NoRange(Option<TypeName>),
}

struct SlmpConnection {
    base_route: Route,
    word_order: WordOrder,
    next_serial: Cell<u16>,
    phase: RefCell<Phase>,
    w_count: Cell<PointCount>,
    pending: RefCell<BTreeMap<u16, PendingRead>>,
    pending_write: RefCell<Option<PendingWrite>>,
    write_poisoned: Cell<bool>,
}

#[derive(Clone)]
struct PendingWrite {
    route: Route,
    number: u32,
    values: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingRead {
    span: ReadSpan,
    covers: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DKind {
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl DKind {
    fn parse(data_type: &str, string_words: Option<u32>) -> Result<Self, String> {
        if string_words.is_some() {
            return Err("numeric D does not accept string_words".into());
        }
        match data_type {
            "i16" => Ok(Self::I16),
            "u16" => Ok(Self::U16),
            "i32" => Ok(Self::I32),
            "u32" => Ok(Self::U32),
            "i64" => Ok(Self::I64),
            "u64" => Ok(Self::U64),
            "f32" => Ok(Self::F32),
            "f64" => Ok(Self::F64),
            _ => Err("unsupported numeric D type".into()),
        }
    }

    const fn words(self) -> u16 {
        match self {
            Self::I16 | Self::U16 => 1,
            Self::I32 | Self::U32 | Self::F32 => 2,
            Self::I64 | Self::U64 | Self::F64 => 4,
        }
    }

    fn decode(self, words: &[u16], order: WordOrder) -> Result<Value, FatalError> {
        if words.len() != usize::from(self.words()) {
            return Err(FatalError::Protocol(
                "tag word count does not match plan".into(),
            ));
        }
        let pair = || [words[0], words[1]];
        let quad = || [words[0], words[1], words[2], words[3]];
        Ok(match self {
            Self::I16 => Value::Int(i64::from(decode_i16(words[0]))),
            Self::U16 => Value::Uint(u64::from(words[0])),
            Self::I32 => Value::Int(i64::from(decode_i32(pair(), order))),
            Self::U32 => Value::Uint(u64::from(decode_u32(pair(), order))),
            Self::I64 => Value::Int(decode_i64(quad(), order)),
            Self::U64 => Value::Uint(decode_u64(quad(), order)),
            Self::F32 => Value::Float(f64::from(decode_f32(pair(), order))),
            Self::F64 => Value::Float(decode_f64(quad(), order)),
        })
    }
}

impl SlmpConnection {
    fn route(&self) -> Route {
        let serial = self.next_serial.get();
        self.next_serial.set(serial.wrapping_add(1));
        Route {
            serial,
            ..self.base_route
        }
    }
}

fn frame_step(send: Vec<u8>) -> session::Step {
    session::Step {
        send,
        expect: session::FrameRule::LengthPrefixed(session::PrefixSpec {
            offset: 11,
            width: 2,
            big_endian: false,
            adjust: 13,
            encoding: session::PrefixEncoding::Binary,
        }),
        max_len: 64,
    }
}

fn read_step(send: Vec<u8>, words: u16) -> session::Step {
    let mut step = frame_step(send);
    // A successful binary 4E word response has 15 header bytes and 2 bytes
    // per word. Keep the previous allowance for short PLC refusal frames.
    step.max_len = (15 + u32::from(words) * 2).max(64);
    step
}

fn response_error(error: ResponseError) -> FatalError {
    match error {
        ResponseError::Framing | ResponseError::MalformedName => {
            FatalError::Framing("invalid SLMP response".into())
        }
        ResponseError::EndCode(code) => FatalError::Protocol(format!("SLMP end code 0x{code:04X}")),
    }
}

fn setting<'a>(settings: &'a [session::Setting], key: &str) -> Option<&'a str> {
    settings
        .iter()
        .find(|item| item.key == key)
        .map(|item| item.value.as_str())
}

fn validate_settings(settings: &[session::Setting]) -> Vec<config::FieldError> {
    let mut errors = Vec::new();
    for (i, item) in settings.iter().enumerate() {
        if settings[..i].iter().any(|earlier| earlier.key == item.key) {
            errors.push(config::FieldError {
                key: item.key.clone(),
                message: "duplicate setting".into(),
            });
        }
        if !matches!(
            item.key.as_str(),
            "host" | "port" | "transport" | "word_order"
        ) {
            errors.push(config::FieldError {
                key: item.key.clone(),
                message: "unsupported setting".into(),
            });
        }
    }
    if setting(settings, "host").is_none_or(|host| host.trim().is_empty()) {
        errors.push(config::FieldError {
            key: "host".into(),
            message: "host is required".into(),
        });
    }
    if setting(settings, "port")
        .and_then(|port| port.parse::<u16>().ok())
        .is_none_or(|port| port == 0)
    {
        errors.push(config::FieldError {
            key: "port".into(),
            message: "port must be 1..65535".into(),
        });
    }
    if setting(settings, "transport").is_some_and(|value| value != "tcp") {
        errors.push(config::FieldError {
            key: "transport".into(),
            message: "only tcp is supported".into(),
        });
    }
    if setting(settings, "word_order")
        .is_some_and(|value| !matches!(value, "low_high" | "high_low"))
    {
        errors.push(config::FieldError {
            key: "word_order".into(),
            message: "word_order must be low_high or high_low".into(),
        });
    }
    errors
}

fn parse_d(text: &str, data_type: &str, words: Option<u32>) -> Result<u32, String> {
    if data_type != "u16" || words.is_some() {
        return Err("only single-word D u16 is supported".into());
    }
    parse_d_number(text)
}

fn parse_read_d(text: &str, data_type: &str, words: Option<u32>) -> Result<(u32, DKind), String> {
    let kind = DKind::parse(data_type, words)?;
    let number = parse_d_number(text)?;
    number
        .checked_add(u32::from(kind.words() - 1))
        .ok_or("D tag address overflows u32")?;
    Ok((number, kind))
}

fn parse_d_number(text: &str) -> Result<u32, String> {
    let digits = text.strip_prefix('D').ok_or("D address required")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid D address".into());
    }
    digits
        .parse::<u32>()
        .map_err(|_| "D address exceeds u32".into())
}

fn parse_read_address(
    text: &str,
    data_type: &str,
    words: Option<u32>,
) -> Result<(Device, u32, DKind), String> {
    let kind = DKind::parse(data_type, words)?;
    if text.starts_with('D') {
        let (number, kind) = parse_read_d(text, data_type, words)?;
        return Ok((Device::D, number, kind));
    }
    let digits = text.strip_prefix('W').ok_or("D or W address required")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid W hexadecimal address".into());
    }
    let number = u32::from_str_radix(digits, 16).map_err(|_| "W address exceeds u32")?;
    number
        .checked_add(u32::from(kind.words() - 1))
        .ok_or("W tag address overflows u32")?;
    Ok((Device::W, number, kind))
}

fn canonical_read(device: Device, number: i64) -> Result<String, String> {
    let number = u32::try_from(number).map_err(|_| "read address exceeds u32")?;
    match device {
        Device::D => Ok(format!("D{number}")),
        Device::W => Ok(format!("W{number:X}")),
        _ => Err("unsupported read area".into()),
    }
}

fn known_series(model: &TypeName) -> Option<Series> {
    match model.family {
        ModelFamily::StandardQ | ModelFamily::Q25PrhWireOnly => Some(Series::Q),
        ModelFamily::StandardR => Some(Series::R),
        ModelFamily::Unknown => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RangeAssessment {
    VerifiedInside,
    VerifiedOutside,
    Unknown,
}

#[cfg(test)]
fn assess_d(model: &TypeName, count: PointCount, number: u32) -> RangeAssessment {
    assess_d_span(model, count, number, 1)
}

fn assess_d_span(model: &TypeName, count: PointCount, number: u32, words: u16) -> RangeAssessment {
    if model.family == ModelFamily::Q25PrhWireOnly {
        return RangeAssessment::Unknown;
    }
    let Some(last) = number.checked_add(u32::from(words - 1)) else {
        return RangeAssessment::VerifiedOutside;
    };
    match count {
        PointCount::Exact(limit) if last < limit => RangeAssessment::VerifiedInside,
        PointCount::AtLeast(prefix) if last < prefix => RangeAssessment::VerifiedInside,
        // R SD280-281 is the full D count. Q SD302 covers internal D only;
        // an address beyond it may belong to extended D.
        PointCount::Exact(limit) if model.family == ModelFamily::StandardR && last >= limit => {
            RangeAssessment::VerifiedOutside
        }
        _ => RangeAssessment::Unknown,
    }
}

fn checked_read_span(
    spec: &TagSpec,
    model: &TypeName,
    d_count: PointCount,
    w_count: PointCount,
) -> Result<ReadSpan, FatalError> {
    let (device, number, kind) =
        parse_read_address(&spec.address, &spec.data_type, spec.string_words)
            .map_err(FatalError::InvalidConfig)?;
    if device == Device::W && model.family != ModelFamily::StandardR {
        return Err(FatalError::InvalidConfig(
            "W reads require a confirmed standard iQ-R CPU".into(),
        ));
    }
    let count = if device == Device::W {
        w_count
    } else {
        d_count
    };
    if assess_d_span(model, count, number, kind.words()) == RangeAssessment::VerifiedOutside {
        return Err(FatalError::InvalidConfig(
            "address exceeds verified configured count".into(),
        ));
    }
    Ok(ReadSpan {
        device,
        start: number,
        count: kind.words(),
    })
}

impl session::GuestConnection for SlmpConnection {
    fn create(settings: Vec<session::Setting>) -> Result<session::Connection, FatalError> {
        if let Some(error) = validate_settings(&settings).first() {
            return Err(FatalError::InvalidConfig(error.message.clone()));
        }
        Ok(session::Connection::new(Self {
            base_route: Route::default(),
            word_order: match setting(&settings, "word_order") {
                Some("high_low") => WordOrder::HighLow,
                _ => WordOrder::LowHigh,
            },
            next_serial: Cell::new(1),
            phase: RefCell::new(Phase::Start),
            w_count: Cell::new(PointCount::Unknown),
            pending: RefCell::new(BTreeMap::new()),
            pending_write: RefCell::new(None),
            write_poisoned: Cell::new(false),
        }))
    }

    fn handshake_next(
        &self,
        prev_response: Option<Vec<u8>>,
    ) -> Result<Option<session::Step>, FatalError> {
        let phase = std::mem::replace(&mut *self.phase.borrow_mut(), Phase::NoRange(None));
        match (phase, prev_response) {
            (Phase::Start, None) => {
                let route = self.route();
                *self.phase.borrow_mut() = Phase::AwaitType { route };
                Ok(Some(frame_step(encode_type_name(route))))
            }
            (Phase::AwaitType { route }, Some(frame)) => {
                let model = match decode_type_name(route, &frame) {
                    Ok(model) => model,
                    Err(ResponseError::EndCode(_)) => {
                        // The PLC answered 0101 but supplied no model. There is no
                        // series evidence for selecting Q/R 0401 framing or SD.
                        *self.phase.borrow_mut() = Phase::NoRange(None);
                        return Ok(None);
                    }
                    Err(error) => return Err(response_error(error)),
                };
                let area = match model.family {
                    ModelFamily::StandardQ => BasicArea::DInternal,
                    ModelFamily::StandardR => BasicArea::D,
                    ModelFamily::Q25PrhWireOnly => {
                        // Q word wire is established, but the SD302 map is not.
                        *self.phase.borrow_mut() = Phase::Ready {
                            model,
                            count: PointCount::Unknown,
                        };
                        return Ok(None);
                    }
                    ModelFamily::Unknown => {
                        *self.phase.borrow_mut() = Phase::NoRange(Some(model));
                        return Ok(None);
                    }
                };
                let mut span = plan_basic_sd(&model, area).expect("known model has D count SD");
                // R SD280-283 is a contiguous D/W count snapshot in one read.
                if model.family == ModelFamily::StandardR {
                    span.count = 4;
                }
                let sd_route = self.route();
                let series = known_series(&model).expect("known model has series");
                let send = encode_read(sd_route, series, span)
                    .map_err(|_| FatalError::Protocol("invalid SD read span".into()))?;
                *self.phase.borrow_mut() = Phase::AwaitSd {
                    route: sd_route,
                    model,
                    span,
                };
                Ok(Some(frame_step(send)))
            }
            (Phase::AwaitSd { route, model, span }, Some(frame)) => {
                let words = match decode_read(route, span, &frame) {
                    Ok(Values::Words(words)) => words,
                    Err(ResponseError::EndCode(_)) => {
                        // A PLC refusal leaves the count unknown, but the confirmed
                        // model still gives us a supported wire format for explicit reads.
                        *self.phase.borrow_mut() = Phase::Ready {
                            model,
                            count: PointCount::Unknown,
                        };
                        return Ok(None);
                    }
                    Err(error) => return Err(response_error(error)),
                    Ok(Values::Bits(_)) => {
                        return Err(FatalError::Protocol("SD read returned bits".into()));
                    }
                };
                let area = if model.family == ModelFamily::StandardQ {
                    BasicArea::DInternal
                } else {
                    BasicArea::D
                };
                let evidence = BasicEvidence {
                    route_confirmed: true,
                    r_firmware: None,
                };
                let count = if model.family == ModelFamily::StandardR {
                    self.w_count.set(basic_sd_count(
                        &model,
                        BasicArea::W,
                        evidence,
                        words.get(2..4),
                    ));
                    basic_sd_count(&model, area, evidence, words.get(0..2))
                } else {
                    basic_sd_count(&model, area, evidence, Some(&words))
                };
                *self.phase.borrow_mut() = Phase::Ready { model, count };
                Ok(None)
            }
            (Phase::Ready { model, count }, None) => {
                *self.phase.borrow_mut() = Phase::Ready { model, count };
                Ok(None)
            }
            (Phase::NoRange(model), None) => {
                *self.phase.borrow_mut() = Phase::NoRange(model);
                Ok(None)
            }
            _ => Err(FatalError::Protocol("invalid SLMP handshake order".into())),
        }
    }

    fn farewell(&self) -> Option<session::Step> {
        None
    }

    fn is_complete(
        &self,
        _step: session::Step,
        _buf: Vec<u8>,
    ) -> Result<session::FrameCheck, FatalError> {
        Err(FatalError::Protocol(
            "plugin-decides framing is not used".into(),
        ))
    }
}

impl session::Guest for Component {
    type Connection = SlmpConnection;
}

impl read::Guest for Component {
    fn plan(
        conn: read::ConnectionBorrow<'_>,
        specs: Vec<TagSpec>,
    ) -> Result<Vec<read::Planned>, FatalError> {
        let conn = conn.get::<SlmpConnection>();
        if conn.pending_write.borrow().is_some() || conn.write_poisoned.get() {
            return Err(FatalError::Protocol("write response is pending".into()));
        }
        let phase = conn.phase.borrow();
        let Phase::Ready { model, count } = &*phase else {
            return Err(FatalError::Protocol(
                "configured range is unresolved".into(),
            ));
        };
        let series = known_series(model).expect("ready model has series");
        // An unsupported tag is left uncovered: FrameSession returns its own
        // per-item Unsupported result without dropping the connection.
        let eligible = specs
            .iter()
            .enumerate()
            .filter_map(|(index, spec)| {
                let span = checked_read_span(spec, model, *count, conn.w_count.get()).ok()?;
                encode_read(conn.base_route, series, span).ok()?;
                Some((
                    u32::try_from(index).ok()?,
                    span,
                    assess_d_span(
                        model,
                        if span.device == Device::W {
                            conn.w_count.get()
                        } else {
                            *count
                        },
                        span.start,
                        span.count,
                    ) == RangeAssessment::VerifiedInside,
                ))
            })
            .collect::<Vec<_>>();
        let mut groups: Vec<PendingRead> = Vec::new();
        for (cover, span, verified) in eligible {
            // Preserve input order. A duplicate, a gap in either specs or D
            // addresses, or an unverified range starts a separate request.
            let joined = if verified {
                groups.last_mut().is_some_and(|last| {
                    if last.span.device != span.device
                        || assess_d_span(
                            model,
                            if span.device == Device::W {
                                conn.w_count.get()
                            } else {
                                *count
                            },
                            last.span.start,
                            last.span.count,
                        ) != RangeAssessment::VerifiedInside
                    {
                        return false;
                    }
                    let Some(next_cover) = last.covers.last().and_then(|n| n.checked_add(1)) else {
                        return false;
                    };
                    let Some(next_d) = last.span.start.checked_add(u32::from(last.span.count))
                    else {
                        return false;
                    };
                    let Some(next_count) = last.span.count.checked_add(span.count) else {
                        return false;
                    };
                    let candidate = ReadSpan {
                        count: next_count,
                        ..last.span
                    };
                    if next_cover != cover
                        || next_d != span.start
                        || encode_read(conn.base_route, series, candidate).is_err()
                    {
                        return false;
                    }
                    last.span = candidate;
                    last.covers.push(cover);
                    true
                })
            } else {
                false
            };
            if !joined {
                groups.push(PendingRead {
                    span,
                    covers: vec![cover],
                });
            }
        }
        if groups.len() > usize::from(u16::MAX) + 1 {
            return Err(FatalError::Protocol("SLMP serial space exhausted".into()));
        }
        let mut serial = conn.next_serial.get();
        let pending = conn.pending.borrow();
        let mut plans = Vec::with_capacity(groups.len());
        let mut new_pending = Vec::with_capacity(groups.len());
        for group in groups {
            if pending.contains_key(&serial) {
                return Err(FatalError::Protocol(
                    "SLMP serial reused while a read is pending".into(),
                ));
            }
            let route = Route {
                serial,
                ..conn.base_route
            };
            let send = encode_read(route, series, group.span)
                .map_err(|_| FatalError::Protocol("invalid D read span".into()))?;
            let step = read_step(send, group.span.count);
            plans.push(read::Planned {
                step,
                covers: group.covers.clone(),
            });
            new_pending.push((serial, group));
            serial = serial.wrapping_add(1);
        }
        drop(pending);
        conn.pending.borrow_mut().extend(new_pending);
        conn.next_serial.set(serial);
        Ok(plans)
    }

    fn decode(
        conn: read::ConnectionBorrow<'_>,
        p: read::Planned,
        specs: Vec<TagSpec>,
        response: Vec<u8>,
    ) -> Result<Vec<Result<Value, ItemError>>, FatalError> {
        let conn = conn.get::<SlmpConnection>();
        let phase = conn.phase.borrow();
        let Phase::Ready { model, count } = &*phase else {
            return Err(FatalError::Protocol(
                "configured range is unresolved".into(),
            ));
        };
        let serial_bytes: [u8; 2] = p
            .step
            .send
            .get(2..4)
            .ok_or_else(|| FatalError::Protocol("invalid plan".into()))?
            .try_into()
            .map_err(|_| FatalError::Protocol("invalid plan".into()))?;
        let route = Route {
            serial: u16::from_le_bytes(serial_bytes),
            ..conn.base_route
        };
        let pending = conn
            .pending
            .borrow()
            .get(&route.serial)
            .cloned()
            .ok_or_else(|| FatalError::Protocol("plan is not pending on this connection".into()))?;
        if p.covers != pending.covers || p.covers.is_empty() {
            return Err(FatalError::Protocol(
                "plan covers do not match pending read".into(),
            ));
        }
        let mut word_offset = 0u16;
        for cover in &p.covers {
            let spec = specs
                .get(*cover as usize)
                .ok_or_else(|| FatalError::Protocol("cover outside specs".into()))?;
            let tag = checked_read_span(spec, model, *count, conn.w_count.get())?;
            let expected_d = pending
                .span
                .start
                .checked_add(u32::from(word_offset))
                .ok_or_else(|| FatalError::Protocol("batch address overflow".into()))?;
            if tag.device != pending.span.device
                || tag.start != expected_d
                || (p.covers.len() > 1
                    && assess_d_span(
                        model,
                        if tag.device == Device::W {
                            conn.w_count.get()
                        } else {
                            *count
                        },
                        tag.start,
                        tag.count,
                    ) != RangeAssessment::VerifiedInside)
            {
                return Err(FatalError::Protocol("plan does not match specs".into()));
            }
            word_offset = word_offset
                .checked_add(tag.count)
                .ok_or_else(|| FatalError::Protocol("batch word count overflow".into()))?;
        }
        if word_offset != pending.span.count {
            return Err(FatalError::Protocol(
                "plan word count does not match specs".into(),
            ));
        }
        let series = known_series(model).expect("ready model has series");
        let expected = encode_read(route, series, pending.span)
            .map_err(|_| FatalError::Protocol("invalid D read span".into()))?;
        if p.step.send != expected
            || p.step.max_len != read_step(expected, pending.span.count).max_len
        {
            return Err(FatalError::Protocol("plan bytes do not match spec".into()));
        }
        conn.pending.borrow_mut().remove(&route.serial);
        match decode_read(route, pending.span, &response) {
            Ok(Values::Words(words)) => {
                let mut offset = 0usize;
                let mut results = Vec::with_capacity(p.covers.len());
                for cover in &p.covers {
                    let spec = &specs[*cover as usize];
                    let kind = DKind::parse(&spec.data_type, spec.string_words)
                        .map_err(FatalError::Protocol)?;
                    let end = offset + usize::from(kind.words());
                    let tag_words = words
                        .get(offset..end)
                        .ok_or_else(|| FatalError::Protocol("tag words exceed response".into()))?;
                    results.push(Ok(kind.decode(tag_words, conn.word_order)?));
                    offset = end;
                }
                if offset != words.len() {
                    return Err(FatalError::Protocol("response words exceed covers".into()));
                }
                Ok(results)
            }
            Ok(Values::Bits(_)) => Err(FatalError::Protocol("D read returned bits".into())),
            Err(ResponseError::EndCode(code)) => Ok((0..pending.covers.len())
                .map(|_| {
                    Err(ItemError::Device(banto::driver::types::DeviceError {
                        code: u32::from(code),
                        message: format!("SLMP end code 0x{code:04X}"),
                    }))
                })
                .collect()),
            Err(error) => Err(response_error(error)),
        }
    }
}

impl write_v1::Guest for Component {
    fn write_next(
        conn: write_v1::ConnectionBorrow<'_>,
        items: Vec<write_v1::WriteItem>,
        prev_response: Option<Vec<u8>>,
    ) -> Result<write_v1::WriteProgress, FatalError> {
        let conn = conn.get::<SlmpConnection>();
        if let Some(response) = prev_response {
            // Consume before parsing so a malformed or refused ACK cannot be retried.
            conn.write_poisoned.set(true);
            let pending = conn.pending_write.borrow_mut().take().ok_or_else(|| {
                FatalError::Protocol("write response without a pending request".into())
            })?;
            let (number, values) = parse_write_run(&items)
                .map_err(|_| FatalError::Protocol("write items changed after send".into()))?;
            if number != pending.number || values != pending.values {
                return Err(FatalError::Protocol("write item changed after send".into()));
            }
            let len = items.len();
            return match decode_write_ack(pending.route, &response) {
                Ok(()) => {
                    conn.write_poisoned.set(false);
                    Ok(write_v1::WriteProgress::Done(
                        (0..len).map(|_| Ok(())).collect(),
                    ))
                }
                Err(ResponseError::EndCode(code)) => {
                    conn.write_poisoned.set(false);
                    Ok(write_v1::WriteProgress::Done(
                        (0..len)
                            .map(|_| {
                                Err(ItemError::Device(banto::driver::types::DeviceError {
                                    code: u32::from(code),
                                    message: format!("SLMP end code 0x{code:04X}"),
                                }))
                            })
                            .collect(),
                    ))
                }
                Err(error) => Err(response_error(error)),
            };
        }
        if conn.write_poisoned.get() {
            return Err(FatalError::Protocol(
                "write outcome is uncertain; reconnect".into(),
            ));
        }
        if conn.pending_write.borrow().is_some() || !conn.pending.borrow().is_empty() {
            return Err(FatalError::Protocol(
                "a response is pending on this connection".into(),
            ));
        }
        if items.is_empty() {
            return Err(FatalError::InvalidConfig(
                "write requires at least one D/u16 item".into(),
            ));
        }
        let (model, count) = match &*conn.phase.borrow() {
            Phase::Ready { model, count } => (model.clone(), *count),
            _ => return Ok(rejected_write("discovery is incomplete")),
        };
        if model.family != ModelFamily::StandardR {
            return Ok(rejected_write_count(
                items.len(),
                "write requires a confirmed standard iQ-R CPU",
            ));
        }
        let (number, values) = match parse_write_run(&items) {
            Ok(run) => run,
            Err(message) => return Ok(rejected_write_count(items.len(), &message)),
        };
        if assess_d_span(&model, count, number, values.len() as u16)
            != RangeAssessment::VerifiedInside
        {
            return Ok(rejected_write_count(
                items.len(),
                "D span is not within the verified range",
            ));
        }
        let route = conn.route();
        let send = encode_r_d_words(route, number, &values)
            .map_err(|_| FatalError::Protocol("validated write span cannot be encoded".into()))?;
        *conn.pending_write.borrow_mut() = Some(PendingWrite {
            route,
            number,
            values,
        });
        Ok(write_v1::WriteProgress::Send(read::Planned {
            step: frame_step(send),
            covers: (0..items.len() as u32).collect(),
        }))
    }
}

fn valid_word(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0 && (0.0..=65535.0).contains(&value)
}

/// Validate the entire request before assigning a serial or producing a frame.
fn parse_write_run(items: &[write_v1::WriteItem]) -> Result<(u32, Vec<u16>), String> {
    if items.is_empty() || items.len() > 960 {
        return Err("write requires 1..=960 D/u16 words".into());
    }
    let mut start = None;
    let mut values = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let number = parse_d(
            &item.spec.address,
            &item.spec.data_type,
            item.spec.string_words,
        )?;
        let first = *start.get_or_insert(number);
        if first.checked_add(index as u32) != Some(number) {
            return Err("D write items must be consecutive and ascending".into());
        }
        let Value::Float(value) = item.value else {
            return Err("write value must be a finite integer in 0..65535".into());
        };
        if !valid_word(value) {
            return Err("write value must be a finite integer in 0..65535".into());
        }
        values.push(value as u16);
    }
    Ok((start.expect("nonempty request"), values))
}

fn rejected_write(message: &str) -> write_v1::WriteProgress {
    rejected_write_count(1, message)
}

fn rejected_write_count(len: usize, message: &str) -> write_v1::WriteProgress {
    write_v1::WriteProgress::Done(
        (0..len)
            .map(|_| Err(ItemError::Rejected(message.into())))
            .collect(),
    )
}

impl address::Guest for Component {
    fn parse(
        text: String,
        data_type: String,
        string_words: Option<u32>,
    ) -> Result<address::Parsed, String> {
        let (device, number, kind) = parse_read_address(&text, &data_type, string_words)?;
        Ok(address::Parsed {
            canonical: canonical_read(device, i64::from(number))?,
            area: if device == Device::W { "W" } else { "D" }.into(),
            // The existing write extension remains restricted to D/u16.
            // write-next also requires StandardR and a verified range.
            writable_area: device == Device::D && kind == DKind::U16,
            bit: None,
            word_count: u32::from(kind.words()),
            index: Some(u64::from(number)),
        })
    }

    fn offset(
        text: String,
        data_type: String,
        string_words: Option<u32>,
        n: i64,
    ) -> Result<String, String> {
        let (device, number, kind) = parse_read_address(&text, &data_type, string_words)?;
        let word_offset = n
            .checked_mul(i64::from(kind.words()))
            .ok_or("offset overflow")?;
        let next = i64::from(number)
            .checked_add(word_offset)
            .ok_or("offset overflow")?;
        let next = canonical_read(device, next)?;
        parse_read_address(&next, &data_type, string_words)?;
        Ok(next)
    }

    fn supported_kinds() -> Vec<String> {
        ["u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn offset_words(text: String, n: i64) -> Result<String, String> {
        let (device, number, _) = parse_read_address(&text, "u16", None)?;
        canonical_read(
            device,
            i64::from(number).checked_add(n).ok_or("offset overflow")?,
        )
    }

    fn help() -> String {
        "SLMP D decimal and standard iQ-R W hexadecimal numeric reads (1/2/4 words); range may remain unknown after handshake"
            .into()
    }

    fn probe() -> Option<TagSpec> {
        Some(TagSpec {
            address: "D0".into(),
            data_type: "u16".into(),
            string_words: None,
        })
    }
}

impl config::Guest for Component {
    fn validate(settings: Vec<config::Setting>) -> Vec<config::FieldError> {
        validate_settings(&settings)
    }
    fn secret_keys() -> Vec<String> {
        Vec::new()
    }
    fn schema() -> Vec<config::SettingField> {
        use config::{
            ChoiceOption, FieldGroup, FieldKind, IntegerSpec, SettingField, SettingValue,
        };
        vec![
            SettingField {
                key: "host".into(),
                label: "Host".into(),
                help: None,
                kind: FieldKind::Host,
                required: true,
                default: None,
                group: FieldGroup::Basic,
            },
            SettingField {
                key: "port".into(),
                label: "Port".into(),
                help: None,
                kind: FieldKind::Integer(IntegerSpec {
                    min: Some(1),
                    max: Some(65535),
                }),
                required: true,
                default: Some(SettingValue::Integer(5000)),
                group: FieldGroup::Basic,
            },
            SettingField {
                key: "word_order".into(),
                label: "Word order".into(),
                help: Some("16-bit word order within one numeric value".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "low_high".into(),
                        label: "Low then high".into(),
                    },
                    ChoiceOption {
                        value: "high_low".into(),
                        label: "High then low".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("low_high".into())),
                group: FieldGroup::Advanced,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(family: ModelFamily) -> TypeName {
        TypeName {
            raw_name: [b' '; 16],
            name: String::new(),
            code: 0,
            family,
        }
    }

    #[test]
    fn r_w_hex_address_is_read_only_and_checks_complete_tag() {
        let r = model(ModelFamily::StandardR);
        let spec = TagSpec {
            address: "W1E".into(),
            data_type: "u32".into(),
            string_words: None,
        };
        assert_eq!(
            checked_read_span(&spec, &r, PointCount::Unknown, PointCount::Exact(32)).unwrap(),
            ReadSpan {
                device: Device::W,
                start: 30,
                count: 2
            }
        );
        let outside = TagSpec {
            address: "W1F".into(),
            ..spec.clone()
        };
        assert!(
            checked_read_span(&outside, &r, PointCount::Unknown, PointCount::Exact(32)).is_err()
        );
        assert!(checked_read_span(&spec, &r, PointCount::Unknown, PointCount::Unknown).is_ok());
        assert!(checked_read_span(
            &spec,
            &model(ModelFamily::StandardQ),
            PointCount::Unknown,
            PointCount::Unknown
        )
        .is_err());
        assert!(parse_read_address("W1G", "u16", None).is_err());
        assert!(parse_read_address("WFFFFFFFF", "f64", None).is_err());
        let parsed =
            <Component as address::Guest>::parse("W0a".into(), "u16".into(), None).unwrap();
        assert_eq!(parsed.canonical, "WA");
        assert_eq!(parsed.area, "W");
        assert!(!parsed.writable_area);
        assert_eq!(
            <Component as address::Guest>::offset("WA".into(), "i32".into(), None, 1).unwrap(),
            "WC"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("WA".into(), 1).unwrap(),
            "WB"
        );
        assert!(parse_d("WA", "u16", None).is_err());
    }

    #[test]
    fn range_assessment_respects_scope_and_verified_prefix() {
        let q = model(ModelFamily::StandardQ);
        let r = model(ModelFamily::StandardR);
        assert_eq!(
            assess_d(&q, PointCount::Exact(100), 99),
            RangeAssessment::VerifiedInside
        );
        assert_eq!(
            assess_d(&q, PointCount::Exact(100), 100),
            RangeAssessment::Unknown
        );
        assert_eq!(
            assess_d(&r, PointCount::Exact(100), 100),
            RangeAssessment::VerifiedOutside
        );
        assert_eq!(
            assess_d(&q, PointCount::AtLeast(32768), 32767),
            RangeAssessment::VerifiedInside
        );
        assert_eq!(
            assess_d(&q, PointCount::AtLeast(32768), 32768),
            RangeAssessment::Unknown
        );
        assert_eq!(
            assess_d(&r, PointCount::Unknown, 0),
            RangeAssessment::Unknown
        );
        assert_eq!(
            assess_d(
                &model(ModelFamily::Q25PrhWireOnly),
                PointCount::Exact(100),
                0
            ),
            RangeAssessment::Unknown,
            "a Q25PRH SD count must not become a verified tag range"
        );
    }

    #[test]
    fn whole_tag_range_and_static_write_capability_are_distinct() {
        let q = model(ModelFamily::StandardQ);
        let r = model(ModelFamily::StandardR);
        assert_eq!(
            assess_d_span(&r, PointCount::Exact(100), 98, 2),
            RangeAssessment::VerifiedInside
        );
        assert_eq!(
            assess_d_span(&r, PointCount::Exact(100), 98, 4),
            RangeAssessment::VerifiedOutside
        );
        assert_eq!(
            assess_d_span(&q, PointCount::Exact(100), 98, 4),
            RangeAssessment::Unknown
        );
        assert_eq!(
            assess_d_span(&q, PointCount::AtLeast(32768), 32766, 2),
            RangeAssessment::VerifiedInside
        );
        assert_eq!(
            assess_d_span(&q, PointCount::AtLeast(32768), 32766, 4),
            RangeAssessment::Unknown
        );
        assert_eq!(
            parse_read_d("D4294967293", "f64", None),
            Err("D tag address overflows u32".into())
        );
        assert_eq!(parse_read_d("D12", "i32", None), Ok((12, DKind::I32)));
        assert!(
            parse_d("D12", "i32", None).is_err(),
            "write remains D/u16 only"
        );
        assert!(
            !<Component as address::Guest>::parse("D10".into(), "i32".into(), None)
                .unwrap()
                .writable_area
        );
        assert!(
            <Component as address::Guest>::parse("D10".into(), "u16".into(), None)
                .unwrap()
                .writable_area
        );
        assert_eq!(
            <Component as address::Guest>::offset("D10".into(), "f64".into(), None, 1).unwrap(),
            "D14"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("D10".into(), 1).unwrap(),
            "D11"
        );
    }

    #[test]
    fn numeric_d_codec_uses_full_word_order_and_preserves_nonfinite() {
        assert!(matches!(
            DKind::I16.decode(&[0x8000], WordOrder::LowHigh),
            Ok(Value::Int(-32768))
        ));
        assert!(matches!(
            DKind::U16.decode(&[0xffff], WordOrder::HighLow),
            Ok(Value::Uint(65535))
        ));
        assert!(matches!(
            DKind::I32.decode(&[0, 0x8000], WordOrder::LowHigh),
            Ok(Value::Int(-2147483648))
        ));
        assert!(matches!(
            DKind::U32.decode(&[0x8000, 0], WordOrder::HighLow),
            Ok(Value::Uint(0x8000_0000))
        ));
        assert!(matches!(
            DKind::I64.decode(&[0, 0, 0, 0x8000], WordOrder::LowHigh),
            Ok(Value::Int(i64::MIN))
        ));
        assert!(matches!(
            DKind::U64.decode(&[1, 0, 0, 0x20], WordOrder::LowHigh),
            Ok(Value::Uint(9_007_199_254_740_993))
        ));
        assert!(matches!(
            DKind::F32.decode(&[0, 0x3fc0], WordOrder::LowHigh),
            Ok(Value::Float(1.5))
        ));
        assert!(matches!(
            DKind::F64.decode(&[0x3ff8, 0, 0, 0], WordOrder::HighLow),
            Ok(Value::Float(1.5))
        ));
        assert!(
            matches!(DKind::F32.decode(&[0, 0x7fc0], WordOrder::LowHigh), Ok(Value::Float(v)) if v.is_nan())
        );
        assert!(DKind::F64.decode(&[0, 0], WordOrder::LowHigh).is_err());
    }

    #[test]
    fn word_order_setting_is_checked_without_changing_default() {
        let settings = [
            session::Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            session::Setting {
                key: "port".into(),
                value: "5000".into(),
            },
            session::Setting {
                key: "word_order".into(),
                value: "high_low".into(),
            },
        ];
        assert!(validate_settings(&settings).is_empty());
        let mut bad = settings.clone();
        bad[2].value = "middle".into();
        assert!(validate_settings(&bad)
            .iter()
            .any(|e| e.key == "word_order"));
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);
