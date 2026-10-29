//! Modbus TCP component with optional write-v1 support.
//! The host owns sockets, timeouts, retries, and connection policy.
//!
//! String tags (stage C1): `data-type "string"` with `string-words = n`
//! (1..=123, one request) on a holding `4xxxx` or input `3xxxx` register
//! span. A read is one FC3/FC4 of the n words and returns `value::bytes` of
//! exactly `2 n` bytes in character order; Modbus packs each register high
//! byte first (author decision), so these are the register data bytes as on
//! the wire, never decoded, NUL-cut or encoding-checked. A write is one
//! string item alone whose `value::bytes` is exactly `2 n` bytes (encoded and
//! 0-padded by the Hub) and is sent as one FC16 over the whole span inside one
//! holding allowlist interval; the guest neither truncates nor pads. Under
//! `register_write=fc6` a one-word string is one FC6 and a wider one is
//! refused before send, as for numerics. Like every write, only the
//! acknowledgement echo is checked; there is no read-back.

use std::cell::{Cell, RefCell};

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_modbus_read_core::{
    decode_write_ack, mask_write_register_request, register_bit_masks, write_coil_request,
    write_coils_request, write_registers_request, write_single_register_request, BitFunction,
    NumericKind, NumericValue, ReadBit, ReadError, ReadHoldingNumeric, ReadRegisterBit,
    ReadRegisterSpan, RegisterFunction, WordOrder, MAX_RESPONSE_LEN,
};
use exports::banto::driver::{address, config, read, session, write_v1};

wit_bindgen::generate!({ path: "../wit", world: "driver-with-write-v1" });

struct Component;

struct ModbusConnection {
    unit_id: u8,
    word_order: WordOrder,
    next_transaction_id: Cell<u16>,
    probe_address: String,
    handshake_pending: Cell<bool>,
    handshake_done: Cell<bool>,
    probe_poisoned: Cell<bool>,
    write_enabled: bool,
    /// False when the device is declared without FC22: register-bit writes
    /// are refused before send instead of timing out the batch.
    register_bit_write: bool,
    /// True when the device is declared without FC16: a holding numeric
    /// write is one single-word FC6 request, and anything wider is refused.
    register_write_fc6: bool,
    write_allowlist: Vec<WriteRange>,
    pending_write: RefCell<Option<PendingWrite>>,
    write_poisoned: Cell<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Area {
    Coil,
    Discrete,
    Input,
    Holding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WriteRange {
    area: Area,
    first: u16,
    last: u16,
}

struct PendingWrite {
    signature: Vec<u8>,
    count: usize,
    transaction_id: u16,
    function: u8,
    echo: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagKind {
    Numeric(Kind, RegisterFunction),
    Bit(BitFunction),
    RegisterBit(RegisterFunction, u8),
    /// A string of `n` contiguous registers (1..=123).
    String(RegisterFunction, u16),
}

/// One FC3/FC4/FC16 request carries a string tag (CONTRACT C1).
const MAX_STRING_WORDS: u32 = 123;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Kind {
    const fn word_count(self) -> u32 {
        match self {
            Self::I16 | Self::U16 => 1,
            Self::I32 | Self::U32 | Self::F32 => 2,
            Self::I64 | Self::U64 | Self::F64 => 4,
        }
    }

    const fn core_kind(self) -> NumericKind {
        match self {
            Self::I16 => NumericKind::I16,
            Self::U16 => NumericKind::U16,
            Self::I32 => NumericKind::I32,
            Self::U32 => NumericKind::U32,
            Self::I64 => NumericKind::I64,
            Self::U64 => NumericKind::U64,
            Self::F32 => NumericKind::F32,
            Self::F64 => NumericKind::F64,
        }
    }
}

fn setting<'a>(settings: &'a [banto::driver::types::Setting], key: &str) -> Option<&'a str> {
    settings
        .iter()
        .find(|item| item.key == key)
        .map(|item| item.value.as_str())
}

fn check_settings(settings: &[banto::driver::types::Setting]) -> Vec<config::FieldError> {
    let mut errors = Vec::new();
    for (index, item) in settings.iter().enumerate() {
        if settings[..index]
            .iter()
            .any(|earlier| earlier.key == item.key)
        {
            errors.push(config::FieldError {
                key: item.key.clone(),
                message: "設定キーが重複しています".into(),
            });
        }
    }
    if setting(settings, "host").is_none_or(|host| host.trim().is_empty()) {
        errors.push(config::FieldError {
            key: "host".into(),
            message: "host を指定してください".into(),
        });
    }
    check_number(settings, "port", 1, 65_535, true, &mut errors);
    check_number(settings, "unit_id", 0, 255, false, &mut errors);
    check_number(
        settings,
        "connect_timeout_ms",
        1,
        600_000,
        false,
        &mut errors,
    );
    check_number(
        settings,
        "response_timeout_ms",
        1,
        600_000,
        false,
        &mut errors,
    );
    if let Some(transport) = setting(settings, "transport") {
        if transport != "tcp" {
            errors.push(config::FieldError {
                key: "transport".into(),
                message: "初版は tcp のみ対応します".into(),
            });
        }
    }
    if let Some(order) = setting(settings, "word_order") {
        if order != "high_low" && order != "low_high" {
            errors.push(config::FieldError {
                key: "word_order".into(),
                message: "word_order は high_low または low_high を指定してください".into(),
            });
        }
    }
    if let Some(probe) = setting(settings, "probe_address") {
        if let Err(message) = parse_probe(probe) {
            errors.push(config::FieldError {
                key: "probe_address".into(),
                message,
            });
        }
    }
    if let Some(mode) = setting(settings, "write_mode") {
        if !matches!(mode, "disabled" | "enabled") {
            errors.push(config::FieldError {
                key: "write_mode".into(),
                message: "write_mode must be disabled or enabled".into(),
            });
        }
    }
    if let Some(mode) = setting(settings, "register_bit_write") {
        if !matches!(mode, "fc22" | "disabled") {
            errors.push(config::FieldError {
                key: "register_bit_write".into(),
                message: "register_bit_write must be fc22 or disabled".into(),
            });
        }
    }
    if let Some(mode) = setting(settings, "register_write") {
        if !matches!(mode, "fc16" | "fc6") {
            errors.push(config::FieldError {
                key: "register_write".into(),
                message: "register_write must be fc16 or fc6".into(),
            });
        }
    }
    if let Some(list) = setting(settings, "write_allowlist") {
        if let Err(message) = parse_write_allowlist(list) {
            errors.push(config::FieldError {
                key: "write_allowlist".into(),
                message,
            });
        }
    }
    if setting(settings, "write_mode") == Some("enabled")
        && setting(settings, "write_allowlist").is_none_or(str::is_empty)
    {
        errors.push(config::FieldError {
            key: "write_allowlist".into(),
            message: "enabled writes require an explicit allowlist".into(),
        });
    }
    errors
}

fn parse_write_allowlist(text: &str) -> Result<Vec<WriteRange>, String> {
    if text.len() > 512 {
        return Err("write_allowlist is too long".into());
    }
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut ranges = Vec::new();
    for part in text.split(',') {
        if ranges.len() == 16 {
            return Err("write_allowlist supports at most 16 ranges".into());
        }
        let (first, last) = part
            .split_once('-')
            .ok_or("each write range requires start-end")?;
        let (start, start_kind) = parse_tag(
            first,
            if first.starts_with('0') {
                "bool"
            } else {
                "u16"
            },
            None,
        )?;
        let (end, end_kind) = parse_tag(
            last,
            if last.starts_with('0') { "bool" } else { "u16" },
            None,
        )?;
        let range_area = area(start_kind);
        if !matches!(range_area, Area::Coil | Area::Holding)
            || range_area != area(end_kind)
            || start > end
        {
            return Err("write range must be ascending in one writable area".into());
        }
        if ranges
            .iter()
            .any(|r: &WriteRange| r.area == range_area && start <= r.last && end >= r.first)
        {
            return Err("write ranges overlap".into());
        }
        ranges.push(WriteRange {
            area: range_area,
            first: start,
            last: end,
        });
    }
    Ok(ranges)
}

fn check_number(
    settings: &[banto::driver::types::Setting],
    key: &str,
    min: u32,
    max: u32,
    required: bool,
    errors: &mut Vec<config::FieldError>,
) {
    match setting(settings, key) {
        None if !required => {}
        Some(value) if value.parse::<u32>().is_ok_and(|n| (min..=max).contains(&n)) => {}
        _ => errors.push(config::FieldError {
            key: key.into(),
            message: format!("{key} は {min}〜{max} の整数を指定してください"),
        }),
    }
}

/// Preserve the native parser's 5/6 digit reference and canonical form.
/// Numeric tags occupy one, two, or four complete holding registers.
fn parse_holding(text: &str, data_type: &str, words: Option<u32>) -> Result<(u16, Kind), String> {
    let kind = match (data_type, words) {
        ("i16", None) => Kind::I16,
        ("u16", None) => Kind::U16,
        ("i32", None) => Kind::I32,
        ("u32", None) => Kind::U32,
        ("i64", None) => Kind::I64,
        ("u64", None) => Kind::U64,
        ("f32", None) => Kind::F32,
        ("f64", None) => Kind::F64,
        _ => return Err("FC3/FC4 support i16/u16/i32/u32/i64/u64/f32/f64 registers".into()),
    };
    let text = text.trim();
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 5 | 6)
        || !matches!(bytes[0], b'3' | b'4')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return Err(format!("{text:?} は保持レジスタの参照番号ではありません"));
    }
    let number = text[1..]
        .parse::<u32>()
        .map_err(|_| format!("{text:?} は保持レジスタの参照番号ではありません"))?;
    let offset = u16::try_from(number.checked_sub(1).ok_or("レジスタ番号は1からです")?)
        .map_err(|_| "保持レジスタの範囲を超えています".to_string())?;
    if u32::from(offset) + kind.word_count() > u32::from(u16::MAX) + 1 {
        return Err("numeric tag exceeds holding-register address space".into());
    }
    Ok((offset, kind))
}

/// A string spans `n` whole registers of one holding or input table, with
/// the same reference and canonical form as a numeric tag at its start.
fn parse_string(text: &str, words: Option<u32>) -> Result<(u16, TagKind), String> {
    let words = words
        .filter(|words| (1..=MAX_STRING_WORDS).contains(words))
        .ok_or("string tags require string_words 1..=123 (one request)")?;
    let (offset, _) = parse_holding(text, "u16", None)?;
    if u32::from(offset) + words > u32::from(u16::MAX) + 1 {
        return Err("string tag exceeds register address space".into());
    }
    Ok((
        offset,
        TagKind::String(register_function(text), words as u16),
    ))
}

fn parse_bit(
    text: &str,
    data_type: &str,
    words: Option<u32>,
) -> Result<(u16, BitFunction), String> {
    if data_type != "bool" || words.is_some() {
        return Err("bit tables require bool without string_words".into());
    }
    let text = text.trim();
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 5 | 6)
        || !matches!(bytes[0], b'0' | b'1')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return Err("invalid bit reference".into());
    }
    let number = text[1..]
        .parse::<u32>()
        .map_err(|_| "invalid bit reference")?;
    let offset = u16::try_from(number.checked_sub(1).ok_or("bit reference starts at 1")?)
        .map_err(|_| "bit reference exceeds 65536 points")?;
    Ok((
        offset,
        if bytes[0] == b'0' {
            BitFunction::Coil
        } else {
            BitFunction::DiscreteInput
        },
    ))
}

fn parse_register_bit(
    text: &str,
    data_type: &str,
    words: Option<u32>,
) -> Result<(u16, TagKind), String> {
    if data_type != "bool" || words.is_some() {
        return Err("register-bit requires bool without string_words".into());
    }
    let (base, suffix) = text
        .trim()
        .split_once('.')
        .ok_or("register-bit suffix required")?;
    if !matches!(base.len(), 5 | 6) || !base.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid register-bit reference".into());
    }
    if !(1..=2).contains(&suffix.len()) || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("register-bit suffix must be decimal 0..15".into());
    }
    let bit = suffix.parse::<u8>().map_err(|_| "invalid register bit")?;
    if bit > 15 {
        return Err("register-bit suffix must be decimal 0..15".into());
    }
    let (offset, _) = parse_holding(base, "u16", None)?;
    Ok((offset, TagKind::RegisterBit(register_function(base), bit)))
}

fn parse_tag(text: &str, data_type: &str, words: Option<u32>) -> Result<(u16, TagKind), String> {
    match text.trim().as_bytes().first() {
        Some(b'0' | b'1') => parse_bit(text, data_type, words)
            .map(|(offset, function)| (offset, TagKind::Bit(function))),
        Some(b'3' | b'4') if text.trim().contains('.') => {
            parse_register_bit(text, data_type, words)
        }
        _ if data_type == "string" => parse_string(text, words),
        _ => parse_holding(text, data_type, words)
            .map(|(offset, kind)| (offset, TagKind::Numeric(kind, register_function(text)))),
    }
}

