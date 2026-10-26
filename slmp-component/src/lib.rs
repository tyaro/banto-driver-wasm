//! Binary 4E SLMP guest for numeric D/W reads and restricted common-policy D/W u16 writes.
//! The host owns transport. Each connection owns its discovery evidence.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_register_codec::{
    decode_f32, decode_f64, decode_i16, decode_i32, decode_i64, decode_u32, decode_u64, WordOrder,
};
use banto_slmp_read_core::{
    basic_sd_count, decode_read, decode_type_name, decode_write_ack,
    encode_q25prh_d3010_3013_test_write, encode_q_dw_words, encode_r_dw_words, encode_read,
    encode_type_name, plan_basic_sd, BasicArea, BasicEvidence, Device, ModelFamily, PointCount,
    ReadSpan, ResponseError, Route, Series, TypeName, Values,
};
use exports::banto::driver::{address, config, read, session, write_v1};
mod write_policy;
use write_policy::{WritePolicy, ALLOWLIST_KEY, MODE_KEY};

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
    q25prh_test_write_enabled: bool,
    write_policy: WritePolicy,
    q25prh_internal_d_count: Cell<PointCount>,
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
    device: Device,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    Bit,
    WordBit(u8),
    Numeric(DKind),
    LongTimer,
    /// Third response word: b0 coil, b1 contact (MC manual p.87).
    LongTimerFlag(u8),
    LongCounter,
}

impl ReadKind {
    const fn points(self) -> u16 {
        match self {
            Self::Bit => 1,
            Self::WordBit(_) => 1,
            Self::Numeric(kind) => kind.words(),
            Self::LongTimer | Self::LongTimerFlag(_) => 4,
            Self::LongCounter => 2,
        }
    }

    // WIT metadata describes the typed value, not every word carried by a
    // long timer response (which also includes status and system flags).
    const fn value_words(self) -> u32 {
        match self {
            Self::Bit | Self::WordBit(_) | Self::LongTimerFlag(_) => 1,
            Self::Numeric(kind) => kind.words() as u32,
            Self::LongTimer | Self::LongCounter => 2,
        }
    }

