//! Binary 4E or 3E SLMP guest for Q/R reads and restricted numeric/bit writes.
//! The host owns transport (`transport` tcp or udp; frames are identical).
//! Each connection owns its discovery evidence.
//! 3E has no serial number: reads are matched to responses in issue order.
//! A D/W word-bit write is a three-step read-modify-write (0401 read, 1401
//! write-back of `(current & !(set | clear)) | set`, 0401 confirm read). It is
//! not atomic: a change by the PLC to another bit of the same word between
//! the read and the write-back is overwritten, and the confirm read checks
//! only the written bits.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_register_codec::{
    decode_f32, decode_f64, decode_i16, decode_i32, decode_i64, decode_u32, decode_u64, WordOrder,
};
use banto_slmp_read_core::{
    basic_sd_count, decode_q_w_total, decode_read, decode_type_name, decode_write_ack, encode_bits,
    encode_q25prh_d3010_3013_test_write, encode_q_dw_words, encode_r_dw_words, encode_read,
    encode_type_name, plan_basic_sd, plan_q_w_total, BasicArea, BasicEvidence, Device, Frame,
    ModelFamily, PointCount, ReadSpan, ResponseError, Route, Series, TypeName, Values,
};
use exports::banto::driver::{address, config, diagnostics_v1, read, session, write_v1};
mod diagnostics;
mod write_policy;
use write_policy::{WritePolicy, ALLOWLIST_KEY, MODE_KEY};

wit_bindgen::generate!({ path: "../wit", world: "driver-with-write-and-diagnostics-v1" });

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
    /// QnU only: SD310-311, read once after a successful SD302-303.
    AwaitWTotal {
        route: Route,
        model: TypeName,
        d_count: PointCount,
        span: ReadSpan,
    },
    AwaitBitSd {
        route: Route,
        model: TypeName,
        d_count: PointCount,
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
    /// SD310-311 internal + extended W total. Read path only; never a write grant.
    q_w_total: Cell<Option<u32>>,
    bit_counts: Cell<[PointCount; 4]>, // M, Y, L, B; only same-connection SD evidence.
    diagnostic_bit_counts: Cell<[PointCount; 3]>, // X, SB, F; never a write grant.
    pending: RefCell<BTreeMap<u16, PendingRead>>,
    /// Pending read keys in issue order. 3E decodes only the front entry.
    read_order: RefCell<VecDeque<u16>>,
    pending_write: RefCell<Option<PendingWrite>>,
    write_poisoned: Cell<bool>,
    pending_diagnostic: RefCell<Option<diagnostics::PendingDiagnostic>>,
    diagnostic_poisoned: Cell<bool>,
    bit_sd_attempted: Cell<bool>,
    bit_sd_refused: Cell<bool>,
    dw_sd_refused: Cell<bool>,
}

#[derive(Clone)]
enum PendingWrite {
    Run {
        route: Route,
        run: WriteRun,
    },
    /// Word-bit read-modify-write; `route` is the request awaiting a response.
    Rmw {
        route: Route,
        series: Series,
        rmw: BitRmw,
        stage: RmwStage,
    },
}

/// One word-bit write request: distinct bits of a single D/W word.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BitRmw {
    device: Device,
    number: u32,
    /// Bit index of each item, in item order.
    bits: Vec<u8>,
    set: u16,
    clear: u16,
}

impl BitRmw {
    fn span(&self) -> ReadSpan {
        ReadSpan {
            device: self.device,
            start: self.number,
            count: 1,
        }
    }