/// The handshake probe is one coil (FC1), discrete input (FC2), input
/// register (FC4) or holding register (FC3). Register bits are not probes.
fn parse_probe(text: &str) -> Result<(u16, TagKind), String> {
    let data_type = if matches!(text.trim().as_bytes().first(), Some(b'0' | b'1')) {
        "bool"
    } else {
        "u16"
    };
    parse_tag(text, data_type, None).map_err(|_| {
        "probe_address requires one 0xxxx coil, 1xxxx discrete input, 3xxxx input or 4xxxx holding register".into()
    })
}

fn area(kind: TagKind) -> Area {
    match kind {
        TagKind::Bit(BitFunction::Coil) => Area::Coil,
        TagKind::Bit(BitFunction::DiscreteInput) => Area::Discrete,
        TagKind::RegisterBit(RegisterFunction::Input, _) => Area::Input,
        TagKind::RegisterBit(RegisterFunction::Holding, _) => Area::Holding,
        TagKind::Numeric(_, RegisterFunction::Input) => Area::Input,
        TagKind::Numeric(_, RegisterFunction::Holding) => Area::Holding,
        TagKind::String(RegisterFunction::Input, _) => Area::Input,
        TagKind::String(RegisterFunction::Holding, _) => Area::Holding,
    }
}

fn tag_width(kind: TagKind) -> u32 {
    match kind {
        TagKind::Numeric(kind, _) => kind.word_count(),
        TagKind::Bit(_) => 1,
        TagKind::RegisterBit(_, _) => 1,
        TagKind::String(_, words) => u32::from(words),
    }
}

fn canonical_tag(offset: u16, kind: TagKind) -> String {
    match kind {
        TagKind::Bit(BitFunction::Coil) => format!("0{:05}", u32::from(offset) + 1),
        TagKind::Bit(BitFunction::DiscreteInput) => format!("1{:05}", u32::from(offset) + 1),
        TagKind::RegisterBit(function, bit) => {
            format!("{}.{bit}", canonical_register(offset, function))
        }
        TagKind::Numeric(_, function) | TagKind::String(function, _) => {
            canonical_register(offset, function)
        }
    }
}

#[cfg(test)]
fn canonical(offset: u16) -> String {
    canonical_register(offset, RegisterFunction::Holding)
}

fn canonical_register(offset: u16, function: RegisterFunction) -> String {
    let prefix = match function {
        RegisterFunction::Holding => 4,
        RegisterFunction::Input => 3,
    };
    format!("{prefix}{:05}", u32::from(offset) + 1)
}

fn register_function(text: &str) -> RegisterFunction {
    if text.trim().starts_with('3') {
        RegisterFunction::Input
    } else {
        RegisterFunction::Holding
    }
}

fn request_bytes(
    conn: &ModbusConnection,
    transaction_id: u16,
    offset: u16,
    kind: Kind,
    function: RegisterFunction,
) -> [u8; 12] {
    ReadHoldingNumeric {
        transaction_id,
        unit_id: conn.unit_id,
        offset,
        kind: kind.core_kind(),
        word_order: conn.word_order,
    }
    .request_with_function(function)
    .expect("validated numeric start offset")
}

/// The one-tag read request for any tag kind.
fn read_request(
    conn: &ModbusConnection,
    transaction_id: u16,
    offset: u16,
    kind: TagKind,
) -> [u8; 12] {
    match kind {
        TagKind::Numeric(kind, function) => {
            request_bytes(conn, transaction_id, offset, kind, function)
        }
        TagKind::Bit(function) => ReadBit {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            function,
        }
        .request(),
        TagKind::RegisterBit(function, bit) => ReadRegisterBit {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            bit,
            function,
        }
        .request(),
        TagKind::String(function, words) => ReadRegisterSpan {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            count: words,
            function,
        }
        .request()
        .expect("validated string span"),
    }
}

fn decode_read(
    conn: &ModbusConnection,
    transaction_id: u16,
    offset: u16,
    kind: TagKind,
    response: &[u8],
) -> Result<Value, ReadError> {
    match kind {
        TagKind::Numeric(kind, function) => ReadHoldingNumeric {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            kind: kind.core_kind(),
            word_order: conn.word_order,
        }
        .decode_with_function(response, function)
        .map(|value| match value {
            NumericValue::Int(value) => Value::Int(value),
            NumericValue::Uint(value) => Value::Uint(value),
            NumericValue::Float(value) => Value::Float(value),
        }),
        TagKind::Bit(function) => ReadBit {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            function,
        }
        .decode(response)
        .map(Value::Bool),
        TagKind::RegisterBit(function, bit) => ReadRegisterBit {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            bit,
            function,
        }
        .decode(response)
        .map(Value::Bool),
        // Character order is the wire order: each register high byte first.
        TagKind::String(function, words) => ReadRegisterSpan {
            transaction_id,
            unit_id: conn.unit_id,
            offset,
            count: words,
            function,
        }
        .decode(response)
        .map(|data| Value::Bytes(data.to_vec())),
    }
}

fn step(send: [u8; 12]) -> session::Step {
    frame_step(send.to_vec())
}

fn frame_step(send: Vec<u8>) -> session::Step {
    session::Step {
        send,
        expect: session::FrameRule::LengthPrefixed(session::PrefixSpec {
            offset: 4,
            width: 2,
            big_endian: true,
            adjust: 6,
            encoding: session::PrefixEncoding::Binary,
        }),
        max_len: MAX_RESPONSE_LEN as u32,
    }
}

fn numeric_words(kind: Kind, value: &Value, order: WordOrder) -> Result<Vec<u16>, String> {
    let bytes: Vec<u8> = match (kind, value) {
        (Kind::I16, Value::Int(n)) => i16::try_from(*n)
            .map_err(|_| "i16 value out of range")?
            .to_be_bytes()
            .to_vec(),
        (Kind::U16, Value::Uint(n)) => u16::try_from(*n)
            .map_err(|_| "u16 value out of range")?
            .to_be_bytes()
            .to_vec(),
        (Kind::I32, Value::Int(n)) => i32::try_from(*n)
            .map_err(|_| "i32 value out of range")?
            .to_be_bytes()
            .to_vec(),
        (Kind::U32, Value::Uint(n)) => u32::try_from(*n)
            .map_err(|_| "u32 value out of range")?
            .to_be_bytes()
            .to_vec(),
        (Kind::I64, Value::Int(n)) => n.to_be_bytes().to_vec(),
        (Kind::U64, Value::Uint(n)) => n.to_be_bytes().to_vec(),
        (Kind::F32, Value::Float(n)) if n.is_finite() && n.abs() <= f32::MAX as f64 => {
            (*n as f32).to_bits().to_be_bytes().to_vec()
        }
        (Kind::F64, Value::Float(n)) if n.is_finite() => n.to_bits().to_be_bytes().to_vec(),
        _ => return Err("write value has the wrong type or exceeds its width".into()),
    };
    let mut words: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes(*pair))
        .collect();
    if order == WordOrder::LowHigh && words.len() > 1 {
        words.reverse();
    }
    Ok(words)
}

/// Function code, request frame and the PDU bytes the ACK must echo.
type WriteRequest = (u8, Vec<u8>, Vec<u8>);

fn preflight_write(
    conn: &ModbusConnection,
    items: &[write_v1::WriteItem],
    transaction_id: u16,
) -> Result<WriteRequest, String> {
    if items.is_empty() || items.len() > 123 {
        return Err("write requires 1..123 items".into());
    }
    if let Some(request) = preflight_string_write(conn, items, transaction_id)? {
        return Ok(request);
    }
    if let Some(request) = preflight_register_bit_write(conn, items, transaction_id)? {
        return Ok(request);
    }
    let mut first = None;
    let mut area_kind = None;
    let mut words = Vec::new();
    let mut end = 0u32;
    let mut coils = Vec::new();
    for item in items {
        let (offset, kind) = parse_tag(
            &item.spec.address,
            &item.spec.data_type,
            item.spec.string_words,
        )?;
        let current_area = area(kind);
        if !matches!(current_area, Area::Coil | Area::Holding) {
            return Err("input-only area is not writable".into());
        }
        if area_kind.is_some_and(|area| area != current_area) {
            return Err("write items must share one area".into());
        }
        if first.is_some() && u32::from(offset) != end {
            return Err("write items must form a contiguous span".into());
        }
        area_kind = Some(current_area);
        first.get_or_insert(offset);
        end = u32::from(offset) + tag_width(kind);
        if end > 65_536 {
            return Err("write span exceeds Modbus address space".into());
        }
        match kind {
            TagKind::Bit(BitFunction::Coil) => match item.value {
                Value::Bool(value) => coils.push(value),
                _ => return Err("coil write requires a Bool value".into()),
            },
            TagKind::Numeric(kind, RegisterFunction::Holding) => {
                words.extend(numeric_words(kind, &item.value, conn.word_order)?)
            }
            TagKind::RegisterBit(_, _) => {
                return Err("register-bit writes require one point per request".into())
            }
            TagKind::String(_, _) => return Err("a string tag must be written alone".into()),
            _ => return Err("input-only area is not writable".into()),
        }
    }
    let start = first.expect("checked nonempty");
    let last = u16::try_from(end - 1).map_err(|_| "write span exceeds Modbus address space")?;
    let area = area_kind.expect("checked nonempty");
    if !conn
        .write_allowlist
        .iter()
        .any(|range| range.area == area && range.first <= start && last <= range.last)
    {
        return Err("write span is outside the enabled allowlist".into());
    }
    if let [value] = coils[..] {
        let frame = write_coil_request(transaction_id, conn.unit_id, start, value);
        Ok((5, frame.to_vec(), frame[8..12].to_vec()))
    } else if area == Area::Coil {
        // A contiguous coil run inside one allowlist interval is one FC15.
        let frame = write_coils_request(transaction_id, conn.unit_id, start, &coils)
            .map_err(|_| "coil write exceeds the FC15 count or address limit")?;
        let mut echo = start.to_be_bytes().to_vec();
        echo.extend_from_slice(&(coils.len() as u16).to_be_bytes());
        Ok((15, frame, echo))
    } else if conn.register_write_fc6 {
        let [word] = words[..] else {
            return Err(
                "register_write=fc6 allows only one i16/u16 holding register per request".into(),
            );
        };
        let frame = write_single_register_request(transaction_id, conn.unit_id, start, word);
        Ok((6, frame.to_vec(), frame[8..12].to_vec()))
    } else {
        let frame = write_registers_request(transaction_id, conn.unit_id, start, &words)
            .map_err(|_| "register write exceeds the FC16 count or address limit")?;
        let mut echo = start.to_be_bytes().to_vec();
        echo.extend_from_slice(&(words.len() as u16).to_be_bytes());
        Ok((16, frame, echo))
    }
}

/// A string tag is written alone as its whole span in one request: the
/// Hub's `2 n` bytes in character order become n registers, high byte first,
/// inside one holding allowlist interval. Nothing is truncated or padded.
fn preflight_string_write(
    conn: &ModbusConnection,
    items: &[write_v1::WriteItem],
    transaction_id: u16,
) -> Result<Option<WriteRequest>, String> {
    if !items.iter().any(|item| item.spec.data_type == "string") {
        return Ok(None);
    }
    let [item] = items else {
        return Err("a string tag must be written alone".into());
    };
    let (offset, kind) = parse_tag(
        &item.spec.address,
        &item.spec.data_type,
        item.spec.string_words,
    )?;
    let TagKind::String(function, words) = kind else {
        return Err("string tags require a holding register span".into());
    };
    if function != RegisterFunction::Holding {
        return Err("input-only area is not writable".into());
    }
    let Value::Bytes(bytes) = &item.value else {
        return Err("string write requires a bytes value".into());
    };
    if bytes.len() != usize::from(words) * 2 {
        return Err("string write requires exactly 2 * string_words bytes".into());
    }
    let last = offset + (words - 1);
    if !conn
        .write_allowlist
        .iter()
        .any(|range| range.area == Area::Holding && range.first <= offset && last <= range.last)
    {
        return Err("write span is outside the enabled allowlist".into());
    }
    let registers: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes(*pair))
        .collect();
    if conn.register_write_fc6 {
        // FC6 writes one register: like numerics, only a one-word span fits.
        let [word] = registers[..] else {
            return Err(
                "register_write=fc6 allows only one holding register per request; this string needs FC16".into(),
            );
        };
        let frame = write_single_register_request(transaction_id, conn.unit_id, offset, word);
        return Ok(Some((6, frame.to_vec(), frame[8..12].to_vec())));
    }
    let frame = write_registers_request(transaction_id, conn.unit_id, offset, &registers)
        .map_err(|_| "register write exceeds the FC16 count or address limit")?;
    let mut echo = offset.to_be_bytes().to_vec();
    echo.extend_from_slice(&words.to_be_bytes());
    Ok(Some((16, frame, echo)))
}