    fn decode_words(self, words: &[u16], order: WordOrder) -> Result<Value, FatalError> {
        match self {
            Self::WordBit(bit) if words.len() == 1 => Ok(Value::Bool(words[0] & (1 << bit) != 0)),
            Self::Numeric(kind) => kind.decode(words, order),
            // MC batch read returns four words per long timer: two current-value
            // words, then coil/contact and system flags. These are not four
            // independent timer values.
            Self::LongTimer if words.len() == 4 => Ok(Value::Int(i64::from(decode_i32(
                [words[0], words[1]],
                WordOrder::LowHigh,
            )))),
            Self::LongTimerFlag(bit) if words.len() == 4 => {
                Ok(Value::Bool(words[2] & (1 << bit) != 0))
            }
            Self::LongCounter if words.len() == 2 => Ok(Value::Int(i64::from(decode_i32(
                [words[0], words[1]],
                WordOrder::LowHigh,
            )))),
            _ => Err(FatalError::Protocol(
                "tag word count does not match plan".into(),
            )),
        }
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

fn read_step(send: Vec<u8>, span: ReadSpan) -> session::Step {
    let mut step = frame_step(send);
    // Binary 4E bit responses pack two points per byte; words use two bytes.
    // Keep the previous allowance for short PLC refusal frames.
    let body_len = if span.device.is_bit() {
        u32::from(span.count).div_ceil(2)
    } else {
        u32::from(span.count) * 2
    };
    step.max_len = (15 + body_len).max(64);
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
            "host"
                | "port"
                | "transport"
                | "word_order"
                | "q25prh_d3010_3013_test_write"
                | MODE_KEY
                | ALLOWLIST_KEY
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
    if setting(settings, "q25prh_d3010_3013_test_write")
        .is_some_and(|value| !matches!(value, "disabled" | "enabled"))
    {
        errors.push(config::FieldError {
            key: "q25prh_d3010_3013_test_write".into(),
            message: "Q25PRH test write must be disabled or explicitly enabled".into(),
        });
    }
    if let Err((key, message)) = WritePolicy::parse(
        setting(settings, MODE_KEY),
        setting(settings, ALLOWLIST_KEY),
    ) {
        errors.push(config::FieldError {
            key: key.into(),
            message: message.into(),
        });
    }
    if setting(settings, MODE_KEY) == Some("enabled")
        && setting(settings, "q25prh_d3010_3013_test_write") == Some("enabled")
    {
        errors.push(config::FieldError {
            key: MODE_KEY.into(),
            message: "normal SLMP write mode and Q25PRH test mode cannot both be enabled".into(),
        });
    }
    errors
}

#[cfg(test)]
fn parse_d(text: &str, data_type: &str, words: Option<u32>) -> Result<u32, String> {
    if data_type != "u16" || words.is_some() {
        return Err("only single-word D u16 is supported".into());
    }
    parse_d_number(text)
}

fn parse_dw_write(
    text: &str,
    data_type: &str,
    words: Option<u32>,
) -> Result<(Device, u32), String> {
    let (device, number, kind) = parse_read_address(text, data_type, words)?;
    if !matches!(device, Device::D | Device::W) || kind != ReadKind::Numeric(DKind::U16) {
        return Err("write requires a D or W u16 word tag".into());
    }
    Ok((device, number))
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
) -> Result<(Device, u32, ReadKind), String> {
    if let Some((word, bit)) = text.strip_prefix('D').and_then(|rest| rest.split_once('.')) {
        if data_type != "bit" || words.is_some() {
            return Err("D word-bit requires bit type without string_words".into());
        }
        if word.is_empty() || !word.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid D word-bit address".into());
        }
        if bit.len() != 1 || !bit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("word bit must be one hexadecimal digit".into());
        }
        let number = word.parse::<u32>().map_err(|_| "D address exceeds u32")?;
        let bit = u8::from_str_radix(bit, 16).map_err(|_| "invalid word bit")?;
        return Ok((Device::D, number, ReadKind::WordBit(bit)));
    }
    // LTS/LTC/LSTS/LSTC cannot head a batch bit read. Read their corresponding
    // current-value code in word units and take the contact/coil flag from
    // response word three. One logical timer still consumes four wire words.
    for (prefix, device, bit) in [
        ("LSTS", Device::LSTS, 1),
        ("LSTC", Device::LSTC, 0),
        ("LTS", Device::LTS, 1),
        ("LTC", Device::LTC, 0),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if data_type != "bit" || words.is_some() {
                return Err("long timer contact/coil requires bit without string_words".into());
            }
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid long timer address".into());
            }
            let number = digits
                .parse::<u32>()
                .map_err(|_| "long timer address exceeds u32")?;
            return Ok((device, number, ReadKind::LongTimerFlag(bit)));
        }
    }
    // Long timer current values use four response words, including flags;
    // long counter current values use two.
    for (prefix, device, kind) in [
        ("LSTN", Device::LSTN, ReadKind::LongTimer),
        ("LTN", Device::LTN, ReadKind::LongTimer),
        ("LCN", Device::LCN, ReadKind::LongCounter),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if data_type != "i32" || words.is_some() {
                return Err("long current value requires i32 without string_words".into());
            }
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid long device address".into());
            }
            let number = digits
                .parse::<u32>()
                .map_err(|_| "long device address exceeds u32")?;
            return Ok((device, number, kind));
        }
    }
    // Multi-letter prefixes precede their single-letter names.
    let bit_prefixes = [
        ("LCS", Device::LCS, 10),
        ("LCC", Device::LCC, 10),
        ("STS", Device::STS, 10),
        ("STC", Device::STC, 10),
        ("TS", Device::TS, 10),
        ("TC", Device::TC, 10),
        ("SS", Device::SS, 10),
        ("SC", Device::SC, 10),
        ("CS", Device::CS, 10),
        ("CC", Device::CC, 10),
        ("SB", Device::SB, 16),
        ("SM", Device::SM, 10),
        ("M", Device::M, 10),
        ("X", Device::X, 16),
        ("Y", Device::Y, 16),
        ("B", Device::B, 16),
        ("L", Device::L, 10),
        ("F", Device::F, 10),
        ("V", Device::V, 10),
    ];
    if let Some((device, radix, digits)) =
        bit_prefixes.iter().find_map(|(prefix, device, radix)| {
            text.strip_prefix(prefix)
                .map(|digits| (*device, *radix, digits))
        })
    {
        if data_type != "bit" || words.is_some() {
            return Err("bit devices require bit type without string_words".into());
        }
        if digits.is_empty()
            || !digits.bytes().all(|b| {
                if radix == 10 {
                    b.is_ascii_digit()
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err("invalid bit device address".into());
        }
        let number =
            u32::from_str_radix(digits, radix).map_err(|_| "bit device address exceeds u32")?;
        return Ok((device, number, ReadKind::Bit));
    }
    let kind = DKind::parse(data_type, words)?;
    for (prefix, device) in [
        ("STN", Device::STN),
        ("TN", Device::TN),
        ("SN", Device::SN),
        ("CN", Device::CN),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if kind.words() != 1 {
                return Err("timer/counter current value is one word".into());
            }
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid timer/counter address".into());
            }
            let number = digits
                .parse::<u32>()
                .map_err(|_| "timer/counter address exceeds u32")?;
            return Ok((device, number, ReadKind::Numeric(kind)));
        }
    }
    if text.starts_with('D') {
        let (number, kind) = parse_read_d(text, data_type, words)?;
        return Ok((Device::D, number, ReadKind::Numeric(kind)));
    }
    let (device, radix, digits) = if let Some(digits) = text.strip_prefix("SW") {
        (Device::SW, 16, digits)
    } else if let Some(digits) = text.strip_prefix("SD") {
        (Device::SD, 10, digits)
    } else if let Some(digits) = text.strip_prefix('W') {
        (Device::W, 16, digits)
    } else if let Some(digits) = text.strip_prefix("ZR") {
        (Device::ZR, 16, digits)
    } else {
        return Err("unsupported word device address".into());
    };
    if digits.is_empty()
        || !digits.bytes().all(|b| {
            if radix == 16 {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err("invalid word device address".into());
    }
    let number =
        u32::from_str_radix(digits, radix).map_err(|_| "word device address exceeds u32")?;
    number
        .checked_add(u32::from(kind.words() - 1))
        .ok_or("word tag address overflows u32")?;
    Ok((device, number, ReadKind::Numeric(kind)))
}

fn canonical_read(device: Device, number: i64) -> Result<String, String> {
    let number = u32::try_from(number).map_err(|_| "read address exceeds u32")?;
    match device {
        Device::M => Ok(format!("M{number}")),
        Device::X => Ok(format!("X{number:X}")),
        Device::Y => Ok(format!("Y{number:X}")),
        Device::B => Ok(format!("B{number:X}")),
        Device::L => Ok(format!("L{number}")),
        Device::F => Ok(format!("F{number}")),
        Device::V => Ok(format!("V{number}")),
        Device::SB => Ok(format!("SB{number:X}")),
        Device::SM => Ok(format!("SM{number}")),
        Device::D => Ok(format!("D{number}")),
        Device::W => Ok(format!("W{number:X}")),
        Device::SW => Ok(format!("SW{number:X}")),
        Device::SD => Ok(format!("SD{number}")),
        Device::TS => Ok(format!("TS{number}")),
        Device::TC => Ok(format!("TC{number}")),
        Device::TN => Ok(format!("TN{number}")),
        Device::SS => Ok(format!("SS{number}")),
        Device::SC => Ok(format!("SC{number}")),
        Device::SN => Ok(format!("SN{number}")),
        Device::STS => Ok(format!("STS{number}")),
        Device::STC => Ok(format!("STC{number}")),
        Device::STN => Ok(format!("STN{number}")),
        Device::CS => Ok(format!("CS{number}")),
        Device::CC => Ok(format!("CC{number}")),
        Device::CN => Ok(format!("CN{number}")),
        Device::LTS => Ok(format!("LTS{number}")),
        Device::LTC => Ok(format!("LTC{number}")),
        Device::LSTS => Ok(format!("LSTS{number}")),
        Device::LSTC => Ok(format!("LSTC{number}")),
        Device::LTN => Ok(format!("LTN{number}")),
        Device::LSTN => Ok(format!("LSTN{number}")),
        Device::LCN => Ok(format!("LCN{number}")),
        Device::LCS => Ok(format!("LCS{number}")),
        Device::LCC => Ok(format!("LCC{number}")),
        Device::ZR => Ok(format!("ZR{number:X}")),
    }
}

fn canonical_read_tag(device: Device, number: i64, kind: ReadKind) -> Result<String, String> {
    let base = canonical_read(device, number)?;
    match kind {
        ReadKind::WordBit(bit) => Ok(format!("{base}.{bit:X}")),
        _ => Ok(base),
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

fn count_guarantees_span(count: PointCount, number: u32, words: u16) -> bool {
    let Some(end_exclusive) = number.checked_add(u32::from(words)) else {
        return false;
    };
    matches!(count, PointCount::Exact(limit) | PointCount::AtLeast(limit) if end_exclusive <= limit)
}

fn read_count(device: Device, d_count: PointCount, w_count: PointCount) -> PointCount {
    match device {
        Device::D => d_count,
        Device::W => w_count,
        // This handshake obtains D/W counts only. Do not infer M/X/Y/B
        // configuration from a CPU family, Q25PRH SD302, or the wire limit.
        Device::M
        | Device::X
        | Device::Y
        | Device::B
        | Device::L
        | Device::F
        | Device::V
        | Device::SB
        | Device::SM
        | Device::SW
        | Device::SD
        | Device::TS
        | Device::TC
        | Device::TN
        | Device::SS
        | Device::SC
        | Device::SN
        | Device::STS
        | Device::STC
        | Device::STN
        | Device::CS
        | Device::CC
        | Device::CN
        | Device::LTS
        | Device::LTC
        | Device::LSTS
        | Device::LSTC
        | Device::LTN
        | Device::LSTN
        | Device::LCN
        | Device::LCS
        | Device::LCC
        | Device::ZR => PointCount::Unknown,
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
    // Q25PRH has only its separately checked internal-D map. The standard
    // timer/counter and file-register maps must not be borrowed for it.
    if !matches!(
        device,
        Device::D
            | Device::W
            | Device::SW
            | Device::SD
            | Device::M
            | Device::X
            | Device::Y
            | Device::B
            | Device::L
            | Device::F
            | Device::V
            | Device::SB
            | Device::SM
    ) && (model.family == ModelFamily::Q25PrhWireOnly
        || (matches!(
            device,
            Device::LTS
                | Device::LTC
                | Device::LSTS
                | Device::LSTC
                | Device::LTN
                | Device::LSTN
                | Device::LCN
                | Device::LCS
                | Device::LCC
                | Device::ZR
        ) && model.family != ModelFamily::StandardR))
    {
        return Err(FatalError::InvalidConfig(
            "device is unsupported for this CPU family".into(),
        ));
    }
    if (matches!(device, Device::SS | Device::SC | Device::SN)
        && model.family != ModelFamily::StandardQ)
        || (matches!(device, Device::STS | Device::STC | Device::STN)
            && model.family != ModelFamily::StandardR)
    {
        return Err(FatalError::InvalidConfig(
            "retentive timer name does not match CPU series".into(),
        ));
    }
    if device == Device::W {
        match model.family {
            ModelFamily::StandardR => {}
            ModelFamily::Q25PrhWireOnly
                if count_guarantees_span(w_count, number, kind.points()) => {}
            ModelFamily::StandardQ
                if assess_d_span(model, w_count, number, kind.points())
                    == RangeAssessment::VerifiedInside => {}
            _ => {
                return Err(FatalError::InvalidConfig(
                    "W reads require a confirmed iQ-R count or a verified standard Q internal W span"
                        .into(),
                ));
            }
        }
    }
    let count = read_count(device, d_count, w_count);
    if assess_d_span(model, count, number, kind.points()) == RangeAssessment::VerifiedOutside {
        return Err(FatalError::InvalidConfig(
            "address exceeds verified configured count".into(),
        ));
    }
    Ok(ReadSpan {
        device,
        start: number,
        count: kind.points(),
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
            q25prh_test_write_enabled: setting(&settings, "q25prh_d3010_3013_test_write")
                == Some("enabled"),
            write_policy: WritePolicy::parse(
                setting(&settings, MODE_KEY),
                setting(&settings, ALLOWLIST_KEY),
            )
            .expect("settings validated above"),
            q25prh_internal_d_count: Cell::new(PointCount::Unknown),
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
                    ModelFamily::Q25PrhWireOnly => BasicArea::DInternal,
                    ModelFamily::Unknown => {
                        *self.phase.borrow_mut() = Phase::NoRange(Some(model));
                        return Ok(None);
                    }
                };
                let mut span = plan_basic_sd(&model, area).expect("known model has D count SD");
                // Read each family's adjacent D/W count fields in one snapshot.
                if model.family == ModelFamily::StandardR {
                    span.count = 4; // SD280-283: 32-bit D and W counts.
                } else if matches!(
                    model.family,
                    ModelFamily::StandardQ | ModelFamily::Q25PrhWireOnly
                ) {
                    span.count = 2; // SD302-303: 16-bit internal D and W counts.
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
                let area = if matches!(
                    model.family,
                    ModelFamily::StandardQ | ModelFamily::Q25PrhWireOnly
                ) {
                    BasicArea::DInternal
                } else {
                    BasicArea::D
                };
                let evidence = BasicEvidence {
                    route_confirmed: true,
                    r_firmware: None,
                };
                let count = match model.family {
                    ModelFamily::StandardR => {
                        self.w_count.set(basic_sd_count(
                            &model,
                            BasicArea::W,
                            evidence,
                            words.get(2..4),
                        ));
                        basic_sd_count(&model, area, evidence, words.get(0..2))
                    }
                    ModelFamily::StandardQ => {
                        self.w_count.set(basic_sd_count(
                            &model,
                            BasicArea::WInternal,
                            evidence,
                            words.get(1..2),
                        ));
                        basic_sd_count(&model, area, evidence, words.get(0..1))
                    }
                    ModelFamily::Q25PrhWireOnly => {
                        self.w_count.set(basic_sd_count(
                            &model,
                            BasicArea::WInternal,
                            evidence,
                            words.get(1..2),
                        ));
                        basic_sd_count(&model, area, evidence, words.get(0..1))
                    }
                    ModelFamily::Unknown => unreachable!("unknown model does not plan SD"),
                };
                if model.family == ModelFamily::Q25PrhWireOnly {
                    // Preserve the existing unknown-range read policy. This
                    // separately obtained internal prefix gates only the
                    // explicit four-word test write on this connection.
                    self.q25prh_internal_d_count.set(count);
                    *self.phase.borrow_mut() = Phase::Ready {
                        model,
                        count: PointCount::Unknown,
                    };
                } else {
                    *self.phase.borrow_mut() = Phase::Ready { model, count };
                }
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
                        read_count(span.device, *count, conn.w_count.get()),
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
                            read_count(span.device, *count, conn.w_count.get()),
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
            let step = read_step(send, group.span);
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
                        read_count(tag.device, *count, conn.w_count.get()),
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
        if p.step.send != expected || p.step.max_len != read_step(expected, pending.span).max_len {
            return Err(FatalError::Protocol("plan bytes do not match spec".into()));
        }
        conn.pending.borrow_mut().remove(&route.serial);
        match decode_read(route, pending.span, &response) {
            Ok(Values::Words(words)) if !pending.span.device.is_bit() => {
                let mut offset = 0usize;
                let mut results = Vec::with_capacity(p.covers.len());
                for cover in &p.covers {
                    let spec = &specs[*cover as usize];
                    let (_, _, kind) =
                        parse_read_address(&spec.address, &spec.data_type, spec.string_words)
                            .map_err(FatalError::Protocol)?;
                    let end = offset + usize::from(kind.points());
                    let tag_words = words
                        .get(offset..end)
                        .ok_or_else(|| FatalError::Protocol("tag words exceed response".into()))?;
                    results.push(Ok(kind.decode_words(tag_words, conn.word_order)?));
                    offset = end;
                }
                if offset != words.len() {
                    return Err(FatalError::Protocol("response words exceed covers".into()));
                }
                Ok(results)
            }
            Ok(Values::Bits(bits)) if pending.span.device.is_bit() => {
                if bits.len() != p.covers.len() {
                    return Err(FatalError::Protocol(
                        "bit response does not match covers".into(),
                    ));
                }
                Ok(bits.into_iter().map(Value::Bool).map(Ok).collect())
            }
            Ok(_) => Err(FatalError::Protocol(
                "read response has wrong device unit".into(),
            )),
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
            let (device, number, values) = parse_write_run(&items)
                .map_err(|_| FatalError::Protocol("write items changed after send".into()))?;
            if device != pending.device || number != pending.number || values != pending.values {
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
                "write requires at least one D or W u16 item".into(),
            ));
        }
        let (model, count) = match &*conn.phase.borrow() {
            Phase::Ready { model, count } => (model.clone(), *count),
            _ => return Ok(rejected_write_count(items.len(), "discovery is incomplete")),
        };
        let q25prh = model.family == ModelFamily::Q25PrhWireOnly
            && model.name == "Q25PRHCPU"
            && model.code == 0x004c;
        let q25prh_test = q25prh && conn.q25prh_test_write_enabled;
        if !matches!(
            model.family,
            ModelFamily::StandardR | ModelFamily::StandardQ
        ) && !q25prh
        {
            return Ok(rejected_write_count(
                items.len(),
                "write requires a confirmed Q/R model",
            ));
        }
        let (device, number, values) = match parse_write_run(&items) {
            Ok(run) => run,
            Err(message) => return Ok(rejected_write_count(items.len(), &message)),
        };
        if q25prh_test && (device != Device::D || number != 3010 || values.len() != 4) {
            return Ok(rejected_write_count(
                items.len(),
                "Q25PRH test write requires exactly D3010..D3013/u16",
            ));
        }
        if q25prh_test && !count_guarantees_span(conn.q25prh_internal_d_count.get(), number, 4) {
            return Ok(rejected_write_count(
                items.len(),
                "Q25PRH SD302 does not confirm the full D3010..D3013 test span",
            ));
        }
        if !q25prh_test
            && !conn
                .write_policy
                .allows_device_span(device, number, values.len())
        {
            return Ok(rejected_write_count(
                items.len(),
                "device span is outside the enabled connection allowlist",
            ));
        }
        if !q25prh_test {
            let proven = if model.family == ModelFamily::StandardR {
                let area_count = if device == Device::D {
                    count
                } else {
                    conn.w_count.get()
                };
                assess_d_span(&model, area_count, number, values.len() as u16)
                    == RangeAssessment::VerifiedInside
            } else if model.family == ModelFamily::StandardQ && device == Device::W {
                count_guarantees_span(conn.w_count.get(), number, values.len() as u16)
            } else if q25prh && device == Device::D {
                count_guarantees_span(
                    conn.q25prh_internal_d_count.get(),
                    number,
                    values.len() as u16,
                )
            } else {
                false
            };
            if !proven {
                return Ok(rejected_write_count(
                    items.len(),
                    "device span is not within the same-connection SD range",
                ));
            }
        }
        let route = conn.route();
        let send = if q25prh_test {
            // This opt-in never upgrades Q's unknown configured D count.
            let words: [u16; 4] = values.as_slice().try_into().expect("checked Q test width");
            encode_q25prh_d3010_3013_test_write(route, words)
        } else if model.family == ModelFamily::StandardR {
            encode_r_dw_words(route, device, number, &values).map_err(|_| {
                FatalError::Protocol("validated write span cannot be encoded".into())
            })?
        } else {
            encode_q_dw_words(route, device, number, &values).map_err(|_| {
                FatalError::Protocol("validated Q write span cannot be encoded".into())
            })?
        };
        *conn.pending_write.borrow_mut() = Some(PendingWrite {
            route,
            device,
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
fn parse_write_run(items: &[write_v1::WriteItem]) -> Result<(Device, u32, Vec<u16>), String> {
    if items.is_empty() || items.len() > 960 {
        return Err("write requires 1..=960 consecutive D or W u16 words".into());
    }
    let mut start = None;
    let mut area = None;
    let mut values = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let (device, number) = parse_dw_write(
            &item.spec.address,
            &item.spec.data_type,
            item.spec.string_words,
        )?;
        if area.get_or_insert(device) != &device {
            return Err("write items must be in one device area".into());
        }
        let first = *start.get_or_insert(number);
        if first.checked_add(index as u32) != Some(number) {
            return Err("write items must be consecutive and ascending".into());
        }
        let Value::Float(value) = item.value else {
            return Err("write value must be a finite integer in 0..65535".into());
        };
        if !valid_word(value) {
            return Err("write value must be a finite integer in 0..65535".into());
        }
        values.push(value as u16);
    }
    Ok((
        area.expect("nonempty request"),
        start.expect("nonempty request"),
        values,
    ))
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
            canonical: canonical_read_tag(device, i64::from(number), kind)?,
            area: match device {
                Device::M => "M",
                Device::X => "X",
                Device::Y => "Y",
                Device::B => "B",
                Device::L => "L",
                Device::F => "F",
                Device::V => "V",
                Device::SB => "SB",
                Device::SM => "SM",
                Device::D => "D",
                Device::W => "W",
                Device::SW => "SW",
                Device::SD => "SD",
                Device::TS => "TS",
                Device::TC => "TC",
                Device::TN => "TN",
                Device::SS => "SS",
                Device::SC => "SC",
                Device::SN => "SN",
                Device::STS => "STS",
                Device::STC => "STC",
                Device::STN => "STN",
                Device::CS => "CS",
                Device::CC => "CC",
                Device::CN => "CN",
                Device::LTS => "LTS",
                Device::LTC => "LTC",
                Device::LSTS => "LSTS",
                Device::LSTC => "LSTC",
                Device::LTN => "LTN",
                Device::LSTN => "LSTN",
                Device::LCN => "LCN",
                Device::LCS => "LCS",
                Device::LCC => "LCC",
                Device::ZR => "ZR",
            }
            .into(),
            // Static capability only; the common setting, SD proof and Host gate still decide writes.
            // write-next also requires StandardR and a verified range.
            writable_area: matches!(device, Device::D | Device::W)
                && kind == ReadKind::Numeric(DKind::U16),
            bit: if let ReadKind::WordBit(bit) = kind {
                Some(bit)
            } else {
                None
            },
            word_count: kind.value_words(),
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
        let next = if let ReadKind::WordBit(bit) = kind {
            let start = i64::from(number)
                .checked_mul(16)
                .and_then(|v| v.checked_add(i64::from(bit)))
                .ok_or("offset overflow")?;
            let next_bit = start.checked_add(n).ok_or("offset overflow")?;
            if next_bit < 0 {
                return Err("offset before D0.0".into());
            }
            canonical_read_tag(
                device,
                next_bit / 16,
                ReadKind::WordBit((next_bit % 16) as u8),
            )?
        } else {
            let width = match kind {
                ReadKind::Numeric(kind) => i64::from(kind.words()),
                ReadKind::Bit
                | ReadKind::LongTimer
                | ReadKind::LongTimerFlag(_)
                | ReadKind::LongCounter => 1,
                ReadKind::WordBit(_) => unreachable!(),
            };
            let point_offset = n.checked_mul(width).ok_or("offset overflow")?;
            let next = i64::from(number)
                .checked_add(point_offset)
                .ok_or("offset overflow")?;
            canonical_read_tag(device, next, kind)?
        };
        parse_read_address(&next, &data_type, string_words)?;
        Ok(next)
    }

    fn supported_kinds() -> Vec<String> {
        [
            "bit", "u16", "i16", "u32", "i32", "u64", "i64", "f32", "f64",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn offset_words(text: String, n: i64) -> Result<String, String> {
        if ["LTS", "LTC", "LSTS", "LSTC", "LTN", "LSTN", "LCN"]
            .iter()
            .any(|prefix| text.starts_with(prefix))
        {
            return Err("offset_words is undefined for long current values".into());
        }
        let kind = if text.starts_with('D') && text.contains('.')
            || [
                "STS", "STC", "TS", "TC", "SS", "SC", "CS", "CC", "M", "X", "Y", "B", "L", "F",
                "V", "SB", "SM",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix))
                && !text.starts_with("SW")
        {
            "bit"
        } else {
            "u16"
        };
        let (device, number, parsed_kind) = parse_read_address(&text, kind, None)?;
        canonical_read_tag(
            device,
            i64::from(number).checked_add(n).ok_or("offset overflow")?,
            parsed_kind,
        )
    }

    fn help() -> String {
        "SLMP Q/R timer/counter contacts and coils are bits; TN/SN/CN are one-word current values. Standard iQ-R adds LCS/LCC bit contacts/coils, LTN/LSTN/LCN i32 current values, LTS/LTC/LSTS/LSTC flags through four-word current-value reads, and hexadecimal ZR; block-switched R is not supported. Dn.h reads one hexadecimal-indexed bit of Dn. New area ranges remain unknown and use one request per tag. Local write support is D/W u16 only and still requires a matching Host gate; Q25PRH W remains read-only"
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
            ChoiceOption, FieldGroup, FieldKind, IntegerSpec, SettingField, SettingValue, TextSpec,
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
            SettingField {
                key: "q25prh_d3010_3013_test_write".into(),
                label: "Q25PRH D3010-D3013 test write".into(),
                help: Some("Test only. Requires exact Q25PRHCPU / 004CH discovery, same-connection SD302 proof of the full four-word span, and a dedicated Host write gate. Does not enable production writes.".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "disabled".into(),
                        label: "Disabled".into(),
                    },
                    ChoiceOption {
                        value: "enabled".into(),
                        label: "Enabled for controlled test only".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("disabled".into())),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: MODE_KEY.into(),
                label: "SLMP write mode".into(),
                help: Some("D/W u16 writes require one explicit connection allowlist, same-connection area SD proof, and a separate Host write gate. Default is disabled. Other device writes remain unsupported.".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "disabled".into(),
                        label: "Disabled".into(),
                    },
                    ChoiceOption {
                        value: "enabled".into(),
                        label: "Enabled for this connection".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("disabled".into())),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: ALLOWLIST_KEY.into(),
                label: "SLMP writable ranges".into(),
                help: Some("Comma-separated nonoverlapping D decimal and W hexadecimal ranges, such as D3010-D3013,W1A-W1F. Up to 16 ranges of 1..960 words; D3000 and D3001 are protected. The saved setting keys remain compatible.".into()),
                kind: FieldKind::Text(TextSpec {
                    max_len: Some(256),
                    placeholder: Some("D3010-D3013,W1A-W1F".into()),
                }),
                required: false,
                default: Some(SettingValue::Text(String::new())),
                group: FieldGroup::Advanced,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_write_run_checks_area_order_and_complete_values() {
        let item = |address: &str, value: f64| write_v1::WriteItem {
            spec: TagSpec {
                address: address.into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Float(value),
        };
        assert_eq!(
            parse_write_run(&[item("W1A", 1.0), item("W1B", 65535.0)]),
            Ok((Device::W, 0x1a, vec![1, 65535]))
        );
        for run in [
            vec![item("W1A", 1.0), item("D27", 2.0)],
            vec![item("W1B", 1.0), item("W1A", 2.0)],
            vec![item("W1A", 1.0), item("W1C", 2.0)],
            vec![item("W1A", f64::NAN)],
            vec![item("W1A", 65536.0)],
        ] {
            assert!(parse_write_run(&run).is_err());
        }
    }

    fn model(family: ModelFamily) -> TypeName {
        TypeName {
            raw_name: [b' '; 16],
            name: String::new(),
            code: 0,
            family,
        }
    }

    #[test]
    fn timer_file_and_word_bit_metadata_and_restrictions() {
        let q = model(ModelFamily::StandardQ);
        let r = model(ModelFamily::StandardR);
        let q25 = model(ModelFamily::Q25PrhWireOnly);
        for (text, kind, device, number, points) in [
            ("TS12", "bit", Device::TS, 12, 1),
            ("TC12", "bit", Device::TC, 12, 1),
            ("TN12", "u16", Device::TN, 12, 1),
            ("SS4", "bit", Device::SS, 4, 1),
            ("SC4", "bit", Device::SC, 4, 1),
            ("SN4", "i16", Device::SN, 4, 1),
            ("CS3", "bit", Device::CS, 3, 1),
            ("CC3", "bit", Device::CC, 3, 1),
            ("CN3", "u16", Device::CN, 3, 1),
            ("D10.F", "bit", Device::D, 10, 1),
        ] {
            let spec = TagSpec {
                address: text.into(),
                data_type: kind.into(),
                string_words: None,
            };
            let span =
                checked_read_span(&spec, &q, PointCount::Exact(32), PointCount::Unknown).unwrap();
            assert_eq!(
                (span.device, span.start, span.count),
                (device, number, points)
            );
            assert!(
                !<Component as address::Guest>::parse(text.into(), kind.into(), None)
                    .unwrap()
                    .writable_area
            );
            if device != Device::D {
                assert!(
                    checked_read_span(&spec, &q25, PointCount::Unknown, PointCount::Unknown)
                        .is_err()
                );
            }
        }
        for (text, device, count) in [
            ("LTN4", Device::LTN, 4),
            ("LSTN4", Device::LSTN, 4),
            ("LCN4", Device::LCN, 2),
        ] {
            let spec = TagSpec {
                address: text.into(),
                data_type: "i32".into(),
                string_words: None,
            };
            assert_eq!(
                checked_read_span(&spec, &r, PointCount::Unknown, PointCount::Unknown).unwrap(),
                ReadSpan {
                    device,
                    start: 4,
                    count
                }
            );
            assert!(
                checked_read_span(&spec, &q, PointCount::Unknown, PointCount::Unknown).is_err()
            );
            assert_eq!(
                <Component as address::Guest>::parse(text.into(), "i32".into(), None)
                    .unwrap()
                    .word_count,
                2
            );
        }
        for (text, kind, device) in [
            ("STS4", "bit", Device::STS),
            ("STC4", "bit", Device::STC),
            ("STN4", "u16", Device::STN),
            ("LCS4", "bit", Device::LCS),
            ("LCC4", "bit", Device::LCC),
        ] {
            let spec = TagSpec {
                address: text.into(),
                data_type: kind.into(),
                string_words: None,
            };
            assert_eq!(
                checked_read_span(&spec, &r, PointCount::Unknown, PointCount::Unknown)
                    .unwrap()
                    .device,
                device
            );
            assert!(
                checked_read_span(&spec, &q, PointCount::Unknown, PointCount::Unknown).is_err()
            );
        }
        let q_ret = TagSpec {
            address: "SS4".into(),
            data_type: "bit".into(),
            string_words: None,
        };
        assert!(checked_read_span(&q_ret, &r, PointCount::Unknown, PointCount::Unknown).is_err());
        let zr = TagSpec {
            address: "ZR1a".into(),
            data_type: "u32".into(),
            string_words: None,
        };
        assert_eq!(
            checked_read_span(&zr, &r, PointCount::Unknown, PointCount::Unknown).unwrap(),
            ReadSpan {
                device: Device::ZR,
                start: 26,
                count: 2
            }
        );
        assert!(checked_read_span(&zr, &q, PointCount::Unknown, PointCount::Unknown).is_err());
        let parsed =
            <Component as address::Guest>::parse("D10.f".into(), "bit".into(), None).unwrap();
        assert_eq!(
            (
                parsed.canonical.as_str(),
                parsed.area.as_str(),
                parsed.index,
                parsed.bit,
                parsed.word_count
            ),
            ("D10.F", "D", Some(10), Some(15), 1)
        );
        assert_eq!(
            <Component as address::Guest>::offset("D10.F".into(), "bit".into(), None, 1).unwrap(),
            "D11.0"
        );
        assert_eq!(
            <Component as address::Guest>::offset("LTN4".into(), "i32".into(), None, 1).unwrap(),
            "LTN5"
        );
        let timer_contact =
            <Component as address::Guest>::parse("LTS4".into(), "bit".into(), None).unwrap();
        assert_eq!(
            (
                timer_contact.area.as_str(),
                timer_contact.word_count,
                timer_contact.writable_area
            ),
            ("LTS", 1, false)
        );
        assert_eq!(
            <Component as address::Guest>::offset("LSTC4".into(), "bit".into(), None, 1).unwrap(),
            "LSTC5"
        );
        assert!(matches!(
            ReadKind::LongTimerFlag(1).decode_words(&[0, 0, 0b10, 0], WordOrder::LowHigh),
            Ok(Value::Bool(true))
        ));
        assert!(matches!(
            ReadKind::LongTimerFlag(0).decode_words(&[0, 0, 0b10, 0], WordOrder::LowHigh),
            Ok(Value::Bool(false))
        ));
        assert_eq!(
            <Component as address::Guest>::offset_words("D10.F".into(), 1).unwrap(),
            "D11.F"
        );
        assert_eq!(
            <Component as address::Guest>::offset("ZR1A".into(), "u32".into(), None, 1).unwrap(),
            "ZR1C"
        );
        for (text, kind) in [
            ("D0.G", "bit"),
            ("D0.10", "bit"),
            ("D0.0", "u16"),
            ("TN1", "u32"),
            ("LTS1", "u16"),
            ("LCS1A", "bit"),
            ("LCN1", "u32"),
            ("R1", "u16"),
        ] {
            assert!(
                parse_read_address(text, kind, None).is_err(),
                "{text}/{kind}"
            );
        }
        assert!(matches!(
            ReadKind::WordBit(15).decode_words(&[0x8000], WordOrder::LowHigh),
            Ok(Value::Bool(true))
        ));
        assert!(matches!(
            ReadKind::LongTimer.decode_words(&[0x1234, 0xffff, 3, 0], WordOrder::HighLow),
            Ok(Value::Int(-60876))
        ));
    }

    #[test]
    fn bit_addresses_keep_device_radix_point_units_and_unknown_range() {
        let r = model(ModelFamily::StandardR);
        for (text, canonical, area, number) in [
            ("M10", "M10", "M", 10),
            ("X0f", "XF", "X", 15),
            ("Y10", "Y10", "Y", 16),
            ("B1a", "B1A", "B", 26),
        ] {
            let parsed = <Component as address::Guest>::parse(text.into(), "bit".into(), None)
                .expect("bit address");
            assert_eq!(parsed.canonical, canonical);
            assert_eq!(parsed.area, area);
            assert_eq!(parsed.index, Some(number));
            assert_eq!(parsed.word_count, 1);
            assert_eq!(parsed.bit, None);
            assert!(!parsed.writable_area);
            let spec = TagSpec {
                address: text.into(),
                data_type: "bit".into(),
                string_words: None,
            };
            assert_eq!(
                checked_read_span(&spec, &r, PointCount::Exact(0), PointCount::Exact(0))
                    .unwrap()
                    .count,
                1
            );
        }
        for (address, kind, words) in [
            ("M1A", "bit", None),
            ("X10.1", "bit", None),
            ("x10", "bit", None),
            ("Y10", "u16", None),
            ("B10", "bit", Some(1)),
            ("D10", "bit", None),
            ("W10", "bit", None),
        ] {
            assert!(
                parse_read_address(address, kind, words).is_err(),
                "{address}/{kind}"
            );
        }
        assert_eq!(
            <Component as address::Guest>::offset("XF".into(), "bit".into(), None, 1).unwrap(),
            "X10"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("B1F".into(), 1).unwrap(),
            "B20"
        );
        assert_eq!(
            <Component as address::Guest>::offset("M0".into(), "bit".into(), None, -1),
            Err("read address exceeds u32".into())
        );
        assert!(<Component as address::Guest>::supported_kinds().contains(&"bit".into()));
        let q25 = model(ModelFamily::Q25PrhWireOnly);
        let spec = TagSpec {
            address: "B1A".into(),
            data_type: "bit".into(),
            string_words: None,
        };
        assert!(
            checked_read_span(&spec, &q25, PointCount::Exact(100), PointCount::Unknown).is_ok()
        );
        assert_eq!(
            read_count(Device::B, PointCount::Exact(100), PointCount::Exact(100)),
            PointCount::Unknown
        );
    }

    #[test]
    fn additional_device_syntax_keeps_units_unknown_and_write_disabled() {
        let r = model(ModelFamily::StandardR);
        for (text, canonical, area, index) in [
            ("L12", "L12", "L", 12),
            ("F13", "F13", "F", 13),
            ("V14", "V14", "V", 14),
            ("SB1a", "SB1A", "SB", 26),
            ("SM15", "SM15", "SM", 15),
        ] {
            let parsed =
                <Component as address::Guest>::parse(text.into(), "bit".into(), None).unwrap();
            assert_eq!(
                (
                    parsed.canonical.as_str(),
                    parsed.area.as_str(),
                    parsed.index
                ),
                (canonical, area, Some(index))
            );
            assert!(!parsed.writable_area);
            let spec = TagSpec {
                address: text.into(),
                data_type: "bit".into(),
                string_words: None,
            };
            let span =
                checked_read_span(&spec, &r, PointCount::Exact(0), PointCount::Exact(0)).unwrap();
            assert_eq!(
                read_count(span.device, PointCount::Exact(100), PointCount::Exact(100)),
                PointCount::Unknown
            );
        }
        for (text, canonical, area, index) in
            [("SW1a", "SW1A", "SW", 26), ("SD290", "SD290", "SD", 290)]
        {
            let parsed =
                <Component as address::Guest>::parse(text.into(), "u32".into(), None).unwrap();
            assert_eq!(
                (
                    parsed.canonical.as_str(),
                    parsed.area.as_str(),
                    parsed.index,
                    parsed.word_count
                ),
                (canonical, area, Some(index), 2)
            );
            assert!(!parsed.writable_area);
            let spec = TagSpec {
                address: text.into(),
                data_type: "u32".into(),
                string_words: None,
            };
            let span =
                checked_read_span(&spec, &r, PointCount::Exact(0), PointCount::Exact(0)).unwrap();
            assert_eq!(span.count, 2);
        }
        for (address, kind) in [
            ("SB1G", "bit"),
            ("SM1A", "bit"),
            ("SD1A", "u16"),
            ("SW1G", "u16"),
            ("L1", "u16"),
            ("SW1", "bit"),
            ("SWFFFFFFFF", "f64"),
        ] {
            assert!(
                parse_read_address(address, kind, None).is_err(),
                "{address}/{kind}"
            );
        }
        assert_eq!(
            <Component as address::Guest>::offset("SB1F".into(), "bit".into(), None, 1).unwrap(),
            "SB20"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("SW1F".into(), 1).unwrap(),
            "SW20"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("SD9".into(), 1).unwrap(),
            "SD10"
        );
    }

    #[test]
    fn r_w_hex_address_is_static_u16_writable_and_checks_complete_tag() {
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
        assert!(parsed.writable_area);
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
    fn standard_q_w_requires_a_verified_internal_prefix_for_the_complete_tag() {
        let q = model(ModelFamily::StandardQ);
        let pair = TagSpec {
            address: "W1E".into(),
            data_type: "u32".into(),
            string_words: None,
        };
        assert_eq!(
            checked_read_span(&pair, &q, PointCount::Unknown, PointCount::Exact(32)).unwrap(),
            ReadSpan {
                device: Device::W,
                start: 30,
                count: 2,
            }
        );
        assert!(checked_read_span(
            &TagSpec {
                address: "W1F".into(),
                ..pair.clone()
            },
            &q,
            PointCount::Unknown,
            PointCount::Exact(32)
        )
        .is_err());
        assert!(checked_read_span(&pair, &q, PointCount::Unknown, PointCount::Unknown).is_err());
        assert!(checked_read_span(
            &TagSpec {
                address: "W7FFF".into(),
                data_type: "u16".into(),
                ..pair.clone()
            },
            &q,
            PointCount::Unknown,
            PointCount::AtLeast(32768)
        )
        .is_ok());
        assert!(checked_read_span(
            &TagSpec {
                address: "W8000".into(),
                data_type: "u16".into(),
                ..pair.clone()
            },
            &q,
            PointCount::Unknown,
            PointCount::AtLeast(32768)
        )
        .is_err());
        assert!(checked_read_span(
            &pair,
            &model(ModelFamily::Q25PrhWireOnly),
            PointCount::Unknown,
            PointCount::Exact(32)
        )
        .is_ok());
        assert!(checked_read_span(
            &pair,
            &model(ModelFamily::Q25PrhWireOnly),
            PointCount::Unknown,
            PointCount::Unknown
        )
        .is_err());
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

    #[test]
    fn q25prh_write_setting_is_schema_declared_disabled_by_default() {
        let mut settings = vec![
            session::Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            session::Setting {
                key: "port".into(),
                value: "5000".into(),
            },
        ];
        assert!(validate_settings(&settings).is_empty());
        settings.push(session::Setting {
            key: "q25prh_d3010_3013_test_write".into(),
            value: "disabled".into(),
        });
        assert!(validate_settings(&settings).is_empty());
        settings.pop();
        settings.push(session::Setting {
            key: "q25prh_d3010_3013_test_write".into(),
            value: "enabled".into(),
        });
        assert!(validate_settings(&settings).is_empty());
        settings[2].value = "true".into();
        assert!(validate_settings(&settings)
            .iter()
            .any(|error| error.key == "q25prh_d3010_3013_test_write"));
        let schema = <Component as config::Guest>::schema();
        let field = schema
            .iter()
            .find(|field| field.key == "q25prh_d3010_3013_test_write")
            .expect("controlled test setting must be declared for normal settings validation");
        assert!(
            matches!(&field.default, Some(config::SettingValue::Text(value)) if value == "disabled")
        );
        assert!(!field.required);
        assert!(matches!(&field.group, config::FieldGroup::Advanced));
        assert!(matches!(&field.kind, config::FieldKind::Choice(options)
            if options.iter().map(|option| option.value.as_str()).collect::<Vec<_>>() == ["disabled", "enabled"]));
    }

    #[test]
    fn normal_write_policy_schema_and_validation_are_fail_closed() {
        let schema = <Component as config::Guest>::schema();
        let mode = schema
            .iter()
            .find(|field| field.key == MODE_KEY)
            .expect("mode field");
        assert!(
            matches!(&mode.default, Some(config::SettingValue::Text(value))
            if value == "disabled")
        );
        assert!(matches!(&mode.kind, config::FieldKind::Choice(options)
            if options.iter().map(|option| option.value.as_str()).collect::<Vec<_>>()
                == ["disabled", "enabled"]));
        let allowlist = schema
            .iter()
            .find(|field| field.key == ALLOWLIST_KEY)
            .expect("allowlist field");
        assert!(
            matches!(&allowlist.default, Some(config::SettingValue::Text(value))
            if value.is_empty())
        );
        assert!(matches!(&allowlist.kind, config::FieldKind::Text(spec)
            if spec.max_len == Some(256)));

        let mut settings = vec![
            session::Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            session::Setting {
                key: "port".into(),
                value: "5000".into(),
            },
        ];
        assert!(validate_settings(&settings).is_empty());
        settings.push(session::Setting {
            key: MODE_KEY.into(),
            value: "enabled".into(),
        });
        assert!(validate_settings(&settings)
            .iter()
            .any(|error| error.key == ALLOWLIST_KEY));
        settings.push(session::Setting {
            key: ALLOWLIST_KEY.into(),
            value: "D3010-D3013,D4000-D4009".into(),
        });
        assert!(validate_settings(&settings).is_empty());
        settings.push(session::Setting {
            key: "q25prh_d3010_3013_test_write".into(),
            value: "enabled".into(),
        });
        assert!(validate_settings(&settings)
            .iter()
            .any(|error| error.key == MODE_KEY));
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);