    /// Keep every bit outside `set | clear` as read.
    const fn merge(&self, current: u16) -> u16 {
        (current & !(self.set | self.clear)) | self.set
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RmwStage {
    Read,
    WriteBack,
    Confirm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WritePayload {
    Words(Vec<u16>),
    Bits(Vec<bool>),
}

impl WritePayload {
    fn points(&self) -> usize {
        match self {
            Self::Words(words) => words.len(),
            Self::Bits(bits) => bits.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WriteRun {
    device: Device,
    number: u32,
    payload: WritePayload,
    all_u16: bool,
    kinds: Vec<ReadKind>,
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
            return Err("numeric word tags do not accept string_words".into());
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
            _ => Err("unsupported numeric word type".into()),
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

    fn encode(self, value: &Value, order: WordOrder) -> Result<Vec<u16>, String> {
        let bits: u64 = match self {
            Self::I16 | Self::I32 | Self::I64 => {
                let number = write_signed(value)?;
                let (min, max) = match self {
                    Self::I16 => (i16::MIN as i64, i16::MAX as i64),
                    Self::I32 => (i32::MIN as i64, i32::MAX as i64),
                    _ => (i64::MIN, i64::MAX),
                };
                if !(min..=max).contains(&number) {
                    return Err("signed write value exceeds tag width".into());
                }
                number as u64
            }
            Self::U16 | Self::U32 | Self::U64 => {
                let number = write_unsigned(value)?;
                let max = match self {
                    Self::U16 => u16::MAX as u64,
                    Self::U32 => u32::MAX as u64,
                    _ => u64::MAX,
                };
                if number > max {
                    return Err("unsigned write value exceeds tag width".into());
                }
                number
            }
            Self::F32 => {
                let Value::Float(number) = value else {
                    return Err("floating write requires a float value".into());
                };
                let narrowed = *number as f32;
                if !number.is_finite() || !narrowed.is_finite() {
                    return Err("non-finite floating write value".into());
                }
                narrowed.to_bits() as u64
            }
            Self::F64 => {
                let Value::Float(number) = value else {
                    return Err("floating write requires a float value".into());
                };
                if !number.is_finite() {
                    return Err("non-finite floating write value".into());
                }
                number.to_bits()
            }
        };
        let mut words = (0..self.words())
            .map(|part| (bits >> (part * 16)) as u16)
            .collect::<Vec<_>>();
        if order == WordOrder::HighLow && words.len() > 1 {
            words.reverse();
        }
        Ok(words)
    }
}

// WIT has exact Int/Uint variants. Legacy Float integer values are accepted
// only while their integer is exactly representable in binary64.
fn write_signed(value: &Value) -> Result<i64, String> {
    match value {
        Value::Int(number) => Ok(*number),
        Value::Float(number)
            if number.is_finite()
                && number.fract() == 0.0
                && number.abs() <= 9_007_199_254_740_991.0 =>
        {
            Ok(*number as i64)
        }
        _ => Err("integer write requires an exact finite integer".into()),
    }
}

fn write_unsigned(value: &Value) -> Result<u64, String> {
    match value {
        Value::Uint(number) => Ok(*number),
        Value::Float(number)
            if number.is_finite()
                && number.fract() == 0.0
                && (0.0..=9_007_199_254_740_991.0).contains(number) =>
        {
            Ok(*number as u64)
        }
        _ => Err("integer write requires an exact finite integer".into()),
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
    fn new(settings: &[session::Setting]) -> Result<Self, FatalError> {
        if let Some(error) = validate_settings(settings).first() {
            return Err(FatalError::InvalidConfig(error.message.clone()));
        }
        let (network, pc, io) = route_fields(settings);
        Ok(Self {
            base_route: Route {
                frame: match setting(settings, "frame") {
                    Some("3e") => Frame::E3,
                    _ => Frame::E4,
                },
                network,
                pc,
                io,
                ..Route::default()
            },
            word_order: match setting(settings, "word_order") {
                Some("high_low") => WordOrder::HighLow,
                _ => WordOrder::LowHigh,
            },
            q25prh_test_write_enabled: setting(settings, "q25prh_d3010_3013_test_write")
                == Some("enabled"),
            write_policy: WritePolicy::parse(
                setting(settings, MODE_KEY),
                setting(settings, ALLOWLIST_KEY),
            )
            .expect("settings validated above"),
            q25prh_internal_d_count: Cell::new(PointCount::Unknown),
            next_serial: Cell::new(1),
            phase: RefCell::new(Phase::Start),
            w_count: Cell::new(PointCount::Unknown),
            q_w_total: Cell::new(None),
            bit_counts: Cell::new([PointCount::Unknown; 4]),
            diagnostic_bit_counts: Cell::new([PointCount::Unknown; 3]),
            pending: RefCell::new(BTreeMap::new()),
            read_order: RefCell::new(VecDeque::new()),
            pending_write: RefCell::new(None),
            write_poisoned: Cell::new(false),
            pending_diagnostic: RefCell::new(None),
            diagnostic_poisoned: Cell::new(false),
            bit_sd_attempted: Cell::new(false),
            bit_sd_refused: Cell::new(false),
            dw_sd_refused: Cell::new(false),
        })
    }

    fn finish_dw_discovery(
        &self,
        model: TypeName,
        d_count: PointCount,
    ) -> Result<Option<session::Step>, FatalError> {
        if self.write_policy.has_bit_ranges()
            && matches!(
                model.family,
                ModelFamily::StandardQ | ModelFamily::StandardR
            )
        {
            let span = if model.family == ModelFamily::StandardQ {
                ReadSpan {
                    device: Device::SD,
                    start: 290,
                    count: 5,
                }
            } else {
                ReadSpan {
                    device: Device::SD,
                    start: 262,
                    count: 14,
                }
            };
            self.bit_sd_attempted.set(true);
            let route = self.route();
            let series = known_series(&model).expect("standard Q/R model");
            let send = encode_read(route, series, span)
                .map_err(|_| FatalError::Protocol("invalid bit-count SD span".into()))?;
            *self.phase.borrow_mut() = Phase::AwaitBitSd {
                route,
                model,
                d_count,
                span,
            };
            Ok(Some(frame_step(route.frame, send)))
        } else {
            *self.phase.borrow_mut() = Phase::Ready {
                model,
                count: d_count,
            };
            Ok(None)
        }
    }

    fn route(&self) -> Route {
        let serial = self.next_serial.get();
        self.next_serial.set(serial.wrapping_add(1));
        Route {
            serial,
            ..self.base_route
        }
    }
}

fn frame_step(frame: Frame, send: Vec<u8>) -> session::Step {
    session::Step {
        send,
        expect: session::FrameRule::LengthPrefixed(session::PrefixSpec {
            offset: frame.length_offset() as u32,
            width: 2,
            big_endian: false,
            adjust: frame.length_adjust() as i32,
            encoding: session::PrefixEncoding::Binary,
        }),
        max_len: 64,
    }
}

fn read_step(frame: Frame, send: Vec<u8>, span: ReadSpan) -> session::Step {
    let mut step = frame_step(frame, send);
    // Binary bit responses pack two points per byte; words use two bytes.
    // Keep the previous allowance for short PLC refusal frames.
    let body_len = if span.device.is_bit() {
        u32::from(span.count).div_ceil(2)
    } else {
        u32::from(span.count) * 2
    };
    step.max_len = (frame.response_header_len() as u32 + body_len).max(64);
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

/// Strict decimal parse for route numbers: ASCII digits only, no sign,
/// spaces or leading zeros (except "0" itself).
fn parse_route_number(text: &str, min: u8, max: u8) -> Option<u8> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse::<u8>()
        .ok()
        .filter(|value| (min..=max).contains(value))
}

fn target_cpu_io(value: &str) -> Option<u16> {
    match value {
        "own" => Some(0x03FF),
        "cpu1" => Some(0x03E0),
        "cpu2" => Some(0x03E1),
        "cpu3" => Some(0x03E2),
        "cpu4" => Some(0x03E3),
        _ => None,
    }
}

/// Request destination (network, station-as-`pc`, module I/O) from validated
/// settings. The own station (network 0) uses station FFH.
fn route_fields(settings: &[session::Setting]) -> (u8, u8, u16) {
    let network = setting(settings, "network_no")
        .and_then(|value| parse_route_number(value, 0, 239))
        .unwrap_or(0);
    let pc = if network == 0 {
        0xFF
    } else {
        setting(settings, "station_no")
            .and_then(|value| parse_route_number(value, 1, 120))
            .unwrap_or(0xFF)
    };
    let io = setting(settings, "target_cpu")
        .and_then(target_cpu_io)
        .unwrap_or(0x03FF);
    (network, pc, io)
}

fn validate_route_settings(settings: &[session::Setting], errors: &mut Vec<config::FieldError>) {
    let mut push = |key: &str, message: &str| {
        errors.push(config::FieldError {
            key: key.into(),
            message: message.into(),
        });
    };
    let network = setting(settings, "network_no");
    let station = setting(settings, "station_no");
    let network_ok = network.map(|value| parse_route_number(value, 0, 239));
    let station_ok = station.map(|value| parse_route_number(value, 1, 120));
    if network_ok == Some(None) {
        push(
            "network_no",
            "network_no must be a decimal number from 0 to 239",
        );
    }
    if station_ok == Some(None) {
        push(
            "station_no",
            "station_no must be a decimal number from 1 to 120",
        );
    }
    if network_ok != Some(None) && station_ok != Some(None) {
        let routed = network_ok.flatten().is_some_and(|value| value != 0);
        if routed && station.is_none() {
            push(
                "station_no",
                "station_no is required when network_no is not 0",
            );
        } else if !routed && station.is_some() {
            push(
                "station_no",
                "station_no must be omitted for the connected station (network_no 0)",
            );
        }
    }
    if setting(settings, "target_cpu").is_some_and(|value| target_cpu_io(value).is_none()) {
        push(
            "target_cpu",
            "target_cpu must be own, cpu1, cpu2, cpu3 or cpu4",
        );
    }
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
                | "frame"
                | "network_no"
                | "station_no"
                | "target_cpu"
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
    if setting(settings, "transport").is_some_and(|value| !matches!(value, "tcp" | "udp")) {
        errors.push(config::FieldError {
            key: "transport".into(),
            message: "transport must be tcp or udp".into(),
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
    if setting(settings, "frame").is_some_and(|value| !matches!(value, "4e" | "3e")) {
        errors.push(config::FieldError {
            key: "frame".into(),
            message: "frame must be 4e or 3e".into(),
        });
    }
    validate_route_settings(settings, &mut errors);
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
    // Dn.h and Wn.h read one bit of one word. D numbers are decimal and W
    // numbers hexadecimal; the bit index is one hexadecimal digit for both.
    for (prefix, device, radix) in [('D', Device::D, 10), ('W', Device::W, 16)] {
        let Some((word, bit)) = text
            .strip_prefix(prefix)
            .and_then(|rest| rest.split_once('.'))
        else {
            continue;
        };
        if data_type != "bit" || words.is_some() {
            return Err(format!(
                "{prefix} word-bit requires bit type without string_words"
            ));
        }
        let digit = |b: u8| {
            if radix == 10 {
                b.is_ascii_digit()
            } else {
                b.is_ascii_hexdigit()
            }
        };
        if word.is_empty() || !word.bytes().all(digit) {
            return Err(format!("invalid {prefix} word-bit address"));
        }
        if bit.len() != 1 || !bit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("word bit must be one hexadecimal digit".into());
        }
        let number = u32::from_str_radix(word, radix)
            .map_err(|_| format!("{prefix} address exceeds u32"))?;
        let bit = u8::from_str_radix(bit, 16).map_err(|_| "invalid word bit")?;
        return Ok((device, number, ReadKind::WordBit(bit)));
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
    // LZ is not allowed in 0401 batch read; it needs 0403 random read.
    // Check before the single-letter L bit prefix.
    if text.starts_with("LZ") {
        return Err("LZ long index register needs 0403 random read and is not supported".into());
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
        // Step relay S: only when a digit follows, so SD/SW/SN keep their word parsing.
        ("S", Device::S, 10),
    ];
    if text.starts_with('V') {
        return Err(
            "V edge relay cannot be collected reliably: a one-scan edge may be missed".into(),
        );
    }
    if let Some((device, radix, digits)) =
        bit_prefixes.iter().find_map(|(prefix, device, radix)| {
            if *prefix == "S" && !text.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
                return None;
            }
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
    } else if let Some(digits) = text.strip_prefix('Z') {
        (Device::Z, 10, digits)
    } else if let Some(digits) = text.strip_prefix("RD") {
        (Device::RD, 10, digits)
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
        Device::S => Ok(format!("S{number}")),
        Device::Z => Ok(format!("Z{number}")),
        Device::RD => Ok(format!("RD{number}")),
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

/// Where a Q W span lies when SD310-311 is usable. Internal and extended W
/// have no physical continuity, so a request never covers both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QWRegion {
    Internal,
    Extended,
    Outside,
    /// Crosses the internal/extended boundary or reaches W10000H.
    Unsupported,
}

/// Extended W is considered only when SD303 is exact (WC) and SD310-311 (T) is
/// not below it. `None` keeps the internal-prefix rules. Outside (last >= T)
/// takes precedence over the unsupported cases.
fn classify_q_w(
    w_count: PointCount,
    w_total: Option<u32>,
    number: u32,
    words: u16,
) -> Option<QWRegion> {
    let (PointCount::Exact(wc), Some(total)) = (w_count, w_total) else {
        return None;
    };
    if total < wc {
        return None;
    }
    let Some(last) = number.checked_add(u32::from(words - 1)) else {
        return Some(QWRegion::Outside);
    };
    Some(if last >= total {
        QWRegion::Outside
    } else if last < wc {
        QWRegion::Internal
    } else if number < wc || last >= 0x10000 {
        QWRegion::Unsupported
    } else {
        QWRegion::Extended
    })
}

/// Region of a read span on a target Q W area; `None` for every other case.
fn q_w_region(
    model: &TypeName,
    device: Device,
    w_count: PointCount,
    w_total: Option<u32>,
    number: u32,
    words: u16,
) -> Option<QWRegion> {
    if device != Device::W || model.family != ModelFamily::StandardQ {
        return None;
    }
    classify_q_w(w_count, w_total, number, words)
}

/// Range verdict for a read span. Only Q W with a usable SD310-311 differs
/// from `assess_d_span`.
fn assess_read_span(
    model: &TypeName,
    device: Device,
    d_count: PointCount,
    w_count: PointCount,
    w_total: Option<u32>,
    number: u32,
    words: u16,
) -> RangeAssessment {
    match q_w_region(model, device, w_count, w_total, number, words) {
        Some(QWRegion::Internal | QWRegion::Extended) => RangeAssessment::VerifiedInside,
        Some(QWRegion::Outside) => RangeAssessment::VerifiedOutside,
        Some(QWRegion::Unsupported) => RangeAssessment::Unknown,
        None => assess_d_span(model, read_count(device, d_count, w_count), number, words),
    }
}

fn count_guarantees_span(count: PointCount, number: u32, words: u16) -> bool {
    let Some(end_exclusive) = number.checked_add(u32::from(words)) else {
        return false;
    };
    matches!(count, PointCount::Exact(limit) | PointCount::AtLeast(limit) if end_exclusive <= limit)
}

fn write_bit_count(conn: &SlmpConnection, device: Device) -> PointCount {
    let counts = conn.bit_counts.get();
    match device {
        Device::M => counts[0],
        Device::Y => counts[1],
        Device::L => counts[2],
        Device::B => counts[3],
        _ => PointCount::Unknown,
    }
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
        | Device::ZR
        | Device::S
        | Device::Z
        | Device::RD => PointCount::Unknown,
    }
}

/// ZR is a read-only, range-Unknown, one-tag read on standard R and on
/// standard Q CPUs that have a file register. Q00JCPU (no standard RAM) and
/// Q00UJCPU (no file register) are matched by the exact name/code pair the
/// type-name classifier uses. Q25PRH has no ZR path. SD306-307 is not read.
fn zr_read_allowed(model: &TypeName) -> bool {
    match model.family {
        ModelFamily::StandardR => true,
        ModelFamily::StandardQ => !matches!(
            (model.name.as_str(), model.code),
            ("Q00JCPU", 0x0250) | ("Q00UJCPU", 0x0260)
        ),
        _ => false,
    }
}

fn checked_read_span(
    spec: &TagSpec,
    model: &TypeName,
    d_count: PointCount,
    w_count: PointCount,
) -> Result<ReadSpan, FatalError> {
    checked_read_span_ext(spec, model, d_count, w_count, None)
}

/// `checked_read_span` plus the SD310-311 total for QnU extended W reads.
fn checked_read_span_ext(
    spec: &TagSpec,
    model: &TypeName,
    d_count: PointCount,
    w_count: PointCount,
    w_total: Option<u32>,
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
                | Device::RD
        ) && model.family != ModelFamily::StandardR))
        || (device == Device::ZR && !zr_read_allowed(model))
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
            ModelFamily::StandardQ
                if matches!(
                    classify_q_w(w_count, w_total, number, kind.points()),
                    Some(QWRegion::Internal | QWRegion::Extended)
                ) => {}
            ModelFamily::StandardQ
                if classify_q_w(w_count, w_total, number, kind.points())
                    == Some(QWRegion::Outside) =>
            {
                return Err(FatalError::InvalidConfig(
                    "address exceeds verified configured count".into(),
                ));
            }
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
    if assess_read_span(
        model,
        device,
        d_count,
        w_count,
        w_total,
        number,
        kind.points(),
    ) == RangeAssessment::VerifiedOutside
    {
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
        Ok(session::Connection::new(Self::new(&settings)?))
    }

    fn handshake_next(
        &self,
        prev_response: Option<Vec<u8>>,
    ) -> Result<Option<session::Step>, FatalError> {
        if self.pending_diagnostic.borrow().is_some() || self.diagnostic_poisoned.get() {
            return Err(FatalError::Protocol(
                "diagnostic response is pending; reconnect".into(),
            ));
        }
        let phase = std::mem::replace(&mut *self.phase.borrow_mut(), Phase::NoRange(None));
        match (phase, prev_response) {
            (Phase::Start, None) => {
                let route = self.route();
                *self.phase.borrow_mut() = Phase::AwaitType { route };
                Ok(Some(frame_step(route.frame, encode_type_name(route))))
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
                Ok(Some(frame_step(sd_route.frame, send)))
            }
            (Phase::AwaitSd { route, model, span }, Some(frame)) => {
                let words = match decode_read(route, span, &frame) {
                    Ok(Values::Words(words)) => words,
                    Err(ResponseError::EndCode(_)) => {
                        // A PLC refusal leaves the count unknown, but the confirmed
                        // model still gives us a supported wire format for explicit reads.
                        self.dw_sd_refused.set(true);
                        return self.finish_dw_discovery(model, PointCount::Unknown);
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
                } else if let Some(span) = plan_q_w_total(&model) {
                    let route = self.route();
                    let send = encode_read(route, Series::Q, span)
                        .map_err(|_| FatalError::Protocol("invalid W total SD span".into()))?;
                    *self.phase.borrow_mut() = Phase::AwaitWTotal {
                        route,
                        model,
                        d_count: count,
                        span,
                    };
                    return Ok(Some(frame_step(route.frame, send)));
                } else {
                    return self.finish_dw_discovery(model, count);
                }
                Ok(None)
            }
            (
                Phase::AwaitWTotal {
                    route,
                    model,
                    d_count,
                    span,
                },
                Some(frame),
            ) => {
                match decode_read(route, span, &frame) {
                    Ok(Values::Words(words)) => {
                        self.q_w_total.set(decode_q_w_total(&model, Some(&words)));
                    }
                    // A refusal only leaves extended W closed; the internal
                    // prefix rules and the rest of discovery are unchanged.
                    Err(ResponseError::EndCode(_)) => {}
                    Err(error) => return Err(response_error(error)),
                    Ok(Values::Bits(_)) => {
                        return Err(FatalError::Protocol("SD read returned bits".into()));
                    }
                }
                self.finish_dw_discovery(model, d_count)
            }
            (
                Phase::AwaitBitSd {
                    route,
                    model,
                    d_count,
                    span,
                },
                Some(frame),
            ) => {
                let decoded = match decode_read(route, span, &frame) {
                    Ok(Values::Words(words)) => Some(words),
                    Err(ResponseError::EndCode(_)) => {
                        self.bit_sd_refused.set(true);
                        None
                    }
                    Err(error) => return Err(response_error(error)),
                    Ok(Values::Bits(_)) => {
                        return Err(FatalError::Protocol("SD read returned bits".into()))
                    }
                };
                if let Some(words) = decoded {
                    diagnostics::update_bit_counts(self, &model, &words);
                }
                *self.phase.borrow_mut() = Phase::Ready {
                    model,
                    count: d_count,
                };
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
        if conn.pending_write.borrow().is_some()
            || conn.write_poisoned.get()
            || conn.pending_diagnostic.borrow().is_some()
            || conn.diagnostic_poisoned.get()
        {
            return Err(FatalError::Protocol("write response is pending".into()));
        }
        if conn.pending_diagnostic.borrow().is_some() || conn.diagnostic_poisoned.get() {
            return Err(FatalError::Protocol(
                "diagnostic response is pending; reconnect".into(),
            ));
        }
        if conn.base_route.frame == Frame::E3 && !conn.pending.borrow().is_empty() {
            // Without a serial number a late answer to an earlier read would be
            // indistinguishable from the answer to a new one.
            return Err(FatalError::Protocol(
                "3E read is pending; responses cannot be matched".into(),
            ));
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
                let span = checked_read_span_ext(
                    spec,
                    model,
                    *count,
                    conn.w_count.get(),
                    conn.q_w_total.get(),
                )
                .ok()?;
                if diagnostics::bit_span_outside(conn, model, span) {
                    return None;
                }
                encode_read(conn.base_route, series, span).ok()?;
                Some((
                    u32::try_from(index).ok()?,
                    span,
                    assess_read_span(
                        model,
                        span.device,
                        *count,
                        conn.w_count.get(),
                        conn.q_w_total.get(),
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
                        || assess_read_span(
                            model,
                            span.device,
                            *count,
                            conn.w_count.get(),
                            conn.q_w_total.get(),
                            last.span.start,
                            last.span.count,
                        ) != RangeAssessment::VerifiedInside
                    {
                        return false;
                    }
                    // Never merge across the internal/extended W boundary.
                    let region = |span: ReadSpan| {
                        q_w_region(
                            model,
                            span.device,
                            conn.w_count.get(),
                            conn.q_w_total.get(),
                            span.start,
                            span.count,
                        )
                    };
                    if region(last.span) != region(span) {
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
            let step = read_step(route.frame, send, group.span);
            plans.push(read::Planned {
                step,
                covers: group.covers.clone(),
            });
            new_pending.push((serial, group));
            serial = serial.wrapping_add(1);
        }
        drop(pending);
        conn.read_order
            .borrow_mut()
            .extend(new_pending.iter().map(|(serial, _)| *serial));
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
        if conn.pending_diagnostic.borrow().is_some() || conn.diagnostic_poisoned.get() {
            return Err(FatalError::Protocol(
                "diagnostic response is pending; reconnect".into(),
            ));
        }
        let phase = conn.phase.borrow();
        let Phase::Ready { model, count } = &*phase else {
            return Err(FatalError::Protocol(
                "configured range is unresolved".into(),
            ));
        };
        let serial = match conn.base_route.frame {
            Frame::E4 => {
                let serial_bytes: [u8; 2] = p
                    .step
                    .send
                    .get(2..4)
                    .ok_or_else(|| FatalError::Protocol("invalid plan".into()))?
                    .try_into()
                    .map_err(|_| FatalError::Protocol("invalid plan".into()))?;
                u16::from_le_bytes(serial_bytes)
            }
            // 3E has no serial on the wire. The Host answers plans one at a
            // time in plan order, so only the oldest pending read may decode;
            // its covers and exact bytes are checked below.
            Frame::E3 => *conn.read_order.borrow().front().ok_or_else(|| {
                FatalError::Protocol("plan is not pending on this connection".into())
            })?,
        };
        let route = Route {
            serial,
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
        let mut first_region = None;
        for (position, cover) in p.covers.iter().enumerate() {
            let spec = specs
                .get(*cover as usize)
                .ok_or_else(|| FatalError::Protocol("cover outside specs".into()))?;
            let tag = checked_read_span_ext(
                spec,
                model,
                *count,
                conn.w_count.get(),
                conn.q_w_total.get(),
            )?;
            let region = q_w_region(
                model,
                tag.device,
                conn.w_count.get(),
                conn.q_w_total.get(),
                tag.start,
                tag.count,
            );
            if position == 0 {
                first_region = region;
            }
            let expected_d = pending
                .span
                .start
                .checked_add(u32::from(word_offset))
                .ok_or_else(|| FatalError::Protocol("batch address overflow".into()))?;
            if tag.device != pending.span.device
                || tag.start != expected_d
                || (p.covers.len() > 1
                    && (assess_read_span(
                        model,
                        tag.device,
                        *count,
                        conn.w_count.get(),
                        conn.q_w_total.get(),
                        tag.start,
                        tag.count,
                    ) != RangeAssessment::VerifiedInside
                        || region != first_region))
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
            || p.step.max_len != read_step(route.frame, expected, pending.span).max_len
        {
            return Err(FatalError::Protocol("plan bytes do not match spec".into()));
        }
        conn.pending.borrow_mut().remove(&route.serial);
        conn.read_order
            .borrow_mut()
            .retain(|serial| *serial != route.serial);
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
        write_next_on(conn.get::<SlmpConnection>(), items, prev_response)
    }
}

fn write_next_on(
    conn: &SlmpConnection,
    items: Vec<write_v1::WriteItem>,
    prev_response: Option<Vec<u8>>,
) -> Result<write_v1::WriteProgress, FatalError> {
    if conn.pending_diagnostic.borrow().is_some() || conn.diagnostic_poisoned.get() {
        return Err(FatalError::Protocol(
            "diagnostic response is pending; reconnect".into(),
        ));
    }
    if let Some(response) = prev_response {
        // Consume before parsing so a malformed or refused ACK cannot be retried.
        conn.write_poisoned.set(true);
        let pending = conn.pending_write.borrow_mut().take().ok_or_else(|| {
            FatalError::Protocol("write response without a pending request".into())
        })?;
        let (route, pending_run) = match pending {
            PendingWrite::Run { route, run } => (route, run),
            PendingWrite::Rmw {
                route,
                series,
                rmw,
                stage,
            } => return rmw_response(conn, &items, route, series, rmw, stage, &response),
        };
        let run = parse_write_run(&items, conn.word_order)
            .map_err(|_| FatalError::Protocol("write items changed after send".into()))?;
        if run != pending_run {
            return Err(FatalError::Protocol("write item changed after send".into()));
        }
        let len = items.len();
        return match decode_write_ack(route, &response) {
            Ok(()) => {
                conn.write_poisoned.set(false);
                Ok(write_v1::WriteProgress::Done(
                    (0..len).map(|_| Ok(())).collect(),
                ))
            }
            Err(ResponseError::EndCode(code)) => {
                conn.write_poisoned.set(false);
                Ok(device_error_results(len, code))
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
            "write requires at least one supported item".into(),
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
    // A request naming any D/W word bit is a read-modify-write of one word.
    match parse_bit_rmw(&items) {
        Ok(Some(rmw)) => return rmw_preflight(conn, &model, count, rmw, items.len()),
        Ok(None) => {}
        Err(message) => return Ok(rejected_write_count(items.len(), &message)),
    }
    let run = match parse_write_run(&items, conn.word_order) {
        Ok(run) => run,
        Err(message) => return Ok(rejected_write_count(items.len(), &message)),
    };
    let device = run.device;
    let number = run.number;
    let points = run.payload.points();
    if q25prh_test
        && (device != Device::D
            || number != 3010
            || points != 4
            || !run.all_u16
            || items.len() != 4)
    {
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
    if !q25prh_test && !conn.write_policy.allows_device_span(device, number, points) {
        return Ok(rejected_write_count(
            items.len(),
            "device span is outside the enabled connection allowlist",
        ));
    }
    if !q25prh_test && !write_span_proven(conn, &model, count, q25prh, device, number, points) {
        return Ok(rejected_write_count(
            items.len(),
            "device span is not within the same-connection SD range",
        ));
    }
    let route = conn.route();
    let send = if q25prh_test {
        // This opt-in never upgrades Q's unknown configured D count.
        let WritePayload::Words(ref values) = run.payload else {
            unreachable!("checked Q test unit")
        };
        let words: [u16; 4] = values.as_slice().try_into().expect("checked Q test width");
        encode_q25prh_d3010_3013_test_write(route, words)
    } else {
        match &run.payload {
            WritePayload::Words(values) if model.family == ModelFamily::StandardR => {
                encode_r_dw_words(route, device, number, values)
            }
            WritePayload::Words(values) => encode_q_dw_words(route, device, number, values),
            WritePayload::Bits(bits) => encode_bits(
                route,
                if model.family == ModelFamily::StandardR {
                    Series::R
                } else {
                    Series::Q
                },
                device,
                number,
                bits,
            ),
        }
        .map_err(|_| FatalError::Protocol("validated write span cannot be encoded".into()))?
    };
    *conn.pending_write.borrow_mut() = Some(PendingWrite::Run { route, run });
    Ok(write_v1::WriteProgress::Send(read::Planned {
        step: frame_step(route.frame, send),
        covers: all_covers(items.len()),
    }))
}

fn all_covers(len: usize) -> Vec<u32> {
    (0..len as u32).collect()
}

fn device_error_results(len: usize, code: u16) -> write_v1::WriteProgress {
    write_v1::WriteProgress::Done(
        (0..len)
            .map(|_| {
                Err(ItemError::Device(banto::driver::types::DeviceError {
                    code: u32::from(code),
                    message: format!("SLMP end code 0x{code:04X}"),
                }))
            })
            .collect(),
    )
}

/// Same-connection SD evidence for a word or bit-device write span. Standard
/// Q D has no write proof (SD302 counts internal D only); Q W uses SD303.
fn write_span_proven(
    conn: &SlmpConnection,
    model: &TypeName,
    count: PointCount,
    q25prh: bool,
    device: Device,
    number: u32,
    points: usize,
) -> bool {
    if device.is_bit() {
        matches!(
            model.family,
            ModelFamily::StandardQ | ModelFamily::StandardR
        ) && count_guarantees_span(write_bit_count(conn, device), number, points as u16)
    } else if model.family == ModelFamily::StandardR {
        let area_count = if device == Device::D {
            count
        } else {
            conn.w_count.get()
        };
        assess_d_span(model, area_count, number, points as u16) == RangeAssessment::VerifiedInside
    } else if model.family == ModelFamily::StandardQ && device == Device::W {
        count_guarantees_span(conn.w_count.get(), number, points as u16)
    } else if q25prh && device == Device::D {
        count_guarantees_span(conn.q25prh_internal_d_count.get(), number, points as u16)
    } else {
        false
    }
}

/// `Ok(None)` when no item names a word bit, so the ordinary run path decides.
/// Otherwise every item must be a distinct bit of one D/W word with a bool value.
fn parse_bit_rmw(items: &[write_v1::WriteItem]) -> Result<Option<BitRmw>, String> {
    let parsed = items
        .iter()
        .map(|item| {
            parse_read_address(
                &item.spec.address,
                &item.spec.data_type,
                item.spec.string_words,
            )
        })
        .collect::<Vec<_>>();
    if !parsed
        .iter()
        .any(|address| matches!(address, Ok((_, _, ReadKind::WordBit(_)))))
    {
        return Ok(None);
    }
    let mut word = None;
    let mut bits = Vec::with_capacity(items.len());
    let (mut set, mut clear) = (0u16, 0u16);
    for (item, address) in items.iter().zip(parsed) {
        let Ok((device @ (Device::D | Device::W), number, ReadKind::WordBit(bit))) = address else {
            return Err("a word-bit write cannot be mixed with other items".into());
        };
        if word.is_some_and(|word| word != (device, number)) {
            return Err("word-bit write items must be bits of one word".into());
        }
        word = Some((device, number));
        let Value::Bool(value) = item.value else {
            return Err("word-bit write requires a bool value".into());
        };
        let mask = 1u16 << bit;
        if (set | clear) & mask != 0 {
            return Err("word-bit write names the same bit twice".into());
        }
        if value {
            set |= mask;
        } else {
            clear |= mask;
        }
        bits.push(bit);
    }
    let (device, number) = word.expect("at least one word bit");
    Ok(Some(BitRmw {
        device,
        number,
        bits,
        set,
        clear,
    }))
}

/// Check everything before the first (read) step; nothing is sent on refusal.
fn rmw_preflight(
    conn: &SlmpConnection,
    model: &TypeName,
    count: PointCount,
    rmw: BitRmw,
    len: usize,
) -> Result<write_v1::WriteProgress, FatalError> {
    let series = match model.family {
        ModelFamily::StandardQ => Series::Q,
        ModelFamily::StandardR => Series::R,
        _ => {
            return Ok(rejected_write_count(
                len,
                "word-bit write requires a confirmed standard Q/R model",
            ))
        }
    };
    if rmw.device == Device::D && (3000..=3001).contains(&rmw.number) {
        return Ok(rejected_write_count(len, "D3000 and D3001 are protected"));
    }
    if !conn
        .write_policy
        .allows_word_bits(rmw.device, rmw.number, &rmw.bits)
    {
        return Ok(rejected_write_count(
            len,
            "word bits are outside the enabled connection bit allowlist",
        ));
    }
    // The extended Q W total (SD310-311) is a read path only.
    if !write_span_proven(conn, model, count, false, rmw.device, rmw.number, 1) {
        return Ok(rejected_write_count(
            len,
            "device span is not within the same-connection SD range",
        ));
    }
    let route = conn.route();
    let span = rmw.span();
    let send = encode_read(route, series, span)
        .map_err(|_| FatalError::Protocol("validated word-bit span cannot be encoded".into()))?;
    *conn.pending_write.borrow_mut() = Some(PendingWrite::Rmw {
        route,
        series,
        rmw,
        stage: RmwStage::Read,
    });
    Ok(write_v1::WriteProgress::Send(read::Planned {
        step: read_step(route.frame, send, span),
        covers: all_covers(len),
    }))
}

/// One RMW response. The caller has consumed the pending step and poisoned
/// writes; only a final outcome or a PLC refusal clears the poison.
fn rmw_response(
    conn: &SlmpConnection,
    items: &[write_v1::WriteItem],
    route: Route,
    series: Series,
    rmw: BitRmw,
    stage: RmwStage,
    response: &[u8],
) -> Result<write_v1::WriteProgress, FatalError> {
    if parse_bit_rmw(items).ok().flatten().as_ref() != Some(&rmw) {
        return Err(FatalError::Protocol("write item changed after send".into()));
    }
    let len = items.len();
    if stage == RmwStage::WriteBack {
        return match decode_write_ack(route, response) {
            Ok(()) => {
                let next = conn.route();
                let span = rmw.span();
                let send = encode_read(next, series, span).map_err(|_| {
                    FatalError::Protocol("validated word-bit span cannot be encoded".into())
                })?;
                *conn.pending_write.borrow_mut() = Some(PendingWrite::Rmw {
                    route: next,
                    series,
                    rmw,
                    stage: RmwStage::Confirm,
                });
                Ok(write_v1::WriteProgress::Send(read::Planned {
                    step: read_step(next.frame, send, span),
                    covers: all_covers(len),
                }))
            }
            Err(ResponseError::EndCode(code)) => {
                conn.write_poisoned.set(false);
                Ok(device_error_results(len, code))
            }
            Err(error) => Err(response_error(error)),
        };
    }
    let word = match decode_read(route, rmw.span(), response) {
        Ok(Values::Words(words)) if words.len() == 1 => words[0],
        Ok(_) => {
            return Err(FatalError::Protocol(
                "word-bit read returned an unexpected unit".into(),
            ))
        }
        Err(ResponseError::EndCode(code)) => {
            conn.write_poisoned.set(false);
            return Ok(if stage == RmwStage::Read {
                // Refused before the write-back: the word is unchanged.
                device_error_results(len, code)
            } else {
                // The write-back was accepted; its effect is not confirmed.
                write_v1::WriteProgress::Done(
                    (0..len)
                        .map(|_| {
                            Err(ItemError::VerificationFailed(format!(
                                "confirm read refused: SLMP end code 0x{code:04X}"
                            )))
                        })
                        .collect(),
                )
            });
        }
        Err(error) => return Err(response_error(error)),
    };
    if stage == RmwStage::Read {
        let next = conn.route();
        let words = [rmw.merge(word)];
        let send = if series == Series::R {
            encode_r_dw_words(next, rmw.device, rmw.number, &words)
        } else {
            encode_q_dw_words(next, rmw.device, rmw.number, &words)
        }
        .map_err(|_| FatalError::Protocol("validated word-bit span cannot be encoded".into()))?;
        *conn.pending_write.borrow_mut() = Some(PendingWrite::Rmw {
            route: next,
            series,
            rmw,
            stage: RmwStage::WriteBack,
        });
        return Ok(write_v1::WriteProgress::Send(read::Planned {
            step: frame_step(next.frame, send),
            covers: all_covers(len),
        }));
    }
    conn.write_poisoned.set(false);
    Ok(write_v1::WriteProgress::Done(
        rmw.bits
            .iter()
            .map(|&bit| {
                let mask = 1u16 << bit;
                if (word & mask != 0) == (rmw.set & mask != 0) {
                    Ok(())
                } else {
                    Err(ItemError::VerificationFailed(format!(
                        "read-back bit {bit:X} does not match the written value"
                    )))
                }
            })
            .collect(),
    ))
}

/// Validate the entire request before assigning a serial or producing a frame.
fn parse_write_run(items: &[write_v1::WriteItem], order: WordOrder) -> Result<WriteRun, String> {
    if items.is_empty() || items.len() > 7168 {
        return Err("write requires a bounded nonempty run".into());
    }
    let mut area = None;
    let mut start = 0;
    let mut next = 0;
    let mut payload = None;
    let mut all_u16 = true;
    let mut kinds = Vec::with_capacity(items.len());
    for item in items {
        let (device, number, kind) = parse_read_address(
            &item.spec.address,
            &item.spec.data_type,
            item.spec.string_words,
        )?;
        kinds.push(kind);
        let points = match (device, kind) {
            (Device::D | Device::W, ReadKind::Numeric(kind)) => {
                all_u16 &= kind == DKind::U16;
                WritePayload::Words(kind.encode(&item.value, order)?)
            }
            (Device::M | Device::Y | Device::L | Device::B, ReadKind::Bit) => {
                let Value::Bool(bit) = item.value else {
                    return Err("bit write requires a bool value".into());
                };
                all_u16 = false;
                WritePayload::Bits(vec![bit])
            }
            _ => return Err("device or type is not supported for SLMP write".into()),
        };
        if let Some(area) = area {
            if area != device || next != number {
                return Err("write items must be one area, consecutive and ascending".into());
            }
        } else {
            area = Some(device);
            start = number;
        }
        next = number
            .checked_add(points.points() as u32)
            .ok_or("write span overflows device address")?;
        match (&mut payload, points) {
            (None, points) => payload = Some(points),
            (Some(WritePayload::Words(words)), WritePayload::Words(next)) => words.extend(next),
            (Some(WritePayload::Bits(bits)), WritePayload::Bits(next)) => bits.extend(next),
            _ => return Err("bit and word items cannot be mixed".into()),
        }
        let total = payload.as_ref().expect("nonempty run").points();
        if total > if device.is_bit() { 7168 } else { 960 } {
            return Err("write run exceeds the device request point limit".into());
        }
    }
    Ok(WriteRun {
        device: area.expect("nonempty run"),
        number: start,
        payload: payload.expect("nonempty run"),
        all_u16,
        kinds,
    })
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
                Device::S => "S",
                Device::Z => "Z",
                Device::RD => "RD",
            }
            .into(),
            // Static capability only; the common setting, SD proof and Host gate still decide writes.
            // Connection policy, model, same-connection SD evidence and Host
            // authorization remain separate from this static type capability.
            writable_area: (matches!(device, Device::D | Device::W)
                && matches!(kind, ReadKind::Numeric(_) | ReadKind::WordBit(_)))
                || (matches!(device, Device::M | Device::Y | Device::L | Device::B)
                    && kind == ReadKind::Bit),
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
                return Err("offset before word 0 bit 0".into());
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
        let kind = if (text.starts_with('D') || text.starts_with('W')) && text.contains('.')
            || [
                "STS", "STC", "TS", "TC", "SS", "SC", "CS", "CC", "M", "X", "Y", "B", "L", "F",
                "V", "SB", "SM",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix))
                && !text.starts_with("SW")
            // Step relay S is a bit device only when a digit follows.
            || text.starts_with('S') && text.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
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
        "SLMP Q/R timer/counter contacts and coils are bits; TN/SN/CN are one-word current values. Standard iQ-R adds LCS/LCC bit contacts/coils, LTN/LSTN/LCN i32 current values, LTS/LTC/LSTS/LSTC flags through four-word current-value reads, and hexadecimal ZR. Q ZR (hexadecimal, except Q00JCPU/Q00UJCPU/Q25PRH) is a range-Unknown read-only one-tag read. Read-only S step relay bits (decimal), Z index words on Q/R and iQ-R RD refresh data words (decimal) have unknown ranges and use one request per tag; LZ needs 0403 random read and is not supported; block-switched R is not supported. Dn.h and Wn.h read one hexadecimal-indexed bit of Dn or hexadecimal Wn; Wn.h uses the same W range proof as a one-word W read. A Dn.h/Wn.h write is a non-atomic read-modify-write of the whole word on standard Q/R (read, write back with only the requested bits changed, read again to confirm) that needs explicit bit-interval allowlist entries; one request writes distinct bits of one word only. V edge relays cannot be registered for periodic collection because a one-scan edge may be missed. New area ranges remain unknown and use one request per tag. Local writes require explicit common-policy D/W numeric, M/Y/L/B bool or D/W word-bit spans, same-connection SD proof, and a matching Host gate. Q25PRH W and bit areas remain read-only"
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
                key: "transport".into(),
                label: "Transport".into(),
                help: Some("TCP or UDP to the PLC's SLMP port. UDP does not retransmit: a lost response fails the request (a lost write response is reported as uncertain). With the 3E frame there is no serial number, so a duplicated or delayed UDP datagram can be taken as the answer to a later read and yield a stale value; prefer 4E over UDP.".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "tcp".into(),
                        label: "TCP".into(),
                    },
                    ChoiceOption {
                        value: "udp".into(),
                        label: "UDP".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("tcp".into())),
                group: FieldGroup::Advanced,
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
                key: "frame".into(),
                label: "SLMP frame".into(),
                help: Some("Binary frame header. 4E echoes a serial number in each response. 3E has no serial number, so responses are matched by order and a new read is refused while an earlier one is unanswered. Over UDP, 3E can take a duplicated or delayed datagram as a later answer and yield a stale value; prefer 4E over UDP.".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "4e".into(),
                        label: "4E binary (serial number)".into(),
                    },
                    ChoiceOption {
                        value: "3e".into(),
                        label: "3E binary (no serial number)".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("4e".into())),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: "network_no".into(),
                label: "Request destination network No.".into(),
                help: Some("SLMP request destination network No. 0 is the connected station. 1-239 routes through a relay to another station; station_no is then required. Ranges are proven by the destination's own 0101 model and SD counts read over this same route.".into()),
                kind: FieldKind::Integer(IntegerSpec {
                    min: Some(0),
                    max: Some(239),
                }),
                required: false,
                default: Some(SettingValue::Integer(0)),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: "station_no".into(),
                label: "Request destination station No.".into(),
                help: Some("Station No. 1-120 of the other station on network_no. Required when network_no is not 0 and must be omitted otherwise. Extension-station (7CH) and control/master station (7DH/7EH) destinations are not supported.".into()),
                kind: FieldKind::Integer(IntegerSpec {
                    min: Some(1),
                    max: Some(120),
                }),
                required: false,
                default: None,
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: "target_cpu".into(),
                label: "Request destination CPU".into(),
                help: Some("Request destination CPU. own is the connected station's (control) CPU; cpu1-cpu4 address multiple CPU No.1-4. Redundant system, multidrop, and control/master station (7DH/7EH) destinations are not supported.".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "own".into(),
                        label: "Own (control CPU)".into(),
                    },
                    ChoiceOption {
                        value: "cpu1".into(),
                        label: "Multiple CPU No.1".into(),
                    },
                    ChoiceOption {
                        value: "cpu2".into(),
                        label: "Multiple CPU No.2".into(),
                    },
                    ChoiceOption {
                        value: "cpu3".into(),
                        label: "Multiple CPU No.3".into(),
                    },
                    ChoiceOption {
                        value: "cpu4".into(),
                        label: "Multiple CPU No.4".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("own".into())),
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
                help: Some("D/W numeric, M/Y/L/B bool and D/W word-bit writes require one explicit connection allowlist, same-connection area SD proof, and a separate Host write gate. Default is disabled. Unknown ranges reject writes.".into()),
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
                help: Some("Comma-separated nonoverlapping D/M/L decimal and W/Y/B hexadecimal ranges, such as D3010-D3013,W1A-W1F,M10-M17. Word-bit ranges such as D100.3-D100.5 or W1A.0-W1B.F (D word decimal, W word uppercase hexadecimal, bit one uppercase hexadecimal digit) are the only entries that allow Dn.h/Wn.h writes; a word range never allows bit writes. Up to 16 ranges in total; each D/W interval is at most 960 words and each M/Y/L/B or word-bit interval at most 7168 bits. D3000 and D3001 are protected. Old saved values keep their meaning. A word-bit write is a read-modify-write and is not atomic: if the PLC program or another station changes another bit of the same word between the read and the write-back, that change is overwritten, and the confirm read checks only the written bits. Allowlist only words whose other bits neither the PLC program nor another station writes. A separate Host gate is required.".into()),
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
            parse_write_run(
                &[item("W1A", 1.0), item("W1B", 65535.0)],
                WordOrder::LowHigh
            ),
            Ok(WriteRun {
                device: Device::W,
                number: 0x1a,
                payload: WritePayload::Words(vec![1, 65535]),
                all_u16: true,
                kinds: vec![ReadKind::Numeric(DKind::U16); 2]
            })
        );
        for run in [
            vec![item("W1A", 1.0), item("D27", 2.0)],
            vec![item("W1B", 1.0), item("W1A", 2.0)],
            vec![item("W1A", 1.0), item("W1C", 2.0)],
            vec![item("W1A", f64::NAN)],
            vec![item("W1A", 65536.0)],
        ] {
            assert!(parse_write_run(&run, WordOrder::LowHigh).is_err());
        }
    }

    #[test]
    fn numeric_write_values_check_width_precision_and_word_order() {
        let cases = [
            (DKind::I16, Value::Int(-1), vec![0xffff]),
            (DKind::U16, Value::Uint(65535), vec![0xffff]),
            (DKind::I32, Value::Int(i32::MIN as i64), vec![0, 0x8000]),
            (DKind::U32, Value::Uint(0x1234_5678), vec![0x5678, 0x1234]),
            (DKind::I64, Value::Int(i64::MIN), vec![0, 0, 0, 0x8000]),
            (DKind::U64, Value::Uint(u64::MAX), vec![0xffff; 4]),
            (DKind::F32, Value::Float(1.5), vec![0, 0x3fc0]),
            (DKind::F64, Value::Float(1.5), vec![0, 0, 0, 0x3ff8]),
        ];
        for (kind, value, low_high) in cases {
            assert_eq!(
                kind.encode(&value, WordOrder::LowHigh),
                Ok(low_high.clone())
            );
            let mut high_low = low_high;
            if high_low.len() > 1 {
                high_low.reverse();
            }
            assert_eq!(kind.encode(&value, WordOrder::HighLow), Ok(high_low));
        }
        for (kind, value) in [
            (DKind::I16, Value::Int(32768)),
            (DKind::U16, Value::Int(-1)),
            (DKind::U16, Value::Int(1)),
            (DKind::I16, Value::Uint(1)),
            (DKind::U32, Value::Uint(u32::MAX as u64 + 1)),
            (DKind::U64, Value::Float(9_007_199_254_740_992.0)),
            (DKind::I64, Value::Float(-9_007_199_254_740_992.0)),
            (DKind::F32, Value::Float(f64::MAX)),
            (DKind::F32, Value::Float(f64::NAN)),
            (DKind::F64, Value::Float(f64::INFINITY)),
        ] {
            assert!(kind.encode(&value, WordOrder::LowHigh).is_err(), "{kind:?}");
        }
    }

    #[test]
    fn whole_write_run_uses_word_width_and_bool_bit_points() {
        let item = |address: &str, data_type: &str, value| write_v1::WriteItem {
            spec: TagSpec {
                address: address.into(),
                data_type: data_type.into(),
                string_words: None,
            },
            value,
        };
        let run = parse_write_run(
            &[
                item("D3010", "i32", Value::Int(-1)),
                item("D3012", "u64", Value::Uint(0x1234_5678_9abc_def0)),
            ],
            WordOrder::LowHigh,
        )
        .unwrap();
        assert_eq!(run.device, Device::D);
        assert_eq!(run.number, 3010);
        assert_eq!(
            run.payload,
            WritePayload::Words(vec![0xffff, 0xffff, 0xdef0, 0x9abc, 0x5678, 0x1234])
        );
        assert!(parse_write_run(
            &[
                item("D3010", "i32", Value::Int(1)),
                item("D3011", "u16", Value::Uint(2)),
            ],
            WordOrder::LowHigh
        )
        .is_err());
        assert_eq!(
            parse_write_run(
                &[
                    item("M10", "bit", Value::Bool(true)),
                    item("M11", "bit", Value::Bool(false)),
                ],
                WordOrder::LowHigh
            )
            .unwrap()
            .payload,
            WritePayload::Bits(vec![true, false])
        );
        for (addr, kind, value) in [
            ("X10", "bit", Value::Bool(true)),
            ("V14", "bit", Value::Bool(true)),
            ("M10", "bit", Value::Uint(1)),
            ("D3010.1", "bit", Value::Bool(true)),
        ] {
            assert!(parse_write_run(&[item(addr, kind, value)], WordOrder::LowHigh).is_err());
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
            // Only the D word bit is a (read-modify-write) writable area here.
            assert_eq!(
                <Component as address::Guest>::parse(text.into(), kind.into(), None)
                    .unwrap()
                    .writable_area,
                text == "D10.F"
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
        // Q ZR is range-Unknown and read-only; Q00J/Q00UJ/Q25PRH have no path.
        let expected = ReadSpan {
            device: Device::ZR,
            start: 26,
            count: 2,
        };
        assert_eq!(
            checked_read_span(&zr, &q, PointCount::Unknown, PointCount::Unknown).unwrap(),
            expected
        );
        for (name, code) in [("Q02UCPU", 0x0263), ("Q06HCPU", 0x0042)] {
            let mut named = model(ModelFamily::StandardQ);
            named.name = name.into();
            named.code = code;
            assert_eq!(
                checked_read_span(&zr, &named, PointCount::Unknown, PointCount::Unknown).unwrap(),
                expected,
                "{name}"
            );
        }
        for (name, code) in [("Q00JCPU", 0x0250), ("Q00UJCPU", 0x0260)] {
            let mut named = model(ModelFamily::StandardQ);
            named.name = name.into();
            named.code = code;
            assert!(
                checked_read_span(&zr, &named, PointCount::Unknown, PointCount::Unknown).is_err(),
                "{name}"
            );
        }
        assert!(checked_read_span(
            &zr,
            &model(ModelFamily::Q25PrhWireOnly),
            PointCount::Unknown,
            PointCount::Unknown
        )
        .is_err());
        assert_eq!(
            read_count(Device::ZR, PointCount::Exact(1), PointCount::Exact(1)),
            PointCount::Unknown
        );
        // ZR never becomes writable.
        let zr_write = write_v1::WriteItem {
            spec: zr.clone(),
            value: Value::Float(1.0),
        };
        assert!(parse_write_run(&[zr_write], WordOrder::LowHigh).is_err());
        assert!(
            !<Component as address::Guest>::parse("ZR1A".into(), "u32".into(), None)
                .unwrap()
                .writable_area
        );
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
    fn s_z_rd_are_read_only_unknown_range_devices() {
        let spec = |address: &str, kind: &str| TagSpec {
            address: address.into(),
            data_type: kind.into(),
            string_words: None,
        };
        let none = (PointCount::Unknown, PointCount::Unknown);
        let q = model(ModelFamily::StandardQ);
        let r = model(ModelFamily::StandardR);
        let q25 = model(ModelFamily::Q25PrhWireOnly);
        // (address, type, canonical, area, device, number, wire count, value words, on Q)
        for (text, kind, canonical, area, device, number, count, words, on_q) in [
            ("S10", "bit", "S10", "S", Device::S, 10, 1, 1, true),
            ("Z3", "u16", "Z3", "Z", Device::Z, 3, 1, 1, true),
            ("z3", "u16", "Z3", "Z", Device::Z, 3, 1, 1, true),
            ("Z3", "u64", "Z3", "Z", Device::Z, 3, 4, 4, true),
            ("RD7", "f32", "RD7", "RD", Device::RD, 7, 2, 2, false),
            ("RD7", "u16", "RD7", "RD", Device::RD, 7, 1, 1, false),
        ] {
            let parsed = <Component as address::Guest>::parse(text.into(), kind.into(), None);
            if text.starts_with('z') {
                assert!(parsed.is_err(), "{text} is case-sensitive");
                continue;
            }
            let parsed = parsed.unwrap();
            assert_eq!(parsed.canonical, canonical);
            assert_eq!(parsed.area, area);
            assert_eq!(parsed.index, Some(number));
            assert_eq!(parsed.word_count, words);
            assert!(!parsed.writable_area, "{text}");
            let tag = spec(text, kind);
            let expected = ReadSpan {
                device,
                start: number as u32,
                count,
            };
            assert_eq!(
                checked_read_span(&tag, &r, none.0, none.1).unwrap(),
                expected
            );
            assert_eq!(checked_read_span(&tag, &q, none.0, none.1).is_ok(), on_q);
            assert!(checked_read_span(&tag, &q25, none.0, none.1).is_err());
            assert_eq!(
                read_count(device, PointCount::Exact(1), PointCount::Exact(1)),
                PointCount::Unknown
            );
        }
        // LZ is rejected with a specific message, ahead of the L bit prefix.
        for (text, kind) in [
            ("LZ0", "i32"),
            ("LZ0", "bit"),
            ("LZ", "i32"),
            ("LZ1", "u16"),
        ] {
            assert_eq!(
                parse_read_address(text, kind, None).unwrap_err(),
                "LZ long index register needs 0403 random read and is not supported"
            );
        }
        // The names that begin with S or L/Z keep their previous parsing.
        for (text, kind, device, number) in [
            ("S10", "bit", Device::S, 10),
            ("SD10", "u16", Device::SD, 10),
            ("SW1A", "u16", Device::SW, 0x1a),
            ("SN3", "u16", Device::SN, 3),
            ("SB1", "bit", Device::SB, 1),
            ("SM0", "bit", Device::SM, 0),
            ("SS0", "bit", Device::SS, 0),
            ("STS0", "bit", Device::STS, 0),
            ("ZR10", "u16", Device::ZR, 0x10),
            ("L5", "bit", Device::L, 5),
        ] {
            let (d, n, _) = parse_read_address(text, kind, None).unwrap();
            assert_eq!((d, n), (device, number), "{text}");
        }
        for (text, kind) in [
            ("S1A", "bit"),
            ("S", "bit"),
            ("S10", "u16"),
            ("Z", "u16"),
            ("Z1A", "u16"),
            ("RD1A", "u16"),
            ("RD", "u16"),
        ] {
            assert!(parse_read_address(text, kind, None).is_err(), "{text}");
        }
        // string_words is not accepted for S.
        assert!(parse_read_address("S1", "bit", Some(1)).is_err());
        let off = |text: &str, kind: &str, n| {
            <Component as address::Guest>::offset(text.into(), kind.into(), None, n)
        };
        assert_eq!(off("S10", "bit", 2).unwrap(), "S12");
        assert_eq!(off("Z3", "u32", 1).unwrap(), "Z5");
        assert_eq!(off("RD7", "u16", 3).unwrap(), "RD10");
        let words = |text: &str| <Component as address::Guest>::offset_words(text.into(), 1);
        assert_eq!(words("S10").unwrap(), "S11");
        assert_eq!(words("Z3").unwrap(), "Z4");
        assert_eq!(words("RD7").unwrap(), "RD8");
        assert_eq!(words("SD10").unwrap(), "SD11");
        assert_eq!(words("SW1A").unwrap(), "SW1B");
        assert_eq!(words("SN3").unwrap(), "SN4");
        // Writes never reach these devices.
        for (text, kind, value) in [
            ("S1", "bit", Value::Bool(true)),
            ("Z1", "u16", Value::Float(1.0)),
            ("RD1", "u16", Value::Float(1.0)),
        ] {
            let item = write_v1::WriteItem {
                spec: spec(text, kind),
                value,
            };
            assert!(
                parse_write_run(&[item], WordOrder::LowHigh).is_err(),
                "{text}"
            );
        }
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
            assert_eq!(parsed.writable_area, matches!(area, "M" | "Y" | "B"));
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
    fn additional_device_syntax_keeps_units_unknown_and_only_l_is_static_writable() {
        let r = model(ModelFamily::StandardR);
        for (text, canonical, area, index) in [
            ("L12", "L12", "L", 12),
            ("F13", "F13", "F", 13),
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
            assert_eq!(parsed.writable_area, area == "L");
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
    fn v_edge_relay_is_rejected_at_address_and_read_boundaries() {
        let reason = "V edge relay cannot be collected reliably: a one-scan edge may be missed";
        let spec = TagSpec {
            address: "V14".into(),
            data_type: "bit".into(),
            string_words: None,
        };
        assert_eq!(parse_read_address("V14", "bit", None), Err(reason.into()));
        assert!(matches!(
            <Component as address::Guest>::parse("V14".into(), "bit".into(), None),
            Err(message) if message == reason
        ));
        assert_eq!(
            <Component as address::Guest>::offset("V14".into(), "bit".into(), None, 1),
            Err(reason.into())
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("V14".into(), 1),
            Err(reason.into())
        );
        assert!(matches!(
            checked_read_span(&spec, &model(ModelFamily::StandardR), PointCount::Unknown, PointCount::Unknown),
            Err(FatalError::InvalidConfig(message)) if message == reason
        ));
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
    }

    #[test]
    fn w_word_bit_uses_hex_word_and_bit_and_the_one_word_w_range_proof() {
        let parsed =
            <Component as address::Guest>::parse("W1a.f".into(), "bit".into(), None).unwrap();
        assert_eq!(parsed.canonical, "W1A.F");
        assert_eq!(parsed.area, "W");
        assert_eq!(parsed.bit, Some(15));
        assert_eq!(parsed.index, Some(0x1A));
        assert_eq!(parsed.word_count, 1);
        assert!(
            parsed.writable_area,
            "W word bits are read-modify-write targets"
        );
        assert_eq!(
            parse_read_address("WFFFFFFFF.0", "bit", None),
            Ok((Device::W, u32::MAX, ReadKind::WordBit(0)))
        );
        for (text, kind, words) in [
            ("W1G.0", "bit", None),
            ("W.0", "bit", None),
            ("W+1.0", "bit", None),
            ("W1.10", "bit", None),
            ("W1.", "bit", None),
            ("W1.0", "u16", None),
            ("W1.0", "bit", Some(1)),
            ("W100000000.0", "bit", None),
        ] {
            assert!(
                parse_read_address(text, kind, words).is_err(),
                "{text}/{kind}"
            );
        }
        assert_eq!(
            <Component as address::Guest>::offset("W1F.F".into(), "bit".into(), None, 1).unwrap(),
            "W20.0"
        );
        assert_eq!(
            <Component as address::Guest>::offset("W0.0".into(), "bit".into(), None, -1),
            Err("offset before word 0 bit 0".into())
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("W1F.3".into(), 1).unwrap(),
            "W20.3"
        );

        let r = model(ModelFamily::StandardR);
        let q = model(ModelFamily::StandardQ);
        let last = TagSpec {
            address: "W1F.F".into(),
            data_type: "bit".into(),
            string_words: None,
        };
        let past = TagSpec {
            address: "W20.0".into(),
            ..last.clone()
        };
        for family in [&r, &q] {
            assert_eq!(
                checked_read_span(&last, family, PointCount::Unknown, PointCount::Exact(32))
                    .unwrap(),
                ReadSpan {
                    device: Device::W,
                    start: 31,
                    count: 1
                }
            );
            assert!(
                checked_read_span(&past, family, PointCount::Unknown, PointCount::Exact(32))
                    .is_err()
            );
        }
        // As with one-word W reads, standard Q needs the SD303 count and
        // Q25PRH the guaranteed prefix; standard R reads an unknown count.
        assert!(checked_read_span(&last, &q, PointCount::Unknown, PointCount::Unknown).is_err());
        assert!(checked_read_span(&last, &r, PointCount::Unknown, PointCount::Unknown).is_ok());
        let q25 = model(ModelFamily::Q25PrhWireOnly);
        assert!(
            checked_read_span(&last, &q25, PointCount::Unknown, PointCount::AtLeast(32)).is_ok()
        );
        assert!(
            checked_read_span(&past, &q25, PointCount::Unknown, PointCount::AtLeast(32)).is_err()
        );
        assert!(matches!(
            ReadKind::WordBit(0xA).decode_words(&[0x0400], WordOrder::HighLow),
            Ok(Value::Bool(true))
        ));
        assert!(matches!(
            ReadKind::WordBit(0xA).decode_words(&[0xFBFF], WordOrder::LowHigh),
            Ok(Value::Bool(false))
        ));
        assert!(parse_write_run(
            &[write_v1::WriteItem {
                spec: last.clone(),
                value: Value::Bool(true),
            }],
            WordOrder::LowHigh
        )
        .is_err());
    }

    #[test]
    fn q_w_region_follows_the_internal_count_the_total_and_w10000() {
        use QWRegion::{Extended, Internal, Outside, Unsupported};
        let exact = PointCount::Exact(0x2000);
        let cases = [
            // (number, words, region) with WC = 0x2000 and T = 0x3000.
            (0x1fff, 1, Internal),
            (0x1ffe, 2, Internal),
            (0x2000, 1, Extended),
            (0x2fff, 1, Extended),
            (0x2ffe, 2, Extended),
            (0x2ffc, 4, Extended),
            (0x1fff, 2, Unsupported),
            (0x1ffd, 4, Unsupported),
            (0x2fff, 2, Outside),
            (0x3000, 1, Outside),
            (0xffff_ffff, 2, Outside),
        ];
        for (number, words, region) in cases {
            assert_eq!(
                classify_q_w(exact, Some(0x3000), number, words),
                Some(region),
                "W{number:X} x{words}"
            );
        }
        // T above W10000H: outside (last >= T) wins over unsupported.
        for (number, words, region) in [
            (0xfffe, 2, Extended),
            (0xffff, 1, Extended),
            (0xffff, 2, Unsupported),
            (0x10000, 1, Unsupported),
            (0x11fff, 1, Unsupported),
            (0x11fff, 2, Outside),
            (0x12000, 1, Outside),
        ] {
            assert_eq!(
                classify_q_w(exact, Some(0x12000), number, words),
                Some(region),
                "W{number:X} x{words}"
            );
        }
        // T == WC has no extended area: tags reaching WC are outside.
        assert_eq!(classify_q_w(exact, Some(0x2000), 0x1fff, 1), Some(Internal));
        assert_eq!(classify_q_w(exact, Some(0x2000), 0x1fff, 2), Some(Outside));
        assert_eq!(classify_q_w(exact, Some(0x2000), 0x2000, 1), Some(Outside));
        // Unusable totals keep the internal-prefix rules.
        for (count, total) in [
            (exact, Some(0x1fff)),
            (exact, Some(0)),
            (exact, None),
            (PointCount::AtLeast(32768), Some(0xa000)),
            (PointCount::Unknown, Some(0xa000)),
        ] {
            assert_eq!(classify_q_w(count, total, 0x2000, 1), None);
        }
    }

    #[test]
    fn extended_q_w_reads_verify_inside_and_error_for_outside_and_unsupported() {
        let q = TypeName {
            name: "Q02UCPU".into(),
            code: 0x0263,
            ..model(ModelFamily::StandardQ)
        };
        let tag = |address: &str, data_type: &str| TagSpec {
            address: address.into(),
            data_type: data_type.into(),
            string_words: None,
        };
        let read = |spec: &TagSpec, total: Option<u32>| {
            checked_read_span_ext(
                spec,
                &q,
                PointCount::Unknown,
                PointCount::Exact(0x2000),
                total,
            )
        };
        let outside = "address exceeds verified configured count";
        let unsupported =
            "W reads require a confirmed iQ-R count or a verified standard Q internal W span";
        let message = |result: Result<ReadSpan, FatalError>| match result {
            Err(FatalError::InvalidConfig(message)) => message,
            other => panic!("expected InvalidConfig, got {other:?}"),
        };
        for (address, data_type, start, count) in [
            ("W1FFF", "u16", 0x1fff, 1),
            ("W2000", "u16", 0x2000, 1),
            ("W2FFF", "u16", 0x2fff, 1),
            ("W2FFE", "u32", 0x2ffe, 2),
            ("W2FFC", "f64", 0x2ffc, 4),
        ] {
            assert_eq!(
                read(&tag(address, data_type), Some(0x3000)).unwrap(),
                ReadSpan {
                    device: Device::W,
                    start,
                    count,
                },
                "{address}"
            );
        }
        for (address, data_type) in [("W1FFF", "u32"), ("W1FFD", "u64")] {
            assert_eq!(
                message(read(&tag(address, data_type), Some(0x3000))),
                unsupported,
                "{address} crosses WC"
            );
        }
        for (address, data_type) in [("W2FFF", "u32"), ("W3000", "u16"), ("W4000", "u16")] {
            assert_eq!(
                message(read(&tag(address, data_type), Some(0x3000))),
                outside,
                "{address}"
            );
        }
        // Without a usable total the old rule applies: W2000 is unsupported.
        for total in [None, Some(0x1000)] {
            assert_eq!(message(read(&tag("W2000", "u16"), total)), unsupported);
        }
        assert_eq!(
            message(read(&tag("W2000", "u16"), Some(0x2000))),
            outside,
            "T == WC leaves no extended area"
        );
        // T above W10000H.
        assert!(read(&tag("WFFFF", "u16"), Some(0x12000)).is_ok());
        for address in ["W10000", "W11FFF"] {
            assert_eq!(
                message(read(&tag(address, "u16"), Some(0x12000))),
                unsupported,
                "{address}"
            );
        }
        assert_eq!(
            message(read(&tag("WFFFF", "u32"), Some(0x12000))),
            unsupported
        );
        assert_eq!(message(read(&tag("W12000", "u16"), Some(0x12000))), outside);
        // Non-Q models never use the total.
        let r = model(ModelFamily::StandardR);
        assert!(checked_read_span_ext(
            &tag("W2FFF", "u16"),
            &r,
            PointCount::Unknown,
            PointCount::Unknown,
            Some(0x3000)
        )
        .is_ok());
        let q25 = model(ModelFamily::Q25PrhWireOnly);
        assert!(checked_read_span_ext(
            &tag("W2000", "u16"),
            &q25,
            PointCount::Unknown,
            PointCount::Exact(0x2000),
            Some(0x3000)
        )
        .is_err());
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
            <Component as address::Guest>::parse("D10".into(), "i32".into(), None)
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
    fn transport_setting_accepts_tcp_or_udp_and_defaults_to_tcp() {
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
            key: "transport".into(),
            value: "tcp".into(),
        });
        assert!(validate_settings(&settings).is_empty());
        settings[2].value = "udp".into();
        assert!(validate_settings(&settings).is_empty());
        for bad in ["UDP", "serial", "serial_over_tcp", ""] {
            settings[2].value = bad.into();
            let errors = validate_settings(&settings);
            assert!(
                errors
                    .iter()
                    .any(|e| e.key == "transport" && e.message == "transport must be tcp or udp"),
                "{bad:?}"
            );
        }
        let schema = <Component as config::Guest>::schema();
        let field = schema
            .iter()
            .find(|field| field.key == "transport")
            .expect("transport field");
        assert!(
            matches!(&field.default, Some(config::SettingValue::Text(value)) if value == "tcp")
        );
        assert!(!field.required);
        assert!(matches!(&field.group, config::FieldGroup::Advanced));
        assert!(matches!(&field.kind, config::FieldKind::Choice(options)
            if options.iter().map(|option| option.value.as_str()).collect::<Vec<_>>() == ["tcp", "udp"]));
        let help = field.help.as_deref().expect("help");
        assert!(help.contains("3E") && help.contains("stale") && help.contains("4E"));
        let frame = schema.iter().find(|f| f.key == "frame").expect("frame");
        assert!(frame.help.as_deref().is_some_and(|h| h.contains("UDP")));
    }

    #[test]
    fn frame_setting_accepts_4e_or_3e_and_defaults_to_4e() {
        let mut settings = vec![
            session::Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            session::Setting {
                key: "port".into(),
                value: "5000".into(),
            },
            session::Setting {
                key: "frame".into(),
                value: "4e".into(),
            },
        ];
        assert!(validate_settings(&settings).is_empty());
        settings[2].value = "3e".into();
        assert!(validate_settings(&settings).is_empty());
        for bad in ["3E", "1e", "ascii", ""] {
            settings[2].value = bad.into();
            let errors = validate_settings(&settings);
            assert!(
                errors
                    .iter()
                    .any(|e| e.key == "frame" && e.message == "frame must be 4e or 3e"),
                "{bad:?}"
            );
        }
        let schema = <Component as config::Guest>::schema();
        let field = schema
            .iter()
            .find(|field| field.key == "frame")
            .expect("frame field");
        assert!(matches!(&field.default, Some(config::SettingValue::Text(value)) if value == "4e"));
        assert!(!field.required);
        assert!(matches!(&field.group, config::FieldGroup::Advanced));
        assert!(matches!(&field.kind, config::FieldKind::Choice(options)
            if options.iter().map(|option| option.value.as_str()).collect::<Vec<_>>() == ["4e", "3e"]));
        assert!(field
            .help
            .as_deref()
            .is_some_and(|help| help.contains("by order")));
        assert_eq!(Route::default().frame, Frame::E4);
    }

    fn route_settings(extra: &[(&str, &str)]) -> Vec<session::Setting> {
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
        settings.extend(extra.iter().map(|(key, value)| session::Setting {
            key: (*key).into(),
            value: (*value).into(),
        }));
        settings
    }

    fn route_messages(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        validate_settings(&route_settings(extra))
            .into_iter()
            .map(|e| (e.key, e.message))
            .collect()
    }

    #[test]
    fn route_settings_accept_valid_combinations() {
        let cases: &[&[(&str, &str)]] = &[
            &[],
            &[("network_no", "0")],
            &[("network_no", "1"), ("station_no", "1")],
            &[("network_no", "239"), ("station_no", "120")],
            &[("target_cpu", "own")],
            &[("target_cpu", "cpu1")],
            &[("target_cpu", "cpu2")],
            &[("target_cpu", "cpu3")],
            &[("target_cpu", "cpu4")],
            &[
                ("network_no", "2"),
                ("station_no", "5"),
                ("target_cpu", "cpu4"),
            ],
            &[("network_no", "0"), ("target_cpu", "cpu1")],
        ];
        for case in cases {
            assert!(route_messages(case).is_empty(), "{case:?}");
        }
    }

    #[test]
    fn route_settings_reject_invalid_values_with_exact_messages() {
        const NETWORK: &str = "network_no must be a decimal number from 0 to 239";
        const STATION: &str = "station_no must be a decimal number from 1 to 120";
        for bad in ["240", "-1", "+1", "01", " 1", "1 ", "", "abc", "256", "1.0"] {
            assert_eq!(
                route_messages(&[("network_no", bad)]),
                [("network_no".to_string(), NETWORK.to_string())],
                "{bad:?}"
            );
        }
        for bad in ["0", "121", "125", "126", "255", "-1", "01", " 1", "", "abc"] {
            assert_eq!(
                route_messages(&[("network_no", "1"), ("station_no", bad)]),
                [("station_no".to_string(), STATION.to_string())],
                "{bad:?}"
            );
        }
        for bad in ["03E0", "CPU1", "standby", "", "cpu5", "cpu0"] {
            assert_eq!(
                route_messages(&[("target_cpu", bad)]),
                [(
                    "target_cpu".to_string(),
                    "target_cpu must be own, cpu1, cpu2, cpu3 or cpu4".to_string()
                )],
                "{bad:?}"
            );
        }
    }

    #[test]
    fn route_settings_cross_field_rules() {
        assert_eq!(
            route_messages(&[("network_no", "3")]),
            [(
                "station_no".to_string(),
                "station_no is required when network_no is not 0".to_string()
            )]
        );
        let omitted = "station_no must be omitted for the connected station (network_no 0)";
        for extra in [
            &[("station_no", "5")][..],
            &[("network_no", "0"), ("station_no", "5")][..],
        ] {
            assert_eq!(
                route_messages(extra),
                [("station_no".to_string(), omitted.to_string())]
            );
        }
        // Cross-field errors are suppressed when a value is itself invalid.
        assert_eq!(
            route_messages(&[("network_no", "x"), ("station_no", "5")]).len(),
            1
        );
        assert_eq!(
            route_messages(&[("network_no", "3"), ("station_no", "0")]).len(),
            1
        );
    }

    #[test]
    fn route_settings_schema_declares_keys() {
        let schema = <Component as config::Guest>::schema();
        let find = |key: &str| {
            schema
                .iter()
                .find(|field| field.key == key)
                .unwrap_or_else(|| panic!("{key} field"))
        };
        let network = find("network_no");
        assert!(matches!(
            &network.kind,
            config::FieldKind::Integer(spec) if spec.min == Some(0) && spec.max == Some(239)
        ));
        assert!(matches!(
            &network.default,
            Some(config::SettingValue::Integer(0))
        ));
        assert!(!network.required);
        let station = find("station_no");
        assert!(matches!(
            &station.kind,
            config::FieldKind::Integer(spec) if spec.min == Some(1) && spec.max == Some(120)
        ));
        assert!(station.default.is_none());
        assert!(!station.required);
        let cpu = find("target_cpu");
        assert!(matches!(&cpu.kind, config::FieldKind::Choice(options)
            if options.iter().map(|option| option.value.as_str()).collect::<Vec<_>>()
                == ["own", "cpu1", "cpu2", "cpu3", "cpu4"]));
        assert!(matches!(&cpu.default, Some(config::SettingValue::Text(value)) if value == "own"));
        for field in [network, station, cpu] {
            assert!(matches!(&field.group, config::FieldGroup::Advanced));
        }
    }

    #[test]
    fn route_fields_follow_settings_and_default_to_own_station() {
        assert_eq!(route_fields(&route_settings(&[])), (0, 0xFF, 0x03FF));
        assert_eq!(
            route_fields(&route_settings(&[("target_cpu", "cpu1")])),
            (0, 0xFF, 0x03E0)
        );
        assert_eq!(
            route_fields(&route_settings(&[
                ("network_no", "2"),
                ("station_no", "5"),
                ("target_cpu", "cpu2"),
            ])),
            (2, 5, 0x03E1)
        );
        assert_eq!(
            route_fields(&route_settings(&[
                ("network_no", "239"),
                ("station_no", "120"),
                ("target_cpu", "cpu4"),
            ])),
            (239, 120, 0x03E3)
        );
    }

    #[test]
    fn create_refuses_invalid_route() {
        for extra in [
            &[("network_no", "240")][..],
            &[("network_no", "2")][..],
            &[("station_no", "5")][..],
            &[("target_cpu", "standby")][..],
        ] {
            assert!(matches!(
                <SlmpConnection as session::GuestConnection>::create(route_settings(extra)),
                Err(FatalError::InvalidConfig(_))
            ));
        }
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

    const RMW_ALLOWLIST: &str = "D100.0-D100.5,D200-D210,D3002.0-D3002.F,W1A.0-W1A.F,W40.0-W40.F";

    fn rmw_conn(
        extra: &[(&str, &str)],
        family: ModelFamily,
        d_count: PointCount,
        w_count: PointCount,
    ) -> SlmpConnection {
        let mut settings = vec![
            ("d_u16_write_mode", "enabled"),
            ("d_u16_write_allowlist", RMW_ALLOWLIST),
        ];
        for (key, value) in extra {
            settings.retain(|(existing, _)| existing != key);
            settings.push((key, value));
        }
        let conn = SlmpConnection::new(&route_settings(&settings)).expect("valid settings");
        *conn.phase.borrow_mut() = Phase::Ready {
            model: model(family),
            count: d_count,
        };
        conn.w_count.set(w_count);
        conn
    }

    fn bit_write(address: &str, value: bool) -> write_v1::WriteItem {
        write_v1::WriteItem {
            spec: TagSpec {
                address: address.into(),
                data_type: "bit".into(),
                string_words: None,
            },
            value: Value::Bool(value),
        }
    }

    fn word_write(address: &str, value: u64) -> write_v1::WriteItem {
        write_v1::WriteItem {
            spec: TagSpec {
                address: address.into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Uint(value),
        }
    }

    fn sent(progress: Result<write_v1::WriteProgress, FatalError>, len: usize) -> Vec<u8> {
        let Ok(write_v1::WriteProgress::Send(plan)) = progress else {
            panic!("RMW step must send")
        };
        assert_eq!(plan.covers, all_covers(len));
        assert_eq!(plan.step.max_len, 64);
        plan.step.send
    }

    fn done(progress: Result<write_v1::WriteProgress, FatalError>) -> Vec<Result<(), ItemError>> {
        let Ok(write_v1::WriteProgress::Done(results)) = progress else {
            panic!("RMW must finish")
        };
        results
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

    fn assert_rejected(conn: &SlmpConnection, items: &[write_v1::WriteItem], why: &str) {
        let serial = conn.next_serial.get();
        let Ok(write_v1::WriteProgress::Done(results)) = write_next_on(conn, items.to_vec(), None)
        else {
            panic!("{why}: must be refused before send")
        };
        assert_eq!(results.len(), items.len(), "{why}");
        assert!(
            results
                .iter()
                .all(|result| matches!(result, Err(ItemError::Rejected(_)))),
            "{why}"
        );
        assert_eq!(conn.next_serial.get(), serial, "{why}: nothing sent");
        assert!(conn.pending_write.borrow().is_none(), "{why}");
    }

    #[test]
    fn word_bit_mask_keeps_other_bits_and_mixes_set_and_clear() {
        let rmw = parse_bit_rmw(&[bit_write("D100.3", true), bit_write("D100.5", false)])
            .unwrap()
            .unwrap();
        assert_eq!(
            rmw,
            BitRmw {
                device: Device::D,
                number: 100,
                bits: vec![3, 5],
                set: 0x0008,
                clear: 0x0020,
            }
        );
        assert_eq!(rmw.merge(0xa520), 0xa508);
        assert_eq!(rmw.merge(0xffff), 0xffdf);
        assert_eq!(rmw.merge(0), 0x0008);
        let w = parse_bit_rmw(&[bit_write("W1A.F", true), bit_write("W1a.0", false)])
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.device, w.number, w.set, w.clear),
            (Device::W, 0x1a, 0x8000, 1)
        );
        assert_eq!(w.merge(0x7ff1), 0xfff0);
        // Requests without any word bit keep the ordinary run path.
        assert_eq!(parse_bit_rmw(&[word_write("D100", 1)]), Ok(None));
        assert_eq!(parse_bit_rmw(&[bit_write("M10", true)]), Ok(None));
        for items in [
            vec![bit_write("D100.3", true), bit_write("D100.3", true)],
            vec![bit_write("D100.3", true), bit_write("D100.3", false)],
            vec![bit_write("D100.3", true), bit_write("D101.3", true)],
            vec![bit_write("D100.3", true), bit_write("W64.3", true)],
            vec![bit_write("D100.3", true), word_write("D100", 1)],
            vec![word_write("D101", 1), bit_write("D100.3", true)],
            vec![bit_write("D100.3", true), bit_write("M10", true)],
            vec![bit_write("D100.3", true), bit_write("X10", true)],
            vec![write_v1::WriteItem {
                value: Value::Uint(1),
                ..bit_write("D100.3", true)
            }],
        ] {
            assert!(parse_bit_rmw(&items).is_err());
        }
    }

    #[test]
    fn r_4e_word_bit_write_reads_writes_back_and_confirms_exact_bytes() {
        let conn = rmw_conn(
            &[],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(64),
        );
        let items = vec![bit_write("D100.3", true), bit_write("D100.5", false)];
        let read = |serial: u8| {
            vec![
                0x54, 0, serial, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 0x10, 0, 1, 4, 2, 0, 100, 0,
                0, 0, 0xa8, 0, 1, 0,
            ]
        };
        assert_eq!(sent(write_next_on(&conn, items.clone(), None), 2), read(1));
        assert!(!conn.write_poisoned.get());
        let step2 = sent(
            write_next_on(
                &conn,
                items.clone(),
                Some(reply(&conn, 1, 0, &[0x20, 0xa5])),
            ),
            2,
        );
        assert_eq!(
            step2,
            [
                0x54, 0, 2, 0, 0, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 0x10, 0, 1, 0x14, 2, 0, 100, 0, 0,
                0, 0xa8, 0, 1, 0, 0x08, 0xa5
            ]
        );
        assert!(conn.write_poisoned.get(), "write-back outcome is uncertain");
        assert_eq!(
            sent(
                write_next_on(&conn, items.clone(), Some(reply(&conn, 2, 0, &[]))),
                2
            ),
            read(3)
        );
        let results = done(write_next_on(
            &conn,
            items.clone(),
            Some(reply(&conn, 3, 0, &[0x08, 0xa5])),
        ));
        assert!(matches!(results.as_slice(), [Ok(()), Ok(())]));
        assert!(!conn.write_poisoned.get());
        assert!(conn.pending_write.borrow().is_none());
        // R W uses the exact iQ-R W count.
        assert_eq!(
            sent(
                write_next_on(&conn, vec![bit_write("W1A.0", true)], None),
                1
            )[19..25],
            [0x1a, 0, 0, 0, 0xb4, 0]
        );
    }

    #[test]
    fn q_3e_routed_word_bit_write_uses_the_route_and_issue_order() {
        let conn = rmw_conn(
            &[
                ("frame", "3e"),
                ("network_no", "2"),
                ("station_no", "5"),
                ("target_cpu", "cpu2"),
            ],
            ModelFamily::StandardQ,
            PointCount::Exact(5000),
            PointCount::Exact(0x20),
        );
        let items = vec![bit_write("W1A.0", true), bit_write("W1A.F", true)];
        let read = [
            0x50, 0, 2, 5, 0xe1, 3, 0, 12, 0, 0x10, 0, 1, 4, 0, 0, 0x1a, 0, 0, 0xb4, 1, 0,
        ];
        assert_eq!(sent(write_next_on(&conn, items.clone(), None), 2), read);
        let step2 = sent(
            write_next_on(
                &conn,
                items.clone(),
                Some(reply(&conn, 0, 0, &[0x00, 0x00])),
            ),
            2,
        );
        assert_eq!(
            step2,
            [
                0x50, 0, 2, 5, 0xe1, 3, 0, 14, 0, 0x10, 0, 1, 0x14, 0, 0, 0x1a, 0, 0, 0xb4, 1, 0,
                0x01, 0x80
            ]
        );
        assert_eq!(
            sent(
                write_next_on(&conn, items.clone(), Some(reply(&conn, 0, 0, &[]))),
                2
            ),
            read
        );
        let results = done(write_next_on(
            &conn,
            items,
            Some(reply(&conn, 0, 0, &[0x01, 0x80])),
        ));
        assert!(matches!(results.as_slice(), [Ok(()), Ok(())]));
        // A 4E response to a 3E connection is a framing error.
        let conn = rmw_conn(
            &[("frame", "3e")],
            ModelFamily::StandardQ,
            PointCount::Exact(5000),
            PointCount::Exact(0x20),
        );
        sent(
            write_next_on(&conn, vec![bit_write("W1A.0", true)], None),
            1,
        );
        let mut e4 = vec![0xd4, 0, 1, 0, 0, 0];
        e4.extend_from_slice(&reply(&conn, 0, 0, &[0, 0])[2..]);
        assert!(matches!(
            write_next_on(&conn, vec![bit_write("W1A.0", true)], Some(e4)),
            Err(FatalError::Framing(_))
        ));
    }

    #[test]
    fn word_bit_end_codes_map_per_step() {
        let r = || {
            rmw_conn(
                &[],
                ModelFamily::StandardR,
                PointCount::Exact(200),
                PointCount::Exact(64),
            )
        };
        let items = vec![bit_write("D100.3", true), bit_write("D100.5", false)];
        let device = |results: &[Result<(), ItemError>]| {
            results.len() == 2
                && results.iter().all(|result| {
                    matches!(result, Err(ItemError::Device(error))
                        if error.code == 0xc051 && error.message == "SLMP end code 0xC051")
                })
        };
        // (1) refused: nothing written, nothing more sent, connection usable.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 2);
        let results = done(write_next_on(
            &conn,
            items.clone(),
            Some(reply(&conn, 1, 0xc051, &[])),
        ));
        assert!(device(&results));
        assert!(!conn.write_poisoned.get());
        assert!(conn.pending_write.borrow().is_none());
        assert_eq!(sent(write_next_on(&conn, items.clone(), None), 2)[2], 2);
        // (2) refused.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 2);
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
            2,
        );
        let results = done(write_next_on(
            &conn,
            items.clone(),
            Some(reply(&conn, 2, 0xc051, &[])),
        ));
        assert!(device(&results));
        assert!(!conn.write_poisoned.get());
        assert!(conn.pending_write.borrow().is_none());
        // (3) refused: the write-back was accepted, so the outcome is unverified.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 2);
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
            2,
        );
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 2, 0, &[]))),
            2,
        );
        let results = done(write_next_on(
            &conn,
            items.clone(),
            Some(reply(&conn, 3, 0xc051, &[])),
        ));
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| matches!(result,
            Err(ItemError::VerificationFailed(message)) if message.contains("0xC051"))));
        assert!(!conn.write_poisoned.get());
    }

    #[test]
    fn word_bit_confirm_mismatch_fails_only_the_mismatched_item() {
        let conn = rmw_conn(
            &[],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(64),
        );
        let items = vec![bit_write("D100.3", true), bit_write("D100.5", false)];
        sent(write_next_on(&conn, items.clone(), None), 2);
        sent(
            write_next_on(
                &conn,
                items.clone(),
                Some(reply(&conn, 1, 0, &[0x20, 0xa5])),
            ),
            2,
        );
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 2, 0, &[]))),
            2,
        );
        // Bit 3 set as written; bit 5 was set again by someone else.
        let results = done(write_next_on(
            &conn,
            items,
            Some(reply(&conn, 3, 0, &[0x28, 0xa5])),
        ));
        assert!(matches!(
            results.as_slice(),
            [Ok(()), Err(ItemError::VerificationFailed(_))]
        ));
        assert!(!conn.write_poisoned.get());
    }

    #[test]
    fn malformed_word_bit_responses_are_fatal_and_poison_after_the_write_back() {
        let r = || {
            rmw_conn(
                &[("network_no", "1"), ("station_no", "2")],
                ModelFamily::StandardR,
                PointCount::Exact(200),
                PointCount::Exact(64),
            )
        };
        let items = vec![bit_write("D100.3", true)];
        let poisoned = |conn: &SlmpConnection| {
            assert!(conn.write_poisoned.get());
            assert!(conn.pending_write.borrow().is_none());
            assert!(matches!(
                write_next_on(conn, vec![bit_write("D100.3", true)], None),
                Err(FatalError::Protocol(_))
            ));
        };
        // Step (1): a short read is fatal.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 1);
        assert!(matches!(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0]))),
            Err(FatalError::Framing(_))
        ));
        poisoned(&conn);
        // Step (2): an own-station route echo on a routed connection.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 1);
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
            1,
        );
        let mut wrong_route = reply(&conn, 2, 0, &[]);
        wrong_route[6..11].copy_from_slice(&[0, 0xff, 0xff, 3, 0]);
        assert!(matches!(
            write_next_on(&conn, items.clone(), Some(wrong_route)),
            Err(FatalError::Framing(_))
        ));
        poisoned(&conn);
        // Step (2): a data-bearing write ACK is fatal.
        let conn = r();
        sent(write_next_on(&conn, items.clone(), None), 1);
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
            1,
        );
        assert!(write_next_on(&conn, items.clone(), Some(reply(&conn, 2, 0, &[0, 0]))).is_err());
        poisoned(&conn);
        // Step (3): a stale serial and a short read are fatal.
        for confirm in [&[8u8, 0][..], &[8][..]] {
            let conn = r();
            sent(write_next_on(&conn, items.clone(), None), 1);
            sent(
                write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
                1,
            );
            sent(
                write_next_on(&conn, items.clone(), Some(reply(&conn, 2, 0, &[]))),
                1,
            );
            let serial = if confirm.len() == 2 { 2 } else { 3 };
            assert!(matches!(
                write_next_on(&conn, items.clone(), Some(reply(&conn, serial, 0, confirm))),
                Err(FatalError::Framing(_))
            ));
            poisoned(&conn);
        }
    }

    #[test]
    fn word_bit_steps_refuse_changed_items_and_unanswered_resends() {
        let conn = rmw_conn(
            &[],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(64),
        );
        let items = vec![bit_write("D100.3", true), bit_write("D100.5", false)];
        sent(write_next_on(&conn, items.clone(), None), 2);
        assert!(
            matches!(
                write_next_on(&conn, items.clone(), None),
                Err(FatalError::Protocol(_))
            ),
            "an RMW in progress cannot restart"
        );
        sent(
            write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
            2,
        );
        for changed in [
            vec![bit_write("D100.3", true), bit_write("D100.5", true)],
            vec![bit_write("D100.3", true)],
            vec![bit_write("D100.5", false), bit_write("D100.3", true)],
            vec![word_write("D100", 8)],
        ] {
            let conn = rmw_conn(
                &[],
                ModelFamily::StandardR,
                PointCount::Exact(200),
                PointCount::Exact(64),
            );
            sent(write_next_on(&conn, items.clone(), None), 2);
            sent(
                write_next_on(&conn, items.clone(), Some(reply(&conn, 1, 0, &[0, 0]))),
                2,
            );
            assert!(matches!(
                write_next_on(&conn, changed, Some(reply(&conn, 2, 0, &[]))),
                Err(FatalError::Protocol(_))
            ));
            assert!(conn.write_poisoned.get());
        }
        // A word write response cannot be consumed as an RMW step either.
        let conn = rmw_conn(
            &[],
            ModelFamily::StandardR,
            PointCount::Exact(300),
            PointCount::Exact(64),
        );
        sent(write_next_on(&conn, vec![word_write("D200", 1)], None), 1);
        assert!(matches!(
            write_next_on(&conn, items, Some(reply(&conn, 1, 0, &[]))),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn word_bit_writes_are_refused_before_send_without_policy_model_or_range() {
        let ready = |family, d, w| rmw_conn(&[], family, d, w);
        let r = ready(
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        for (items, why) in [
            (vec![bit_write("D100.6", true)], "outside the bit allowlist"),
            (
                vec![bit_write("D100.3", true), bit_write("D100.6", true)],
                "one bit outside",
            ),
            (vec![bit_write("D200.0", true)], "word interval only"),
            (vec![bit_write("D201.0", true)], "word interval only"),
            (
                vec![bit_write("D100.3", true), word_write("D101", 1)],
                "mixed with a word",
            ),
            (
                vec![bit_write("D100.3", true), bit_write("W1A.0", true)],
                "two words",
            ),
            (
                vec![bit_write("D100.3", true), bit_write("M10", true)],
                "mixed with a bit device",
            ),
            (
                vec![bit_write("D100.3", true), bit_write("D100.3", false)],
                "duplicate bit",
            ),
            (vec![bit_write("D3000.0", true)], "D3000 is protected"),
            (vec![bit_write("D3001.F", true)], "D3001 is protected"),
            (vec![bit_write("SD10.0", true)], "not a D/W word bit"),
        ] {
            assert_rejected(&r, &items, why);
        }
        // D3002 is allowlisted but beyond the R D count of 200.
        assert_rejected(&r, &[bit_write("D3002.0", true)], "out of range");
        let small_r = ready(
            ModelFamily::StandardR,
            PointCount::Exact(100),
            PointCount::Exact(0x41),
        );
        assert_rejected(
            &small_r,
            &[bit_write("D100.3", true)],
            "D100 out of R range",
        );
        let small_w = ready(
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x40),
        );
        assert_rejected(
            &small_w,
            &[bit_write("W40.0", true)],
            "W40 out of R W range",
        );
        let unknown_r = ready(
            ModelFamily::StandardR,
            PointCount::Unknown,
            PointCount::Unknown,
        );
        assert_rejected(&unknown_r, &[bit_write("D100.3", true)], "SD refused");
        assert_rejected(&unknown_r, &[bit_write("W1A.3", true)], "SD refused");
        // Standard Q D has no write proof (as for numeric D); Q W uses SD303.
        let q = ready(
            ModelFamily::StandardQ,
            PointCount::Exact(5000),
            PointCount::Exact(0x20),
        );
        assert_rejected(&q, &[bit_write("D100.3", true)], "Q D bit");
        assert_rejected(&q, &[word_write("D200", 1)], "Q D numeric");
        assert_rejected(&q, &[bit_write("W40.0", true)], "Q W past SD303");
        // The extended W total is never a write grant.
        q.q_w_total.set(Some(0x100));
        assert_rejected(&q, &[bit_write("W40.0", true)], "extended Q W");
        assert_eq!(
            sent(write_next_on(&q, vec![bit_write("W1A.3", true)], None), 1)[19..25],
            [0x1a, 0, 0, 0xb4, 1, 0]
        );
        let r = ready(
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        sent(write_next_on(&r, vec![bit_write("D100.3", true)], None), 1);
        let r = ready(
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        sent(write_next_on(&r, vec![bit_write("W40.F", false)], None), 1);
        for family in [ModelFamily::Q25PrhWireOnly, ModelFamily::Unknown] {
            let conn = ready(family, PointCount::Exact(5000), PointCount::Exact(0x100));
            conn.q25prh_internal_d_count.set(PointCount::Exact(5000));
            assert_rejected(&conn, &[bit_write("D100.3", true)], "Q25PRH/unknown D");
            assert_rejected(&conn, &[bit_write("W1A.3", true)], "Q25PRH/unknown W");
        }
        let disabled = rmw_conn(
            &[("d_u16_write_mode", "disabled")],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        assert_rejected(
            &disabled,
            &[bit_write("D100.3", true)],
            "write_mode disabled",
        );
        let words_only = rmw_conn(
            &[("d_u16_write_allowlist", "D100-D101,W1A-W1A")],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        assert_rejected(
            &words_only,
            &[bit_write("D100.3", true)],
            "word allowlist only",
        );
        assert_rejected(
            &words_only,
            &[bit_write("W1A.3", true)],
            "word allowlist only",
        );
        let undiscovered = rmw_conn(
            &[],
            ModelFamily::StandardR,
            PointCount::Exact(200),
            PointCount::Exact(0x41),
        );
        *undiscovered.phase.borrow_mut() = Phase::Start;
        assert_rejected(&undiscovered, &[bit_write("D100.3", true)], "no discovery");
        // Existing bytes for numeric word writes are unchanged.
        let r = ready(
            ModelFamily::StandardR,
            PointCount::Exact(300),
            PointCount::Exact(0x41),
        );
        assert_eq!(
            sent(write_next_on(&r, vec![word_write("D200", 0x1234)], None), 1)[15..],
            [1, 0x14, 2, 0, 200, 0, 0, 0, 0xa8, 0, 1, 0, 0x34, 0x12]
        );
    }

    #[test]
    fn word_bit_allowlist_settings_and_help_are_declared() {
        for ranges in ["D100.3-D100.5", "W1A.0-W1B.F", "D100-D105,D100.0-D100.F"] {
            let settings = route_settings(&[
                ("d_u16_write_mode", "enabled"),
                ("d_u16_write_allowlist", ranges),
            ]);
            assert!(validate_settings(&settings).is_empty(), "{ranges}");
        }
        for ranges in [
            "D100.5-D100.3",
            "D3000.0-D3000.1",
            "M1.0-M1.1",
            "w1a.0-w1a.1",
        ] {
            let settings = route_settings(&[
                ("d_u16_write_mode", "enabled"),
                ("d_u16_write_allowlist", ranges),
            ]);
            assert!(
                validate_settings(&settings)
                    .iter()
                    .any(|error| error.key == ALLOWLIST_KEY),
                "{ranges}"
            );
        }
        let schema = <Component as config::Guest>::schema();
        let help = schema
            .iter()
            .find(|field| field.key == ALLOWLIST_KEY)
            .and_then(|field| field.help.clone())
            .unwrap();
        for needle in ["D100.3-D100.5", "W1A.0-W1B.F", "not atomic", "overwritten"] {
            assert!(help.contains(needle), "{needle}");
        }
        assert!(<Component as address::Guest>::help().contains("read-modify-write"));
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);