/// A holding register bit is written alone with FC22, so the device changes
/// only that bit. The word must lie in a holding allowlist interval.
fn preflight_register_bit_write(
    conn: &ModbusConnection,
    items: &[write_v1::WriteItem],
    transaction_id: u16,
) -> Result<Option<WriteRequest>, String> {
    let [item] = items else {
        return Ok(None);
    };
    let (offset, kind) = parse_tag(
        &item.spec.address,
        &item.spec.data_type,
        item.spec.string_words,
    )?;
    let TagKind::RegisterBit(function, bit) = kind else {
        return Ok(None);
    };
    if function != RegisterFunction::Holding {
        return Err("input-only area is not writable".into());
    }
    if !conn.register_bit_write {
        return Err(
            "register-bit writes are disabled for this device (register_bit_write=disabled)".into(),
        );
    }
    let Value::Bool(value) = item.value else {
        return Err("register-bit write requires a Bool value".into());
    };
    if !conn
        .write_allowlist
        .iter()
        .any(|range| range.area == Area::Holding && range.first <= offset && offset <= range.last)
    {
        return Err("write span is outside the enabled allowlist".into());
    }
    let (and_mask, or_mask) = register_bit_masks(bit, value).map_err(|_| "invalid register bit")?;
    let frame =
        mask_write_register_request(transaction_id, conn.unit_id, offset, and_mask, or_mask);
    Ok(Some((22, frame.to_vec(), frame[8..14].to_vec())))
}

fn rejected_write(count: usize, message: &str) -> write_v1::WriteProgress {
    write_v1::WriteProgress::Done(
        (0..count)
            .map(|_| Err(ItemError::Rejected(message.into())))
            .collect(),
    )
}

fn planned_read(conn: &ModbusConnection, specs: &[TagSpec]) -> Vec<read::Planned> {
    specs
        .iter()
        .enumerate()
        .filter_map(|(index, spec)| {
            let (offset, kind) =
                parse_tag(&spec.address, &spec.data_type, spec.string_words).ok()?;
            let transaction_id = conn.next_transaction_id.get();
            conn.next_transaction_id.set(transaction_id.wrapping_add(1));
            Some(read::Planned {
                step: step(read_request(conn, transaction_id, offset, kind)),
                covers: vec![index as u32],
            })
        })
        .collect()
}

fn decode_planned(
    conn: &ModbusConnection,
    p: &read::Planned,
    specs: &[TagSpec],
    response: &[u8],
) -> Result<Vec<Result<Value, ItemError>>, FatalError> {
    let [cover] = p.covers.as_slice() else {
        return Err(FatalError::Protocol("FC3 の covers は1件です".into()));
    };
    let spec = specs
        .get(*cover as usize)
        .ok_or_else(|| FatalError::Protocol("covers が specs の範囲外です".into()))?;
    let (offset, kind) = parse_tag(&spec.address, &spec.data_type, spec.string_words)
        .map_err(FatalError::Protocol)?;
    let send: [u8; 12] = p
        .step
        .send
        .as_slice()
        .try_into()
        .map_err(|_| FatalError::Protocol("FC3 要求長が不正です".into()))?;
    let transaction_id = u16::from_be_bytes([send[0], send[1]]);
    if send != read_request(conn, transaction_id, offset, kind) {
        return Err(FatalError::Protocol(
            "計画と要求バイトが一致しません".into(),
        ));
    }
    let value = match decode_read(conn, transaction_id, offset, kind, response) {
        Ok(value) => Ok(value),
        Err(ReadError::Device(code)) => Err(ItemError::Device(banto::driver::types::DeviceError {
            code: u32::from(code),
            message: format!(
                "FC=0x{:02X} Modbus exception 0x{code:02X}",
                match kind {
                    TagKind::Bit(BitFunction::Coil) => 1,
                    TagKind::Bit(BitFunction::DiscreteInput) => 2,
                    TagKind::Numeric(_, RegisterFunction::Holding) => 3,
                    TagKind::Numeric(_, RegisterFunction::Input) => 4,
                    TagKind::RegisterBit(RegisterFunction::Holding, _) => 3,
                    TagKind::RegisterBit(RegisterFunction::Input, _) => 4,
                    TagKind::String(RegisterFunction::Holding, _) => 3,
                    TagKind::String(RegisterFunction::Input, _) => 4,
                }
            ),
        })),
        Err(ReadError::Framing) => {
            return Err(FatalError::Framing("FC3 応答フレームが不正です".into()))
        }
        Err(ReadError::Protocol) => {
            return Err(FatalError::Protocol(
                "FC3 応答 ID が要求と異なります".into(),
            ))
        }
    };
    Ok(vec![value])
}

impl session::GuestConnection for ModbusConnection {
    fn create(settings: Vec<session::Setting>) -> Result<session::Connection, FatalError> {
        let errors = check_settings(&settings);
        if let Some(first) = errors.first() {
            return Err(FatalError::InvalidConfig(first.message.clone()));
        }
        let unit_id = setting(&settings, "unit_id")
            .map(str::parse::<u8>)
            .transpose()
            .map_err(|_| FatalError::InvalidConfig("unit_id が不正です".into()))?
            .unwrap_or(1);
        Ok(session::Connection::new(ModbusConnection {
            unit_id,
            word_order: setting(&settings, "word_order")
                .and_then(WordOrder::parse)
                .unwrap_or_default(),
            next_transaction_id: Cell::new(0),
            probe_address: setting(&settings, "probe_address")
                .unwrap_or("40001")
                .to_owned(),
            handshake_pending: Cell::new(false),
            handshake_done: Cell::new(false),
            probe_poisoned: Cell::new(false),
            write_enabled: setting(&settings, "write_mode") == Some("enabled"),
            register_bit_write: setting(&settings, "register_bit_write") != Some("disabled"),
            register_write_fc6: setting(&settings, "register_write") == Some("fc6"),
            write_allowlist: parse_write_allowlist(
                setting(&settings, "write_allowlist").unwrap_or(""),
            )
            .map_err(FatalError::InvalidConfig)?,
            pending_write: RefCell::new(None),
            write_poisoned: Cell::new(false),
        }))
    }

    fn handshake_next(
        &self,
        prev_response: Option<Vec<u8>>,
    ) -> Result<Option<session::Step>, FatalError> {
        if self.probe_poisoned.get() {
            return Err(FatalError::Protocol(
                "probe outcome is uncertain; reconnect".into(),
            ));
        }
        if let Some(response) = prev_response {
            if !self.handshake_pending.replace(false) {
                return Err(FatalError::Protocol("unsolicited probe response".into()));
            }
            self.probe_poisoned.set(true);
            let (offset, kind) =
                parse_probe(&self.probe_address).map_err(FatalError::InvalidConfig)?;
            let transaction_id = self.next_transaction_id.get().wrapping_sub(1);
            match decode_read(self, transaction_id, offset, kind, &response) {
                Ok(_) => {
                    self.probe_poisoned.set(false);
                    self.handshake_done.set(true);
                    Ok(None)
                }
                Err(ReadError::Device(code)) => Err(FatalError::Protocol(format!(
                    "probe returned Modbus exception 0x{code:02X}"
                ))),
                Err(ReadError::Framing) => {
                    Err(FatalError::Framing("invalid probe response".into()))
                }
                Err(ReadError::Protocol) => Err(FatalError::Protocol(
                    "probe transaction or unit mismatch".into(),
                )),
            }
        } else if self.handshake_done.get() {
            Ok(None)
        } else if self.handshake_pending.get() {
            Err(FatalError::Protocol("probe response is pending".into()))
        } else {
            let (offset, kind) =
                parse_probe(&self.probe_address).map_err(FatalError::InvalidConfig)?;
            let transaction_id = self.next_transaction_id.get();
            self.next_transaction_id.set(transaction_id.wrapping_add(1));
            self.handshake_pending.set(true);
            Ok(Some(step(read_request(self, transaction_id, offset, kind))))
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
        Err(FatalError::Protocol("plugin-decides は未使用です".into()))
    }
}

impl session::Guest for Component {
    type Connection = ModbusConnection;
}

impl read::Guest for Component {
    fn plan(
        conn: read::ConnectionBorrow<'_>,
        specs: Vec<TagSpec>,
    ) -> Result<Vec<read::Planned>, FatalError> {
        let conn = conn.get::<ModbusConnection>();
        if conn.probe_poisoned.get()
            || conn.pending_write.borrow().is_some()
            || conn.write_poisoned.get()
            || conn.handshake_pending.get()
        {
            return Err(FatalError::Protocol(
                "a response is pending or the connection is uncertain".into(),
            ));
        }
        Ok(planned_read(conn, &specs))
    }

    fn decode(
        conn: read::ConnectionBorrow<'_>,
        p: read::Planned,
        specs: Vec<TagSpec>,
        response: Vec<u8>,
    ) -> Result<Vec<Result<Value, ItemError>>, FatalError> {
        decode_planned(conn.get::<ModbusConnection>(), &p, &specs, &response)
    }
}

impl write_v1::Guest for Component {
    fn write_next(
        conn: write_v1::ConnectionBorrow<'_>,
        items: Vec<write_v1::WriteItem>,
        prev_response: Option<Vec<u8>>,
    ) -> Result<write_v1::WriteProgress, FatalError> {
        write_next_impl(conn.get::<ModbusConnection>(), items, prev_response)
    }
}

fn write_next_impl(
    conn: &ModbusConnection,
    items: Vec<write_v1::WriteItem>,
    prev_response: Option<Vec<u8>>,
) -> Result<write_v1::WriteProgress, FatalError> {
    if let Some(response) = prev_response {
        // Consume the pending operation before decoding. An invalid ACK
        // leaves the connection poisoned; no automatic replay is safe.
        conn.write_poisoned.set(true);
        let pending = conn
            .pending_write
            .borrow_mut()
            .take()
            .ok_or_else(|| FatalError::Protocol("unsolicited write response".into()))?;
        let (_, signature, _) = preflight_write(conn, &items, pending.transaction_id)
            .map_err(|_| FatalError::Protocol("write items changed after send".into()))?;
        if signature != pending.signature || items.len() != pending.count {
            return Err(FatalError::Protocol(
                "write items changed after send".into(),
            ));
        }
        return match decode_write_ack(
            &response,
            pending.transaction_id,
            conn.unit_id,
            pending.function,
            &pending.echo,
        ) {
            Ok(()) => {
                conn.write_poisoned.set(false);
                Ok(write_v1::WriteProgress::Done(
                    (0..items.len()).map(|_| Ok(())).collect(),
                ))
            }
            Err(ReadError::Device(code)) => {
                conn.write_poisoned.set(false);
                Ok(write_v1::WriteProgress::Done(
                    (0..items.len())
                        .map(|_| {
                            Err(ItemError::Device(banto::driver::types::DeviceError {
                                code: u32::from(code),
                                message: format!("Modbus exception 0x{code:02X}"),
                            }))
                        })
                        .collect(),
                ))
            }
            Err(ReadError::Framing) => {
                Err(FatalError::Framing("invalid write acknowledgement".into()))
            }
            Err(ReadError::Protocol) => Err(FatalError::Protocol(
                "write acknowledgement mismatch".into(),
            )),
        };
    }
    if conn.write_poisoned.get() || conn.probe_poisoned.get() {
        return Err(FatalError::Protocol(
            "write outcome is uncertain; reconnect".into(),
        ));
    }
    if conn.pending_write.borrow().is_some() || conn.handshake_pending.get() {
        return Err(FatalError::Protocol("a response is pending".into()));
    }
    if items.is_empty() {
        return Err(FatalError::InvalidConfig(
            "write requires at least one item".into(),
        ));
    }
    if !conn.write_enabled {
        return Ok(rejected_write(items.len(), "writes are disabled"));
    }
    let transaction_id = conn.next_transaction_id.get();
    let (function, frame, echo) = match preflight_write(conn, &items, transaction_id) {
        Ok(request) => request,
        Err(message) => return Ok(rejected_write(items.len(), &message)),
    };
    conn.next_transaction_id.set(transaction_id.wrapping_add(1));
    *conn.pending_write.borrow_mut() = Some(PendingWrite {
        signature: frame.clone(),
        count: items.len(),
        transaction_id,
        function,
        echo,
    });
    Ok(write_v1::WriteProgress::Send(read::Planned {
        step: frame_step(frame),
        covers: (0..items.len() as u32).collect(),
    }))
}

impl address::Guest for Component {
    fn parse(
        text: String,
        data_type: String,
        string_words: Option<u32>,
    ) -> Result<address::Parsed, String> {
        let (offset, kind) = parse_tag(&text, &data_type, string_words)?;
        Ok(address::Parsed {
            canonical: canonical_tag(offset, kind),
            area: match area(kind) {
                Area::Coil => "coil",
                Area::Discrete => "discrete_input",
                Area::Input => "input_register",
                Area::Holding => "holding_register",
            }
            .into(),
            writable_area: matches!(area(kind), Area::Coil | Area::Holding),
            bit: match kind {
                TagKind::RegisterBit(_, bit) => Some(bit),
                _ => None,
            },
            word_count: tag_width(kind),
            index: Some(u64::from(offset)),
        })
    }

