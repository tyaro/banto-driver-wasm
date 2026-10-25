//! Binary 4E SLMP guest for a deliberately narrow single-word D read path.
//! The host owns transport. Each connection owns its discovery evidence.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_slmp_read_core::{
    basic_sd_count, decode_read, decode_type_name, encode_read, encode_type_name, plan_basic_sd,
    BasicArea, BasicEvidence, Device, ModelFamily, PointCount, ReadSpan, ResponseError, Route,
    Series, TypeName, Values,
};
use exports::banto::driver::{address, config, read, session};

wit_bindgen::generate!({ path: "../wit", world: "driver" });

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
    next_serial: Cell<u16>,
    phase: RefCell<Phase>,
    pending: RefCell<BTreeMap<u16, ReadSpan>>,
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
        if !matches!(item.key.as_str(), "host" | "port" | "transport") {
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
    errors
}

fn parse_d(text: &str, data_type: &str, words: Option<u32>) -> Result<u32, String> {
    if data_type != "u16" || words.is_some() {
        return Err("only single-word D u16 is supported".into());
    }
    let digits = text.strip_prefix('D').ok_or("D address required")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid D address".into());
    }
    digits
        .parse::<u32>()
        .map_err(|_| "D address exceeds u32".into())
}

fn canonical(number: i64) -> Result<String, String> {
    let number = u32::try_from(number).map_err(|_| "D address exceeds u32".to_string())?;
    Ok(format!("D{number}"))
}

fn known_series(model: &TypeName) -> Option<Series> {
    match model.family {
        ModelFamily::StandardQ => Some(Series::Q),
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

fn assess_d(model: &TypeName, count: PointCount, number: u32) -> RangeAssessment {
    match count {
        PointCount::Exact(limit) if number < limit => RangeAssessment::VerifiedInside,
        PointCount::AtLeast(prefix) if number < prefix => RangeAssessment::VerifiedInside,
        // R SD280-281 is the full D count. Q SD302 covers internal D only;
        // an address beyond it may belong to extended D.
        PointCount::Exact(limit) if model.family == ModelFamily::StandardR && number >= limit => {
            RangeAssessment::VerifiedOutside
        }
        _ => RangeAssessment::Unknown,
    }
}

fn checked_d_span(
    spec: &TagSpec,
    model: &TypeName,
    count: PointCount,
) -> Result<ReadSpan, FatalError> {
    let number = parse_d(&spec.address, &spec.data_type, spec.string_words)
        .map_err(FatalError::InvalidConfig)?;
    if assess_d(model, count, number) == RangeAssessment::VerifiedOutside {
        return Err(FatalError::InvalidConfig(
            "D address exceeds verified configured count".into(),
        ));
    }
    Ok(ReadSpan {
        device: Device::D,
        start: number,
        count: 1,
    })
}

impl session::GuestConnection for SlmpConnection {
    fn create(settings: Vec<session::Setting>) -> Result<session::Connection, FatalError> {
        if let Some(error) = validate_settings(&settings).first() {
            return Err(FatalError::InvalidConfig(error.message.clone()));
        }
        Ok(session::Connection::new(Self {
            base_route: Route::default(),
            next_serial: Cell::new(1),
            phase: RefCell::new(Phase::Start),
            pending: RefCell::new(BTreeMap::new()),
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
                let Some(area) = (match model.family {
                    ModelFamily::StandardQ => Some(BasicArea::DInternal),
                    ModelFamily::StandardR => Some(BasicArea::D),
                    ModelFamily::Unknown => None,
                }) else {
                    *self.phase.borrow_mut() = Phase::NoRange(Some(model));
                    return Ok(None);
                };
                let span = plan_basic_sd(&model, area).expect("known model has D count SD");
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
                let count = basic_sd_count(
                    &model,
                    area,
                    BasicEvidence {
                        route_confirmed: true,
                        r_firmware: None,
                    },
                    Some(&words),
                );
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
        if specs.len() != 1 {
            return Err(FatalError::InvalidConfig(
                "this SLMP stage reads exactly one D tag".into(),
            ));
        }
        let phase = conn.phase.borrow();
        let Phase::Ready { model, count } = &*phase else {
            return Err(FatalError::Protocol(
                "configured range is unresolved".into(),
            ));
        };
        let series = known_series(model).expect("ready model has series");
        let spans = specs
            .iter()
            .map(|spec| checked_d_span(spec, model, *count))
            .collect::<Result<Vec<_>, _>>()?;
        let mut plans = Vec::with_capacity(spans.len());
        for (index, span) in spans.into_iter().enumerate() {
            let route = conn.route();
            if conn.pending.borrow().contains_key(&route.serial) {
                return Err(FatalError::Protocol(
                    "SLMP serial reused while a read is pending".into(),
                ));
            }
            let send = encode_read(route, series, span)
                .map_err(|_| FatalError::Protocol("invalid D read span".into()))?;
            conn.pending.borrow_mut().insert(route.serial, span);
            plans.push(read::Planned {
                step: frame_step(send),
                covers: vec![index as u32],
            });
        }
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
        let [cover] = p.covers.as_slice() else {
            return Err(FatalError::Protocol("one cover required".into()));
        };
        let spec = specs
            .get(*cover as usize)
            .ok_or_else(|| FatalError::Protocol("cover outside specs".into()))?;
        let span = checked_d_span(spec, model, *count)?;
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
        let series = known_series(model).expect("ready model has series");
        let expected = encode_read(route, series, span)
            .map_err(|_| FatalError::Protocol("invalid D read span".into()))?;
        if p.step.send != expected {
            return Err(FatalError::Protocol("plan bytes do not match spec".into()));
        }
        if conn.pending.borrow_mut().remove(&route.serial) != Some(span) {
            return Err(FatalError::Protocol(
                "plan is not pending on this connection".into(),
            ));
        }
        match decode_read(route, span, &response) {
            Ok(Values::Words(words)) => {
                let [word] = words.as_slice() else {
                    return Err(FatalError::Framing("expected one D word".into()));
                };
                Ok(vec![Ok(Value::Uint(u64::from(*word)))])
            }
            Ok(Values::Bits(_)) => Err(FatalError::Protocol("D read returned bits".into())),
            Err(ResponseError::EndCode(code)) => Ok(vec![Err(ItemError::Device(
                banto::driver::types::DeviceError {
                    code: u32::from(code),
                    message: format!("SLMP end code 0x{code:04X}"),
                },
            ))]),
            Err(error) => Err(response_error(error)),
        }
    }
}

impl address::Guest for Component {
    fn parse(
        text: String,
        data_type: String,
        string_words: Option<u32>,
    ) -> Result<address::Parsed, String> {
        let number = parse_d(&text, &data_type, string_words)?;
        Ok(address::Parsed {
            canonical: format!("D{number}"),
            area: "D".into(),
            writable_area: false,
            bit: None,
            word_count: 1,
            index: Some(u64::from(number)),
        })
    }

    fn offset(
        text: String,
        data_type: String,
        string_words: Option<u32>,
        n: i64,
    ) -> Result<String, String> {
        let number = parse_d(&text, &data_type, string_words)?;
        canonical(i64::from(number).checked_add(n).ok_or("offset overflow")?)
    }

    fn supported_kinds() -> Vec<String> {
        vec!["u16".into()]
    }

    fn offset_words(text: String, n: i64) -> Result<String, String> {
        let number = parse_d(&text, "u16", None)?;
        canonical(i64::from(number).checked_add(n).ok_or("offset overflow")?)
    }

    fn help() -> String {
        "SLMP D decimal address, single-word u16 only; range may remain unknown after handshake"
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
        use config::{FieldGroup, FieldKind, IntegerSpec, SettingField, SettingValue};
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
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);