    fn offset(
        canonical: String,
        data_type: String,
        string_words: Option<u32>,
        n: i64,
    ) -> Result<String, String> {
        let (offset, kind) = parse_tag(&canonical, &data_type, string_words)?;
        if matches!(kind, TagKind::RegisterBit(_, _)) && n != 0 {
            return Err("register-bit addresses cannot be offset by tags".into());
        }
        let words = i64::from(tag_width(kind))
            .checked_mul(n)
            .ok_or("オフセットが範囲外です")?;
        let next = i64::from(offset)
            .checked_add(words)
            .ok_or("オフセットが範囲外です")?;
        canonical_for_tag(next, kind)
    }

    fn supported_kinds() -> Vec<String> {
        [
            "bool", "u16", "u32", "i16", "i32", "i64", "u64", "f32", "f64", "string",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn offset_words(canonical: String, n: i64) -> Result<String, String> {
        let data_type = if matches!(canonical.trim().as_bytes().first(), Some(b'0' | b'1'))
            || canonical.contains('.')
        {
            "bool"
        } else {
            "u16"
        };
        let (offset, kind) = parse_tag(&canonical, data_type, None)?;
        let next = i64::from(offset)
            .checked_add(n)
            .ok_or("オフセットが範囲外です")?;
        canonical_for_tag(next, kind)
    }

    fn help() -> String {
        "FC1 coils (0) and FC2 discrete inputs (1): bool; FC3 holding (4) and FC4 input (3): eight numeric types or register-bit bool (for example 40001.3). A 4xxxx.N bit is written alone with FC22 mask write unless register_bit_write=disabled declares a device without FC22; 3xxxx.N is read-only. Coils are written with FC5 (one point) or FC15 (a contiguous run); holding words with FC16, or one i16/u16 with FC6 when register_write=fc6. Connection probe defaults to 40001; probe_address may select one 0xxxx coil, 1xxxx discrete input or 3xxxx input register instead. Strings: 4xxxx or 3xxxx with string_words 1..=123, read as raw bytes high byte first per register; a 4xxxx string is written alone as one FC16 over the whole span (FC6 only for one word under register_write=fc6). Writes require write_mode=enabled and write_allowlist."
            .into()
    }

    fn probe() -> Option<TagSpec> {
        // A connection-bound probe runs during handshake so FC4-only devices
        // can select an input register without changing this WIT interface.
        None
    }
}

fn canonical_for(offset: i64, kind: Kind, function: RegisterFunction) -> Result<String, String> {
    let offset = u16::try_from(offset).map_err(|_| "保持レジスタの範囲を超えています")?;
    if u32::from(offset) + kind.word_count() > u32::from(u16::MAX) + 1 {
        return Err("numeric tag exceeds holding-register address space".into());
    }
    Ok(canonical_register(offset, function))
}

fn canonical_for_tag(offset: i64, kind: TagKind) -> Result<String, String> {
    if let TagKind::Numeric(numeric, function) = kind {
        return canonical_for(offset, numeric, function);
    }
    let offset = u16::try_from(offset).map_err(|_| "tag exceeds Modbus address space")?;
    if u32::from(offset) + tag_width(kind) > 65_536 {
        return Err("tag exceeds Modbus address space".into());
    }
    Ok(canonical_tag(offset, kind))
}

impl config::Guest for Component {
    fn validate(settings: Vec<config::Setting>) -> Vec<config::FieldError> {
        check_settings(&settings)
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
                label: "ホスト".into(),
                help: None,
                kind: FieldKind::Host,
                required: true,
                default: None,
                group: FieldGroup::Basic,
            },
            SettingField {
                key: "port".into(),
                label: "ポート".into(),
                help: None,
                kind: FieldKind::Integer(IntegerSpec {
                    min: Some(1),
                    max: Some(65_535),
                }),
                required: true,
                default: Some(SettingValue::Integer(502)),
                group: FieldGroup::Basic,
            },
            SettingField {
                key: "unit_id".into(),
                label: "ユニットID".into(),
                help: None,
                kind: FieldKind::Integer(IntegerSpec {
                    min: Some(0),
                    max: Some(255),
                }),
                required: false,
                default: Some(SettingValue::Integer(1)),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: "word_order".into(),
                label: "ワード順".into(),
                help: Some("Word order within each multiword numeric value".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption {
                        value: "low_high".into(),
                        label: "low_high".into(),
                    },
                    ChoiceOption {
                        value: "high_low".into(),
                        label: "high_low".into(),
                    },
                ]),
                required: false,
                default: Some(SettingValue::Text("low_high".into())),
                group: FieldGroup::Advanced,
            },
            SettingField {
                key: "probe_address".into(), label: "Probe address".into(),
                help: Some("One accessible 4xxxx holding, 3xxxx input register, 0xxxx coil or 1xxxx discrete input for connection verification".into()),
                kind: FieldKind::Text(config::TextSpec { max_len: Some(6), placeholder: Some("40001".into()) }),
                required: false, default: Some(SettingValue::Text("40001".into())), group: FieldGroup::Advanced,
            },
            SettingField {
                key: "write_mode".into(), label: "Write mode".into(),
                help: Some("Disabled by default; host policy must also permit writes".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption { value: "disabled".into(), label: "disabled".into() },
                    ChoiceOption { value: "enabled".into(), label: "enabled".into() },
                ]),
                required: false, default: Some(SettingValue::Text("disabled".into())), group: FieldGroup::Advanced,
            },
            SettingField {
                key: "register_bit_write".into(), label: "Register-bit write".into(),
                help: Some("fc22 writes one 4xxxx.N bit with FC22 mask write; disabled refuses register-bit writes before send for devices without FC22".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption { value: "fc22".into(), label: "fc22".into() },
                    ChoiceOption { value: "disabled".into(), label: "disabled".into() },
                ]),
                required: false, default: Some(SettingValue::Text("fc22".into())), group: FieldGroup::Advanced,
            },
            SettingField {
                key: "register_write".into(), label: "Register write".into(),
                help: Some("fc16 writes holding words with FC16; fc6 writes one i16/u16 with FC6 and refuses wider writes before send for devices without FC16".into()),
                kind: FieldKind::Choice(vec![
                    ChoiceOption { value: "fc16".into(), label: "fc16".into() },
                    ChoiceOption { value: "fc6".into(), label: "fc6".into() },
                ]),
                required: false, default: Some(SettingValue::Text("fc16".into())), group: FieldGroup::Advanced,
            },
            SettingField {
                key: "write_allowlist".into(), label: "Write address ranges".into(),
                help: Some("Explicit coil or holding ranges, e.g. 000001-000016,400001-400010".into()),
                kind: FieldKind::Text(config::TextSpec { max_len: Some(512), placeholder: None }),
                required: false, default: Some(SettingValue::Text(String::new())), group: FieldGroup::Advanced,
            },
        ]
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_bit_parse_offsets_and_refusal_match_native() {
        for (raw, canonical, bit, area_name, writable) in [
            ("40001.03", "400001.3", 3, "holding_register", true),
            (" 40001.03 ", "400001.3", 3, "holding_register", true),
            ("30001.15", "300001.15", 15, "input_register", false),
            ("465536.0", "465536.0", 0, "holding_register", true),
        ] {
            let parsed =
                <Component as address::Guest>::parse(raw.into(), "bool".into(), None).unwrap();
            assert_eq!(parsed.canonical, canonical);
            assert_eq!(parsed.bit, Some(bit));
            assert_eq!(parsed.word_count, 1);
            assert_eq!(
                parsed.index,
                Some(if raw.starts_with("465536") { 65535 } else { 0 })
            );
            assert_eq!(parsed.area, area_name);
            assert_eq!(parsed.writable_area, writable);
            assert_eq!(
                <Component as address::Guest>::offset(raw.into(), "bool".into(), None, 0).unwrap(),
                canonical
            );
        }
        assert_eq!(
            <Component as address::Guest>::offset_words("400001.3".into(), 1).unwrap(),
            "400002.3"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("300002.15".into(), -1).unwrap(),
            "300001.15"
        );
        assert!(
            <Component as address::Guest>::offset("400001.3".into(), "bool".into(), None, 1)
                .is_err()
        );
        assert!(
            <Component as address::Guest>::offset("400001.3".into(), "bool".into(), None, -1)
                .is_err()
        );
        assert!(<Component as address::Guest>::offset_words("465536.0".into(), 1).is_err());
        assert!(<Component as address::Guest>::offset_words("300001.15".into(), -1).is_err());
        for raw in [
            "40001.",
            "40001.3.4",
            "40001.F",
            "40001.16",
            "40001.003",
            "40001 .3",
            " 40001 .3 ",
            "40001\t.3",
            "00001.3",
            "10001.3",
            "40000.3",
            "465537.3",
            "4001.3",
        ] {
            assert!(
                <Component as address::Guest>::parse(raw.into(), "bool".into(), None).is_err(),
                "{raw}"
            );
        }
        for (raw, kind, words) in [
            ("40001.3", "u16", None),
            ("30001.15", "i32", None),
            ("40001.3", "bool", Some(1)),
            ("40001", "bool", None),
        ] {
            assert!(
                <Component as address::Guest>::parse(raw.into(), kind.into(), words).is_err(),
                "{raw} {kind}"
            );
        }
        assert!(<Component as address::Guest>::probe().is_none());
    }

    #[test]
    fn register_bit_fc3_fc4_read_and_mixed_plan_ignore_word_order() {
        for order in [WordOrder::LowHigh, WordOrder::HighLow] {
            let mut conn = conn();
            conn.word_order = order;
            let specs = vec![
                TagSpec {
                    address: "40001.0".into(),
                    data_type: "bool".into(),
                    string_words: None,
                },
                spec("40002"),
                TagSpec {
                    address: "30001.15".into(),
                    data_type: "bool".into(),
                    string_words: None,
                },
                TagSpec {
                    address: "40002.15".into(),
                    data_type: "bool".into(),
                    string_words: None,
                },
            ];
            let plans = planned_read(&conn, &specs);
            assert_eq!(plans.len(), 4);
            for (index, function, offset, value) in [
                (0, 3, 0, 0x0001u16),
                (1, 3, 1, 0x8000),
                (2, 4, 0, 0x8000),
                (3, 3, 1, 0x8000),
            ] {
                let plan = &plans[index];
                assert_eq!(plan.covers, vec![index as u32]);
                assert_eq!(plan.step.send[7], function);
                assert_eq!(&plan.step.send[8..12], &[0, offset, 0, 1]);
                let [hi, lo] = value.to_be_bytes();
                let response = [
                    plan.step.send[0],
                    plan.step.send[1],
                    0,
                    0,
                    0,
                    5,
                    1,
                    function,
                    2,
                    hi,
                    lo,
                ];
                let decoded = decode_planned(&conn, plan, &specs, &response).unwrap();
                assert!(matches!(
                    decoded.as_slice(),
                    [Ok(Value::Bool(true))] | [Ok(Value::Uint(32768))]
                ));
            }
            assert!(planned_read(
                &conn,
                &[TagSpec {
                    address: "40001.16".into(),
                    data_type: "bool".into(),
                    string_words: None
                }]
            )
            .is_empty());
        }
    }

    #[test]
    fn holding_register_bit_write_uses_one_fc22_mask_write() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist = parse_write_allowlist("400001-400010,000001-000010").unwrap();
        for (address, value, offset, and_mask, or_mask) in [
            ("40001.0", true, 0u16, 0xfffeu16, 0x0001u16),
            ("40001.0", false, 0, 0xfffe, 0),
            ("400010.15", true, 9, 0x7fff, 0x8000),
            ("40005.3", false, 4, 0xfff7, 0),
        ] {
            let item = write_item(address, "bool", Value::Bool(value));
            let (function, frame, echo) =
                preflight_write(&conn, std::slice::from_ref(&item), 7).unwrap();
            let expected = mask_write_register_request(7, 1, offset, and_mask, or_mask);
            assert_eq!((function, frame.as_slice()), (22, &expected[..]));
            assert_eq!(echo, &expected[8..14]);
        }
        let item = write_item("40002.3", "bool", Value::Bool(true));
        let sent = write_next_impl(&conn, vec![item.clone()], None).unwrap();
        let write_v1::WriteProgress::Send(planned) = sent else {
            panic!("FC22 request expected");
        };
        assert_eq!(
            planned.step.send,
            [0, 0, 0, 0, 0, 8, 1, 22, 0, 1, 0xff, 0xf7, 0, 8]
        );
        assert_eq!(planned.covers, [0]);
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], Some(planned.step.send.clone())),
            Ok(write_v1::WriteProgress::Done(results)) if matches!(results.as_slice(), [Ok(())])
        ));
        // A device without FC22 answers with an exception: item-level only.
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], Some(vec![0, 1, 0, 0, 0, 3, 1, 0x96, 1])),
            Ok(write_v1::WriteProgress::Done(results))
                if matches!(results.as_slice(), [Err(ItemError::Device(error))] if error.code == 1)
        ));
        // A different mask echo leaves the outcome uncertain and poisons writes.
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(
                &conn,
                vec![item.clone()],
                Some(vec![0, 2, 0, 0, 0, 8, 1, 22, 0, 1, 0xff, 0xf7, 0, 0])
            ),
            Err(FatalError::Protocol(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, vec![item], None),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn register_bit_write_disabled_refuses_fc22_before_send_and_keeps_other_writes() {
        use banto::driver::types::Setting;
        let settings = |mode: &str| {
            [
                ("host", "fixture.invalid"),
                ("port", "502"),
                ("write_mode", "enabled"),
                ("write_allowlist", "400001-400010,000001-000010"),
                ("register_bit_write", mode),
            ]
            .map(|(key, value)| Setting {
                key: key.into(),
                value: value.into(),
            })
        };
        assert!(check_settings(&settings("fc22")).is_empty());
        assert!(check_settings(&settings("disabled")).is_empty());
        for invalid in ["", "enabled", "FC22", "rmw"] {
            assert!(
                check_settings(&settings(invalid))
                    .iter()
                    .any(|error| error.key == "register_bit_write"),
                "{invalid:?}"
            );
        }
        let mut conn = conn();
        conn.write_enabled = true;
        conn.register_bit_write = false;
        conn.write_allowlist = parse_write_allowlist("400001-400010,000001-000010").unwrap();
        for value in [true, false] {
            let items = vec![write_item("40001.3", "bool", Value::Bool(value))];
            assert!(matches!(
                write_next_impl(&conn, items, None),
                Ok(write_v1::WriteProgress::Done(results))
                    if matches!(results.as_slice(), [Err(ItemError::Rejected(message))]
                        if message.contains("register_bit_write=disabled"))
            ));
            assert_eq!(conn.next_transaction_id.get(), 0);
            assert!(conn.pending_write.borrow().is_none());
            assert!(!conn.write_poisoned.get());
        }
        // Input register bits keep their input-only refusal.
        assert!(matches!(
            preflight_write(&conn, &[write_item("30001.3", "bool", Value::Bool(true))], 0),
            Err(message) if message == "input-only area is not writable"
        ));
        // Word and coil writes are unaffected.
        for (item, function) in [
            (write_item("40001", "u16", Value::Uint(7)), 16),
            (write_item("00001", "bool", Value::Bool(true)), 5),
        ] {
            assert_eq!(preflight_write(&conn, &[item], 0).unwrap().0, function);
        }
    }

    fn assert_rejected_before_send(conn: &ModbusConnection, items: Vec<write_v1::WriteItem>) {
        let count = items.len();
        let transaction_id = conn.next_transaction_id.get();
        assert!(
            matches!(
                write_next_impl(conn, items.clone(), None),
                Ok(write_v1::WriteProgress::Done(results))
                    if results.len() == count
                        && results.iter().all(|r| matches!(r, Err(ItemError::Rejected(_))))
            ),
            "{items:?}"
        );
        assert_eq!(conn.next_transaction_id.get(), transaction_id);
        assert!(conn.pending_write.borrow().is_none());
        assert!(!conn.write_poisoned.get());
    }

    fn coil_run(first: u16, values: &[bool]) -> Vec<write_v1::WriteItem> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                write_item(
                    &format!("{:05}", first + index as u16),
                    "bool",
                    Value::Bool(*value),
                )
            })
            .collect()
    }

    #[test]
    fn contiguous_coil_run_is_one_fc15_and_single_coil_keeps_fc5() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist =
            parse_write_allowlist("000001-000016,000017-000030,400001-400010").unwrap();
        // One coil is the unchanged FC5 frame.
        assert_eq!(
            preflight_write(&conn, &coil_run(3, &[true]), 0x1234).unwrap(),
            (
                5,
                vec![0x12, 0x34, 0, 0, 0, 6, 1, 5, 0, 2, 0xff, 0],
                vec![0, 2, 0xff, 0]
            )
        );
        // Ten coils 00001-00010 = 1011 0011 10, packed LSB-first.
        let values = [
            true, false, true, true, false, false, true, true, true, false,
        ];
        let items = coil_run(1, &values);
        let Ok(write_v1::WriteProgress::Send(planned)) =
            write_next_impl(&conn, items.clone(), None)
        else {
            panic!("FC15 request expected");
        };
        assert_eq!(
            planned.step.send,
            [0, 0, 0, 0, 0, 9, 1, 15, 0, 0, 0, 10, 2, 0xcd, 0x01]
        );
        assert_eq!(planned.covers, (0..10).collect::<Vec<u32>>());
        assert!(matches!(
            write_next_impl(&conn, items, Some(vec![0, 0, 0, 0, 0, 6, 1, 15, 0, 0, 0, 10])),
            Ok(write_v1::WriteProgress::Done(results))
                if results.len() == 10 && results.iter().all(Result::is_ok)
        ));
        assert!(!conn.write_poisoned.get());

        // An exception is item-level for every coil of the run.
        let items = coil_run(17, &[true, false, true]);
        assert!(matches!(
            write_next_impl(&conn, items.clone(), None),
            Ok(write_v1::WriteProgress::Send(planned))
                if planned.step.send == [0, 1, 0, 0, 0, 8, 1, 15, 0, 16, 0, 3, 1, 0b101]
        ));
        assert!(matches!(
            write_next_impl(&conn, items, Some(vec![0, 1, 0, 0, 0, 3, 1, 0x8f, 4])),
            Ok(write_v1::WriteProgress::Done(results))
                if results.len() == 3 && results.iter().all(|r| matches!(
                    r, Err(ItemError::Device(error)) if error.code == 4))
        ));
        assert!(!conn.write_poisoned.get());

        // A quantity mismatch is fatal and the request is not replayed.
        let items = coil_run(1, &[false, false]);
        assert!(matches!(
            write_next_impl(&conn, items.clone(), None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(
                &conn,
                items.clone(),
                Some(vec![0, 2, 0, 0, 0, 6, 1, 15, 0, 0, 0, 1])
            ),
            Err(FatalError::Protocol(_))
        ));
        assert!(conn.write_poisoned.get());
        assert!(matches!(
            write_next_impl(&conn, items, None),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn coil_runs_outside_one_contiguous_allowed_interval_are_rejected_before_send() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist =
            parse_write_allowlist("000001-000016,000017-000030,400001-400010").unwrap();
        for items in [
            // gap
            vec![
                write_item("00001", "bool", Value::Bool(true)),
                write_item("00003", "bool", Value::Bool(true)),
            ],
            // descending
            vec![
                write_item("00002", "bool", Value::Bool(true)),
                write_item("00001", "bool", Value::Bool(true)),
            ],
            // mixed areas
            vec![
                write_item("00001", "bool", Value::Bool(true)),
                write_item("40001", "u16", Value::Uint(1)),
            ],
            vec![
                write_item("00001", "bool", Value::Bool(true)),
                write_item("40001.0", "bool", Value::Bool(true)),
            ],
            // non-Bool value
            vec![
                write_item("00001", "bool", Value::Bool(true)),
                write_item("00002", "bool", Value::Uint(1)),
            ],
            // partly outside the allowlist
            coil_run(30, &[true, true]),
            // across two allowlist intervals
            coil_run(16, &[true, true]),
        ] {
            assert_rejected_before_send(&conn, items);
        }
        // The item cap still applies to coil runs.
        conn.write_allowlist = parse_write_allowlist("000001-000200").unwrap();
        assert_rejected_before_send(&conn, coil_run(1, &[true; 124]));
        assert_eq!(
            preflight_write(&conn, &coil_run(1, &[true; 123]), 0)
                .unwrap()
                .0,
            15
        );
    }

    #[test]
    fn register_write_fc6_sends_one_word_and_refuses_wider_writes_before_send() {
        use banto::driver::types::Setting;
        let settings = |mode: &str| {
            [
                ("host", "fixture.invalid"),
                ("port", "502"),
                ("write_mode", "enabled"),
                ("write_allowlist", "400001-400010,000001-000010"),
                ("register_write", mode),
            ]
            .map(|(key, value)| Setting {
                key: key.into(),
                value: value.into(),
            })
        };
        assert!(check_settings(&settings("fc16")).is_empty());
        assert!(check_settings(&settings("fc6")).is_empty());
        for invalid in ["", "FC6", "fc5", "single"] {
            assert!(
                check_settings(&settings(invalid))
                    .iter()
                    .any(|error| error.key == "register_write"),
                "{invalid:?}"
            );
        }
        let mut duplicate = settings("fc6").to_vec();
        duplicate.push(Setting {
            key: "register_write".into(),
            value: "fc16".into(),
        });
        assert!(check_settings(&duplicate)
            .iter()
            .any(|error| error.key == "register_write"));

        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist = parse_write_allowlist("400001-400010,000001-000010").unwrap();
        // fc16 is the default and keeps FC16 even for one word.
        assert_eq!(
            preflight_write(&conn, &[write_item("40002", "u16", Value::Uint(7))], 0).unwrap(),
            (
                16,
                vec![0, 0, 0, 0, 0, 9, 1, 16, 0, 1, 0, 1, 2, 0, 7],
                vec![0, 1, 0, 1]
            )
        );
        conn.register_write_fc6 = true;
        let item = vec![write_item("40002", "u16", Value::Uint(0xabcd))];
        assert!(matches!(
            write_next_impl(&conn, item.clone(), None),
            Ok(write_v1::WriteProgress::Send(planned))
                if planned.step.send == [0, 0, 0, 0, 0, 6, 1, 6, 0, 1, 0xab, 0xcd]
        ));
        assert!(matches!(
            write_next_impl(&conn, item, Some(vec![0, 0, 0, 0, 0, 6, 1, 6, 0, 1, 0xab, 0xcd])),
            Ok(write_v1::WriteProgress::Done(results)) if matches!(results.as_slice(), [Ok(())])
        ));
        let item = vec![write_item("40010", "i16", Value::Int(-2))];
        assert!(matches!(
            write_next_impl(&conn, item.clone(), None),
            Ok(write_v1::WriteProgress::Send(planned))
                if planned.step.send == [0, 1, 0, 0, 0, 6, 1, 6, 0, 9, 0xff, 0xfe]
        ));
        assert!(matches!(
            write_next_impl(&conn, item, Some(vec![0, 1, 0, 0, 0, 3, 1, 0x86, 1])),
            Ok(write_v1::WriteProgress::Done(results))
                if matches!(results.as_slice(), [Err(ItemError::Device(error))] if error.code == 1)
        ));

        // Anything wider than one word is refused before send.
        for items in [
            vec![
                write_item("40001", "u16", Value::Uint(1)),
                write_item("40002", "u16", Value::Uint(2)),
            ],
            vec![write_item("40001", "i32", Value::Int(-1))],
            vec![write_item("40001", "u32", Value::Uint(1))],
            vec![write_item("40001", "f32", Value::Float(1.5))],
            vec![write_item("40001", "i64", Value::Int(-1))],
            vec![write_item("40001", "u64", Value::Uint(1))],
            vec![write_item("40001", "f64", Value::Float(1.5))],
        ] {
            assert!(
                matches!(
                    preflight_write(&conn, &items, 0),
                    Err(message) if message.contains("register_write=fc6")
                ),
                "{items:?}"
            );
            assert_rejected_before_send(&conn, items);
        }
        assert_rejected_before_send(&conn, vec![write_item("40011", "u16", Value::Uint(1))]);

        // FC22 register bits and coils ignore register_write.
        for (items, function) in [
            (vec![write_item("40001.3", "bool", Value::Bool(true))], 22),
            (coil_run(1, &[true]), 5),
            (coil_run(1, &[true, false]), 15),
        ] {
            assert_eq!(preflight_write(&conn, &items, 0).unwrap().0, function);
        }

        // An echo with a different value is fatal; nothing is replayed.
        let item = vec![write_item("40001", "u16", Value::Uint(1))];
        assert!(matches!(
            write_next_impl(&conn, item.clone(), None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(
                &conn,
                item.clone(),
                Some(vec![0, 2, 0, 0, 0, 6, 1, 6, 0, 0, 0, 2])
            ),
            Err(FatalError::Protocol(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, item, None),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn register_bit_writes_outside_one_allowed_holding_point_are_rejected_before_send() {
        for enabled in [false, true] {
            let mut conn = conn();
            conn.write_enabled = enabled;
            conn.write_allowlist = parse_write_allowlist("400001-400010,000001-000010").unwrap();
            for items in [
                vec![write_item("40011.0", "bool", Value::Bool(true))],
                vec![write_item("40001.0", "bool", Value::Uint(1))],
                vec![
                    write_item("40001", "u16", Value::Uint(1)),
                    write_item("40002.15", "bool", Value::Bool(true)),
                ],
                vec![
                    write_item("40001.0", "bool", Value::Bool(false)),
                    write_item("40002", "u16", Value::Uint(2)),
                ],
                vec![
                    write_item("40001.0", "bool", Value::Bool(false)),
                    write_item("40001.1", "bool", Value::Bool(true)),
                ],
                vec![write_item("30001.15", "bool", Value::Bool(true))],
            ] {
                assert!(preflight_write(&conn, &items, 0).is_err());
                let progress = write_next_impl(&conn, items.clone(), None).unwrap();
                assert!(
                    matches!(progress, write_v1::WriteProgress::Done(results) if results.len() == items.len() && results.iter().all(|result| matches!(result, Err(ItemError::Rejected(_)))))
                );
                assert_eq!(conn.next_transaction_id.get(), 0);
                assert!(conn.pending_write.borrow().is_none());
            }
        }
    }

    fn conn() -> ModbusConnection {
        ModbusConnection {
            unit_id: 1,
            word_order: WordOrder::default(),
            next_transaction_id: Cell::new(0),
            probe_address: "40001".into(),
            handshake_pending: Cell::new(false),
            handshake_done: Cell::new(false),
            probe_poisoned: Cell::new(false),
            write_enabled: false,
            register_bit_write: true,
            register_write_fc6: false,
            write_allowlist: Vec::new(),
            pending_write: RefCell::new(None),
            write_poisoned: Cell::new(false),
        }
    }

    fn spec(address: &str) -> TagSpec {
        TagSpec {
            address: address.into(),
            data_type: "u16".into(),
            string_words: None,
        }
    }

    #[test]
    fn fc4_input_address_plan_and_decode_preserve_fc3_probe() {
        let parsed =
            <Component as address::Guest>::parse("30001".into(), "i32".into(), None).unwrap();
        assert_eq!(parsed.canonical, "300001");
        assert_eq!(parsed.area, "input_register");
        assert!(!parsed.writable_area);
        assert_eq!(
            <Component as address::Guest>::offset("300001".into(), "i32".into(), None, 1).unwrap(),
            "300003"
        );
        assert_eq!(
            <Component as address::Guest>::offset_words("300001".into(), 1).unwrap(),
            "300002"
        );
        assert!(parse_holding("365536", "u32", None).is_err());
        assert!(<Component as address::Guest>::probe().is_none());
        let conn = conn();
        let specs = [TagSpec {
            address: "30001".into(),
            data_type: "i32".into(),
            string_words: None,
        }];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[0].step.send, [0, 0, 0, 0, 0, 6, 1, 4, 0, 0, 0, 2]);
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[0],
                &specs,
                &[0, 0, 0, 0, 0, 7, 1, 4, 4, 0, 0, 0x80, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Int(-2147483648))])
        ));
        let high_low = ModbusConnection {
            unit_id: conn.unit_id,
            word_order: WordOrder::HighLow,
            next_transaction_id: Cell::new(0),
            probe_address: "40001".into(),
            handshake_pending: Cell::new(false),
            handshake_done: Cell::new(false),
            probe_poisoned: Cell::new(false),
            write_enabled: false,
            register_bit_write: true,
            register_write_fc6: false,
            write_allowlist: Vec::new(),
            pending_write: RefCell::new(None),
            write_poisoned: Cell::new(false),
        };
        assert!(matches!(
            decode_planned(
                &high_low,
                &plans[0],
                &specs,
                &[0, 0, 0, 0, 0, 7, 1, 4, 4, 0x80, 0, 0, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Int(-2147483648))])
        ));
        assert!(matches!(
            decode_planned(&conn, &plans[0], &specs, &[0, 0, 0, 0, 0, 3, 1, 0x84, 2]).as_deref(),
            Ok([Err(ItemError::Device(_))])
        ));
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[0],
                &specs,
                &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x80, 0]
            ),
            Err(FatalError::Framing(_))
        ));
    }

    #[test]
    fn address_and_probe_agree_with_fc3_scope() {
        assert_eq!(parse_holding("40001", "u16", None), Ok((0, Kind::U16)));
        assert_eq!(
            parse_holding("465536", "u16", None),
            Ok((u16::MAX, Kind::U16))
        );
        assert_eq!(
            parse_holding("465535", "u32", None),
            Ok((65_534, Kind::U32))
        );
        assert!(parse_holding("465536", "u32", None).is_err());
        assert_eq!(canonical(0), "400001");
        assert_eq!(parse_holding("30001", "u16", None), Ok((0, Kind::U16)));
        assert!(parse_holding("40001.3", "u16", None).is_err());
        assert_eq!(parse_holding("40001", "i16", None), Ok((0, Kind::I16)));
        assert_eq!(
            parse_holding("465536", "i16", None),
            Ok((u16::MAX, Kind::I16))
        );
        assert_eq!(
            parse_holding("465535", "i32", None),
            Ok((65_534, Kind::I32))
        );
        assert!(parse_holding("465536", "i32", None).is_err());
        for kind in ["i64", "u64", "f64"] {
            assert!(parse_holding("465533", kind, None).is_ok());
            assert!(parse_holding("465534", kind, None).is_err());
        }
        assert!(parse_holding("465535", "f32", None).is_ok());
        assert!(parse_holding("465536", "f32", None).is_err());
        assert!(parse_holding("40001", "u32", Some(2)).is_err());
        assert!(<Component as address::Guest>::probe().is_none());
    }

    #[test]
    fn plan_and_decode_use_core_without_extra_abi() {
        let conn = conn();
        let specs = [spec("40001")];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[0].step.send, [0, 0, 0, 0, 0, 6, 1, 3, 0, 0, 0, 1]);
        assert_eq!(plans[0].step.max_len, 260);
        assert!(matches!(
            plans[0].step.expect,
            session::FrameRule::LengthPrefixed(_)
        ));
        let outcome = decode_planned(
            &conn,
            &plans[0],
            &specs,
            &[0, 0, 0, 0, 0, 5, 1, 3, 2, 0, 42],
        );
        assert!(matches!(outcome.as_deref(), Ok([Ok(Value::Uint(42))])));
        let exception = decode_planned(&conn, &plans[0], &specs, &[0, 0, 0, 0, 0, 3, 1, 0x83, 2]);
        assert!(matches!(
            exception.as_deref(),
            Ok([Err(ItemError::Device(_))])
        ));
        let extra = decode_planned(
            &conn,
            &plans[0],
            &specs,
            &[0, 0, 0, 0, 0, 6, 1, 3, 2, 0, 42, 255],
        );
        assert!(matches!(extra, Err(FatalError::Framing(_))));
    }

    #[test]
    fn u32_plan_and_decode_respect_word_order_and_range() {
        let mut conn = conn();
        let specs = [TagSpec {
            address: "465535".into(),
            data_type: "u32".into(),
            string_words: None,
        }];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans[0].step.send, [0, 0, 0, 0, 0, 6, 1, 3, 255, 254, 0, 2]);
        let response = [0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd];
        assert!(matches!(
            decode_planned(&conn, &plans[0], &specs, &response).as_deref(),
            Ok([Ok(Value::Uint(0xabcd_1234))])
        ));
        conn.word_order = WordOrder::HighLow;
        assert!(matches!(
            decode_planned(&conn, &plans[0], &specs, &response).as_deref(),
            Ok([Ok(Value::Uint(0x1234_abcd))])
        ));
        let invalid = [TagSpec {
            address: "465536".into(),
            data_type: "u32".into(),
            string_words: None,
        }];
        assert!(planned_read(&conn, &invalid).is_empty());
    }

    #[test]
    fn signed_plan_decode_and_covers_keep_tag_values_distinct() {
        let mut conn = conn();
        let specs = [
            TagSpec {
                address: "465536".into(),
                data_type: "i16".into(),
                string_words: None,
            },
            TagSpec {
                address: "465535".into(),
                data_type: "i32".into(),
                string_words: None,
            },
        ];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[1].covers, [1]);
        assert_eq!(plans[0].step.send[8..], [0xff, 0xff, 0, 1]);
        assert_eq!(plans[1].step.send[8..], [0xff, 0xfe, 0, 2]);
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[0],
                &specs,
                &[0, 0, 0, 0, 0, 5, 1, 3, 2, 0x80, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Int(-32768))])
        ));
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[1],
                &specs,
                &[0, 1, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x80, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Int(-2147483648))])
        ));
        conn.word_order = WordOrder::HighLow;
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[1],
                &specs,
                &[0, 1, 0, 0, 0, 7, 1, 3, 4, 0x80, 0, 0, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Int(-2147483648))])
        ));
        let mismatch = read::Planned {
            covers: vec![0, 1],
            ..plans[1].clone()
        };
        assert!(matches!(
            decode_planned(&conn, &mismatch, &specs, &[]),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn numeric_four_word_and_float_results_use_existing_wit_variants() {
        let mut conn = conn();
        let specs = [
            TagSpec {
                address: "465533".into(),
                data_type: "i64".into(),
                string_words: None,
            },
            TagSpec {
                address: "400001".into(),
                data_type: "u64".into(),
                string_words: None,
            },
            TagSpec {
                address: "400001".into(),
                data_type: "f64".into(),
                string_words: None,
            },
            TagSpec {
                address: "465535".into(),
                data_type: "f32".into(),
                string_words: None,
            },
        ];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans.len(), 4);
        assert_eq!(
            plans
                .iter()
                .map(|p| p.covers.as_slice())
                .collect::<Vec<_>>(),
            vec![&[0][..], &[1][..], &[2][..], &[3][..]]
        );
        assert_eq!(plans[0].step.send[8..], [0xff, 0xfc, 0, 4]);
        assert_eq!(plans[3].step.send[8..], [0xff, 0xfe, 0, 2]);
        let i64_min = [0, 0, 0, 0, 0, 11, 1, 3, 8, 0, 0, 0, 0, 0, 0, 0x80, 0];
        assert!(matches!(
            decode_planned(&conn, &plans[0], &specs, &i64_min).as_deref(),
            Ok([Ok(Value::Int(i64::MIN))])
        ));
        let u64_precise = [0, 1, 0, 0, 0, 11, 1, 3, 8, 0, 1, 0, 0, 0, 0, 0, 0x20];
        assert!(matches!(
            decode_planned(&conn, &plans[1], &specs, &u64_precise).as_deref(),
            Ok([Ok(Value::Uint(9_007_199_254_740_993))])
        ));
        let f64_value = [0, 2, 0, 0, 0, 11, 1, 3, 8, 0, 0, 0, 0, 0, 0, 0x3f, 0xf8];
        assert!(matches!(
            decode_planned(&conn, &plans[2], &specs, &f64_value).as_deref(),
            Ok([Ok(Value::Float(1.5))])
        ));
        let f32_value = [0, 3, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x3f, 0xc0];
        assert!(matches!(
            decode_planned(&conn, &plans[3], &specs, &f32_value).as_deref(),
            Ok([Ok(Value::Float(1.5))])
        ));
        conn.word_order = WordOrder::HighLow;
        let f32_nan = [0, 3, 0, 0, 0, 7, 1, 3, 4, 0x7f, 0xc0, 0, 0];
        assert!(
            matches!(decode_planned(&conn, &plans[3], &specs, &f32_nan).as_deref(), Ok([Ok(Value::Float(n))]) if n.is_nan())
        );
    }

    #[test]
    fn offset_steps_by_kind_width_while_offset_words_stays_word_based() {
        let offset = |address: &str, kind: &str, n| {
            <Component as address::Guest>::offset(address.into(), kind.into(), None, n)
        };
        assert_eq!(offset("400001", "u32", 1).as_deref(), Ok("400003"));
        assert_eq!(offset("400003", "u32", -1).as_deref(), Ok("400001"));
        assert!(offset("400002", "u32", -1).is_err());
        assert_eq!(offset("465533", "u32", 1).as_deref(), Ok("465535"));
        assert!(offset("465534", "u32", 1).is_err());
        assert!(offset("400001", "u32", i64::MAX).is_err());
        assert!(offset("400001", "u32", i64::MIN).is_err());
        assert_eq!(offset("400001", "u16", 1).as_deref(), Ok("400002"));
        assert_eq!(offset("400001", "i16", 1).as_deref(), Ok("400002"));
        assert_eq!(offset("400001", "i32", 1).as_deref(), Ok("400003"));
        assert_eq!(offset("465533", "i32", 1).as_deref(), Ok("465535"));
        assert!(offset("465534", "i32", 1).is_err());
        assert_eq!(offset("400001", "f64", 1).as_deref(), Ok("400005"));
        assert_eq!(offset("465529", "i64", 1).as_deref(), Ok("465533"));
        assert!(offset("465530", "u64", 1).is_err());
        assert_eq!(
            <Component as address::Guest>::offset_words("400001".into(), 1).as_deref(),
            Ok("400002")
        );
    }

    #[test]
    fn config_rejects_non_tcp_and_bad_unit() {
        use banto::driver::types::Setting;
        let settings = [
            Setting {
                key: "host".into(),
                value: "example.test".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
            Setting {
                key: "transport".into(),
                value: "serial_over_tcp".into(),
            },
            Setting {
                key: "unit_id".into(),
                value: "256".into(),
            },
        ];
        let errors = check_settings(&settings);
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|error| error.key == "transport"));
        assert!(errors.iter().any(|error| error.key == "unit_id"));
    }

    #[test]
    fn config_rejects_duplicate_keys() {
        use banto::driver::types::Setting;
        let settings = [
            Setting {
                key: "host".into(),
                value: "first".into(),
            },
            Setting {
                key: "host".into(),
                value: "second".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
        ];
        let errors = check_settings(&settings);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].key, "host");
    }

    fn write_item(address: &str, data_type: &str, value: Value) -> write_v1::WriteItem {
        write_v1::WriteItem {
            spec: TagSpec {
                address: address.into(),
                data_type: data_type.into(),
                string_words: None,
            },
            value,
        }
    }

    #[test]
    fn fc1_fc2_connection_probe_reads_one_configured_bit() {
        for (address, function, offset) in [("00001", 1, 0u8), ("165536", 2, 0xFF)] {
            let mut probe_conn = conn();
            probe_conn.probe_address = address.into();
            let first =
                <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                    .unwrap()
                    .unwrap();
            assert_eq!(
                first.send,
                [0, 0, 0, 0, 0, 6, 1, function, offset, offset, 0, 1],
                "{address}"
            );
            assert!(
                <ModbusConnection as session::GuestConnection>::handshake_next(
                    &probe_conn,
                    Some(vec![0, 0, 0, 0, 0, 4, 1, function, 1, 0])
                )
                .unwrap()
                .is_none(),
                "{address}"
            );
            assert!(probe_conn.handshake_done.get());
            assert!(!probe_conn.probe_poisoned.get());
        }

        // A register-shaped reply, an exception and a stale transaction ID
        // all fail the handshake and leave the probe outcome uncertain.
        for response in [
            vec![0, 0, 0, 0, 0, 5, 1, 1, 2, 0, 1],
            vec![0, 0, 0, 0, 0, 3, 1, 0x81, 2],
            vec![0, 1, 0, 0, 0, 4, 1, 1, 1, 1],
            vec![0, 0, 0, 0, 0, 4, 1, 2, 1, 1],
            vec![0, 0, 0, 0, 0, 4, 1, 1, 1, 2],
        ] {
            let mut probe_conn = conn();
            probe_conn.probe_address = "00001".into();
            <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                .unwrap()
                .unwrap();
            assert!(
                <ModbusConnection as session::GuestConnection>::handshake_next(
                    &probe_conn,
                    Some(response.clone())
                )
                .is_err(),
                "{response:?}"
            );
            assert!(probe_conn.probe_poisoned.get());
            assert!(
                <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                    .is_err()
            );
        }
    }

    #[test]
    fn fc4_connection_probe_uses_configured_input_register() {
        let mut probe_conn = conn();
        probe_conn.probe_address = "30001".into();
        let first =
            <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                .unwrap()
                .unwrap();
        assert_eq!(first.send, [0, 0, 0, 0, 0, 6, 1, 4, 0, 0, 0, 1]);
        assert!(
            <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                .is_err()
        );
        assert!(
            <ModbusConnection as session::GuestConnection>::handshake_next(
                &probe_conn,
                Some(vec![0, 0, 0, 0, 0, 5, 1, 4, 2, 0, 7])
            )
            .unwrap()
            .is_none()
        );
        assert!(
            <ModbusConnection as session::GuestConnection>::handshake_next(&probe_conn, None)
                .unwrap()
                .is_none()
        );
        let default = conn();
        assert_eq!(
            <ModbusConnection as session::GuestConnection>::handshake_next(&default, None)
                .unwrap()
                .unwrap()
                .send[7],
            3
        );
        assert!(
            <ModbusConnection as session::GuestConnection>::handshake_next(
                &default,
                Some(vec![0, 0, 0, 0, 0, 3, 1, 0x83, 2])
            )
            .is_err()
        );
        assert!(
            <ModbusConnection as session::GuestConnection>::handshake_next(&default, None).is_err()
        );
    }

    #[test]
    fn fc1_fc2_bool_address_plan_decode_and_input_write_rejection() {
        let conn = conn();
        for (address, function, area_name, writable) in [
            ("00001", 1, "coil", true),
            ("10001", 2, "discrete_input", false),
        ] {
            let parsed =
                <Component as address::Guest>::parse(address.into(), "bool".into(), None).unwrap();
            assert_eq!(parsed.area, area_name);
            assert_eq!(parsed.writable_area, writable);
            assert_eq!(parsed.word_count, 1);
            assert_eq!(
                parsed.canonical,
                format!("{}00001", if function == 1 { 0 } else { 1 })
            );
            let spec = TagSpec {
                address: address.into(),
                data_type: "bool".into(),
                string_words: None,
            };
            let planned = planned_read(&conn, std::slice::from_ref(&spec));
            assert_eq!(planned[0].step.send[7], function);
            let response = [
                planned[0].step.send[0],
                planned[0].step.send[1],
                0,
                0,
                0,
                4,
                1,
                function,
                1,
                1,
            ];
            assert!(matches!(
                decode_planned(&conn, &planned[0], &[spec], &response).as_deref(),
                Ok([Ok(Value::Bool(true))])
            ));
        }
        assert!(
            <Component as address::Guest>::parse("165537".into(), "bool".into(), None).is_err()
        );
        assert!(<Component as address::Guest>::parse("00001".into(), "u16".into(), None).is_err());
        assert_eq!(
            <Component as address::Guest>::offset("000001".into(), "bool".into(), None, 1)
                .as_deref(),
            Ok("000002")
        );
    }

    #[test]
    fn write_preflight_checks_every_item_and_pending_ack() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist = parse_write_allowlist("000001-000016,400001-400010").unwrap();
        let items = vec![
            write_item("40001", "u16", Value::Uint(7)),
            write_item("40002", "i32", Value::Int(-1)),
        ];
        let (function, frame, echo) = preflight_write(&conn, &items, 0).unwrap();
        assert_eq!(function, 16);
        assert_eq!(echo, [0, 0, 0, 3]);
        assert_eq!(&frame[13..], &[0, 7, 255, 255, 255, 255]);
        let invalid = vec![items[0].clone(), write_item("30002", "u16", Value::Uint(1))];
        assert!(preflight_write(&conn, &invalid, 0).is_err());
        assert!(
            preflight_write(&conn, &[write_item("40001", "u16", Value::Uint(65536))], 0).is_err()
        );
        assert!(
            preflight_write(&conn, &[write_item("40001", "i16", Value::Int(-32769))], 0).is_err()
        );
        assert!(preflight_write(
            &conn,
            &[write_item("40001", "f32", Value::Float(f64::INFINITY))],
            0
        )
        .is_err());
        assert!(
            preflight_write(&conn, &[write_item("10001", "bool", Value::Bool(true))], 0).is_err()
        );
        assert!(preflight_write(&conn, &[write_item("00001", "bool", Value::Uint(1))], 0).is_err());
        assert!(preflight_write(
            &conn,
            &[
                write_item("40001", "u16", Value::Uint(1)),
                write_item("40003", "u16", Value::Uint(2))
            ],
            0
        )
        .is_err());
        assert!(matches!(
            write_next_impl(&conn, invalid, None),
            Ok(write_v1::WriteProgress::Done(_))
        ));
        assert_eq!(conn.next_transaction_id.get(), 0);
        let sent = write_next_impl(&conn, items.clone(), None).unwrap();
        assert!(matches!(sent, write_v1::WriteProgress::Send(_)));
        assert_eq!(conn.next_transaction_id.get(), 1);
        assert!(matches!(
            write_next_impl(
                &conn,
                items.clone(),
                Some(vec![0, 0, 0, 0, 0, 6, 1, 16, 0, 0, 0, 3])
            ),
            Ok(write_v1::WriteProgress::Done(_))
        ));
        let coil = write_item("00001", "bool", Value::Bool(true));
        assert!(matches!(
            write_next_impl(&conn, vec![coil.clone()], None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(
                &conn,
                vec![coil],
                Some(vec![0, 1, 0, 0, 0, 6, 1, 5, 0, 0, 0, 0])
            ),
            Err(FatalError::Protocol(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, items, None),
            Err(FatalError::Protocol(_))
        ));
    }

    #[test]
    fn probe_and_write_settings_fail_closed() {
        use banto::driver::types::Setting;
        let base = [
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
        ];
        let mut conn = conn();
        conn.write_allowlist = parse_write_allowlist("400001-400010").unwrap();
        assert!(matches!(
            write_next_impl(
                &conn,
                vec![write_item("40001", "u16", Value::Uint(1))],
                None
            ),
            Ok(write_v1::WriteProgress::Done(_))
        ));
        for (key, value) in [
            ("probe_address", "40001.3"),
            ("probe_address", "30001.0"),
            ("probe_address", "00000"),
            ("probe_address", "165537"),
            ("probe_address", "200001"),
            ("write_mode", "yes"),
            ("register_bit_write", "no"),
            ("register_write", "fc5"),
            ("write_allowlist", "300001-300002"),
            ("write_allowlist", "000001-400001"),
            ("write_allowlist", "400001-400004,400004-400005"),
        ] {
            let mut settings = base.to_vec();
            settings.push(Setting {
                key: key.into(),
                value: value.into(),
            });
            assert!(
                check_settings(&settings)
                    .iter()
                    .any(|error| error.key == key),
                "{key}={value}"
            );
        }
        let mut settings = base.to_vec();
        settings.push(Setting {
            key: "write_mode".into(),
            value: "enabled".into(),
        });
        assert!(check_settings(&settings)
            .iter()
            .any(|error| error.key == "write_allowlist"));
    }

    #[test]
    fn all_numeric_write_widths_roundtrip_both_word_orders() {
        let cases = [
            ("i16", Value::Int(i16::MIN as i64)),
            ("u16", Value::Uint(u16::MAX as u64)),
            ("i32", Value::Int(i32::MIN as i64)),
            ("u32", Value::Uint(u32::MAX as u64)),
            ("i64", Value::Int(i64::MIN)),
            ("u64", Value::Uint(u64::MAX)),
            ("f32", Value::Float(1.5)),
            ("f64", Value::Float(-1.5)),
        ];
        for order in [WordOrder::LowHigh, WordOrder::HighLow] {
            let mut conn = conn();
            conn.word_order = order;
            conn.write_allowlist = parse_write_allowlist("400001-400010").unwrap();
            for (name, value) in &cases {
                let item = write_item("400001", name, value.clone());
                let (_, frame, _) = preflight_write(&conn, &[item], 42).unwrap();
                let (_, kind) = parse_holding("400001", name, None).unwrap();
                assert_eq!(frame.len(), 13 + kind.word_count() as usize * 2);
                let data = &frame[13..];
                let mut response = vec![
                    0,
                    42,
                    0,
                    0,
                    0,
                    (3 + data.len()) as u8,
                    1,
                    3,
                    data.len() as u8,
                ];
                response.extend_from_slice(data);
                let decoded = ReadHoldingNumeric {
                    transaction_id: 42,
                    unit_id: 1,
                    offset: 0,
                    kind: kind.core_kind(),
                    word_order: order,
                }
                .decode(&response)
                .unwrap();
                assert!(match (decoded, value) {
                    (NumericValue::Int(a), Value::Int(b)) => a == *b,
                    (NumericValue::Uint(a), Value::Uint(b)) => a == *b,
                    (NumericValue::Float(a), Value::Float(b)) => a == *b,
                    _ => false,
                });
            }
        }
    }

    fn string_spec(address: &str, words: Option<u32>) -> TagSpec {
        TagSpec {
            address: address.into(),
            data_type: "string".into(),
            string_words: words,
        }
    }

    fn string_item(address: &str, words: u32, bytes: &[u8]) -> write_v1::WriteItem {
        write_v1::WriteItem {
            spec: string_spec(address, Some(words)),
            value: Value::Bytes(bytes.to_vec()),
        }
    }

    #[test]
    fn string_parse_offset_and_supported_kinds_follow_the_numeric_register_form() {
        let kinds = <Component as address::Guest>::supported_kinds();
        assert_eq!(kinds.iter().filter(|kind| *kind == "string").count(), 1);
        for (raw, words, canonical, area_name, writable, index) in [
            ("40001", 4, "400001", "holding_register", true, 0),
            (" 400010 ", 1, "400010", "holding_register", true, 9),
            ("30001", 123, "300001", "input_register", false, 0),
            ("465536", 1, "465536", "holding_register", true, 65_535),
            ("465414", 123, "465414", "holding_register", true, 65_413),
        ] {
            let parsed =
                <Component as address::Guest>::parse(raw.into(), "string".into(), Some(words))
                    .unwrap();
            assert_eq!(parsed.canonical, canonical, "{raw}");
            assert_eq!(parsed.area, area_name);
            assert_eq!(parsed.writable_area, writable);
            assert_eq!(parsed.bit, None);
            assert_eq!(parsed.word_count, words);
            assert_eq!(parsed.index, Some(index));
            // Same canonical and index as a numeric tag at the same start.
            let numeric =
                <Component as address::Guest>::parse(raw.into(), "u16".into(), None).unwrap();
            assert_eq!(
                (numeric.canonical, numeric.area, numeric.index),
                (parsed.canonical, parsed.area, parsed.index)
            );
        }
        for (raw, data_type, words) in [
            ("40001", "string", Some(0)),
            ("40001", "string", Some(124)),
            ("40001", "string", None),
            ("465415", "string", Some(123)),
            ("465536", "string", Some(2)),
            ("00001", "string", Some(1)),
            ("10001", "string", Some(1)),
            ("40001.3", "string", Some(1)),
            ("40000", "string", Some(1)),
            ("200001", "string", Some(1)),
            ("40001", "u16", Some(1)),
            ("40001", "bool", Some(1)),
            ("00001", "bool", Some(1)),
            ("40001", "STRING", Some(1)),
        ] {
            assert!(
                <Component as address::Guest>::parse(raw.into(), data_type.into(), words).is_err(),
                "{raw} {data_type} {words:?}"
            );
        }
        let offset = |address: &str, words, n| {
            <Component as address::Guest>::offset(address.into(), "string".into(), Some(words), n)
        };
        assert_eq!(offset("400001", 10, 1).as_deref(), Ok("400011"));
        assert_eq!(offset("400011", 10, -1).as_deref(), Ok("400001"));
        assert_eq!(offset("300001", 4, 2).as_deref(), Ok("300009"));
        assert_eq!(offset("400001", 4, 0).as_deref(), Ok("400001"));
        assert!(offset("400005", 10, -1).is_err());
        assert_eq!(offset("465527", 10, 0).as_deref(), Ok("465527"));
        assert!(offset("465517", 10, 1).is_ok());
        assert!(offset("465518", 10, 1).is_err());
        assert!(offset("400001", 10, i64::MAX).is_err());
        assert!(offset("400001", 0, 1).is_err());
        assert_eq!(
            <Component as address::Guest>::offset_words("400001".into(), 3).as_deref(),
            Ok("400004")
        );
    }

    #[test]
    fn string_read_is_one_fc3_or_fc4_span_returned_as_raw_wire_bytes() {
        let conn = conn();
        let specs = [
            spec("40001"),
            string_spec("40002", Some(3)),
            string_spec("30101", Some(2)),
            TagSpec {
                address: "40005".into(),
                data_type: "u32".into(),
                string_words: None,
            },
            string_spec("40001", Some(124)),
        ];
        let plans = planned_read(&conn, &specs);
        // One request per tag; an invalid string is left uncovered.
        assert_eq!(
            plans.iter().map(|p| p.covers.clone()).collect::<Vec<_>>(),
            [[0], [1], [2], [3]]
        );
        assert_eq!(plans[1].step.send, [0, 1, 0, 0, 0, 6, 1, 3, 0, 1, 0, 3]);
        assert_eq!(plans[2].step.send, [0, 2, 0, 0, 0, 6, 1, 4, 0, 100, 0, 2]);
        // Character order is wire order; NUL and non-UTF-8 bytes are kept.
        let data = [b'A', b'B', 0, b'C', 0xff, 0];
        let mut response = vec![0, 1, 0, 0, 0, 9, 1, 3, 6];
        response.extend_from_slice(&data);
        assert!(matches!(
            decode_planned(&conn, &plans[1], &specs, &response).as_deref(),
            Ok([Ok(Value::Bytes(bytes))]) if bytes == &data
        ));
        assert!(matches!(
            decode_planned(&conn, &plans[2], &specs, &[0, 2, 0, 0, 0, 7, 1, 4, 4, 0, 0, 0, 0]).as_deref(),
            Ok([Ok(Value::Bytes(bytes))]) if bytes == &[0, 0, 0, 0]
        ));
        // Numerics around the string keep their values.
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[0],
                &specs,
                &[0, 0, 0, 0, 0, 5, 1, 3, 2, 0, 42]
            )
            .as_deref(),
            Ok([Ok(Value::Uint(42))])
        ));
        assert!(matches!(
            decode_planned(
                &conn,
                &plans[3],
                &specs,
                &[0, 3, 0, 0, 0, 7, 1, 3, 4, 0, 1, 0, 0]
            )
            .as_deref(),
            Ok([Ok(Value::Uint(1))])
        ));
        // A reply that is not exactly 2 n bytes, or of the other table, is fatal.
        for bad in [
            vec![0, 1, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0, 0],
            vec![0, 1, 0, 0, 0, 11, 1, 3, 8, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![0, 1, 0, 0, 0, 9, 1, 4, 6, 0, 0, 0, 0, 0, 0],
        ] {
            assert!(matches!(
                decode_planned(&conn, &plans[1], &specs, &bad),
                Err(FatalError::Framing(_))
            ));
        }
        assert!(matches!(
            decode_planned(&conn, &plans[1], &specs, &[0, 1, 0, 0, 0, 3, 1, 0x83, 2]).as_deref(),
            Ok([Err(ItemError::Device(error))]) if error.code == 2 && error.message.contains("FC=0x03")
        ));
        // A plan for a different span no longer matches its tag.
        let other = [spec("40001"), string_spec("40002", Some(4))];
        assert!(matches!(
            decode_planned(&conn, &plans[1], &other, &response),
            Err(FatalError::Protocol(_))
        ));

        // The largest string fits the 260-byte response cap.
        let specs = [string_spec("400001", Some(123))];
        let plans = planned_read(&conn, &specs);
        assert_eq!(plans[0].step.send[10..12], [0, 123]);
        let tid = [plans[0].step.send[0], plans[0].step.send[1]];
        let mut response = vec![tid[0], tid[1], 0, 0, 0, 249, 1, 3, 246];
        response.extend((0..246).map(|n| n as u8));
        assert!(response.len() <= MAX_RESPONSE_LEN);
        assert!(matches!(
            decode_planned(&conn, &plans[0], &specs, &response).as_deref(),
            Ok([Ok(Value::Bytes(bytes))]) if bytes.len() == 246 && bytes[..] == response[9..]
        ));
    }

    #[test]
    fn string_write_is_one_fc16_over_the_whole_allowed_span() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist = parse_write_allowlist("400001-400010,400101-400223").unwrap();
        let item = string_item("40002", 3, b"AB\0C\xff\0");
        let (function, frame, echo) =
            preflight_write(&conn, std::slice::from_ref(&item), 9).unwrap();
        assert_eq!(function, 16);
        assert_eq!(
            frame,
            [0, 9, 0, 0, 0, 13, 1, 16, 0, 1, 0, 3, 6, b'A', b'B', 0, b'C', 0xff, 0]
        );
        assert_eq!(echo, [0, 1, 0, 3]);
        let Ok(write_v1::WriteProgress::Send(planned)) =
            write_next_impl(&conn, vec![item.clone()], None)
        else {
            panic!("FC16 request expected");
        };
        assert_eq!(planned.covers, [0]);
        assert_eq!(planned.step.send[7], 16);
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], Some(vec![0, 0, 0, 0, 0, 6, 1, 16, 0, 1, 0, 3])),
            Ok(write_v1::WriteProgress::Done(results)) if matches!(results.as_slice(), [Ok(())])
        ));
        // An exception is item-level; a wrong quantity echo poisons writes.
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], Some(vec![0, 1, 0, 0, 0, 3, 1, 0x90, 2])),
            Ok(write_v1::WriteProgress::Done(results))
                if matches!(results.as_slice(), [Err(ItemError::Device(error))] if error.code == 2)
        ));
        assert!(matches!(
            write_next_impl(&conn, vec![item.clone()], None),
            Ok(write_v1::WriteProgress::Send(_))
        ));
        assert!(matches!(
            write_next_impl(
                &conn,
                vec![item.clone()],
                Some(vec![0, 2, 0, 0, 0, 6, 1, 16, 0, 1, 0, 2])
            ),
            Err(FatalError::Protocol(_))
        ));
        assert!(matches!(
            write_next_impl(&conn, vec![item], None),
            Err(FatalError::Protocol(_))
        ));

        // The 123-word maximum is one FC16 inside one interval.
        let conn = {
            let mut conn = self::conn();
            conn.write_enabled = true;
            conn.write_allowlist = parse_write_allowlist("400001-400010,400101-400223").unwrap();
            conn
        };
        let bytes: Vec<u8> = (0..246).map(|n| n as u8).collect();
        let (function, frame, echo) =
            preflight_write(&conn, &[string_item("400101", 123, &bytes)], 0).unwrap();
        assert_eq!(
            (function, frame.len(), &echo[..]),
            (16, 13 + 246, &[0, 100, 0, 123][..])
        );
        assert_eq!(frame[13..], bytes[..]);
    }

    #[test]
    fn string_writes_outside_the_contract_are_rejected_before_send() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.write_allowlist =
            parse_write_allowlist("400001-400010,400011-400020,000001-000010").unwrap();
        for items in [
            // span partly outside, or across two intervals
            vec![string_item("40009", 3, &[0; 6])],
            vec![string_item("40010", 2, &[0; 4])],
            // wrong byte length: no truncation or padding
            vec![string_item("40001", 2, b"ABC")],
            vec![string_item("40001", 2, b"ABCDE")],
            vec![string_item("40001", 2, b"")],
            // non-bytes value
            vec![write_v1::WriteItem {
                spec: string_spec("40001", Some(1)),
                value: Value::Uint(0x4142),
            }],
            // input registers and missing or invalid string_words
            vec![string_item("30001", 1, b"AB")],
            vec![write_v1::WriteItem {
                spec: string_spec("40001", None),
                value: Value::Bytes(b"AB".to_vec()),
            }],
            vec![string_item("40001", 0, b"")],
            vec![string_item("40001", 124, &[0; 248])],
            vec![string_item("00001", 1, b"AB")],
            vec![string_item("40001.0", 1, b"AB")],
            // mixed with other items, either order, or two strings
            vec![
                string_item("40001", 1, b"AB"),
                write_item("40002", "u16", Value::Uint(1)),
            ],
            vec![
                write_item("40001", "u16", Value::Uint(1)),
                string_item("40002", 1, b"AB"),
            ],
            vec![
                string_item("40001", 1, b"AB"),
                string_item("40002", 1, b"CD"),
            ],
            // bytes for non-string tags
            vec![write_item("40001", "u16", Value::Bytes(b"AB".to_vec()))],
            vec![write_item("00001", "bool", Value::Bytes(b"\x01".to_vec()))],
            vec![write_item(
                "40001.0",
                "bool",
                Value::Bytes(b"\x01".to_vec()),
            )],
        ] {
            assert_rejected_before_send(&conn, items);
        }
        conn.write_enabled = false;
        assert!(matches!(
            write_next_impl(&conn, vec![string_item("40001", 1, b"AB")], None),
            Ok(write_v1::WriteProgress::Done(results))
                if matches!(results.as_slice(), [Err(ItemError::Rejected(message))] if message == "writes are disabled")
        ));
    }

    #[test]
    fn string_write_under_register_write_fc6_fits_only_one_word() {
        let mut conn = conn();
        conn.write_enabled = true;
        conn.register_write_fc6 = true;
        conn.write_allowlist = parse_write_allowlist("400001-400010").unwrap();
        let item = string_item("40003", 1, b"Hi");
        let Ok(write_v1::WriteProgress::Send(planned)) =
            write_next_impl(&conn, vec![item.clone()], None)
        else {
            panic!("FC6 request expected");
        };
        assert_eq!(
            planned.step.send,
            [0, 0, 0, 0, 0, 6, 1, 6, 0, 2, b'H', b'i']
        );
        assert!(matches!(
            write_next_impl(&conn, vec![item], Some(vec![0, 0, 0, 0, 0, 6, 1, 6, 0, 2, b'H', b'i'])),
            Ok(write_v1::WriteProgress::Done(results)) if matches!(results.as_slice(), [Ok(())])
        ));
        let wide = vec![string_item("40001", 2, b"ABCD")];
        assert!(matches!(
            preflight_write(&conn, &wide, 0),
            Err(message) if message.contains("register_write=fc6")
        ));
        assert_rejected_before_send(&conn, wide);
    }
}
