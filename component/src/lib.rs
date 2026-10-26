//! Read-only Modbus TCP component for the full `banto:driver/driver` world.
//! The host owns sockets, timeouts, retries, and connection policy.

use std::cell::Cell;

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_modbus_read_core::{
    NumericKind, NumericValue, ReadError, ReadHoldingNumeric, WordOrder, MAX_RESPONSE_LEN,
};
use exports::banto::driver::{address, config, read, session};

wit_bindgen::generate!({ path: "../wit", world: "driver" });

struct Component;

struct ModbusConnection {
    unit_id: u8,
    word_order: WordOrder,
    next_transaction_id: Cell<u16>,
}

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
    errors
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
        _ => return Err("FC3 supports i16/u16/i32/u32/i64/u64/f32/f64 holding registers".into()),
    };
    let text = text.trim();
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 5 | 6) || bytes[0] != b'4' || !bytes.iter().all(u8::is_ascii_digit) {
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

fn canonical(offset: u16) -> String {
    format!("4{:05}", u32::from(offset) + 1)
}

fn request_bytes(
    conn: &ModbusConnection,
    transaction_id: u16,
    offset: u16,
    kind: Kind,
) -> [u8; 12] {
    ReadHoldingNumeric {
        transaction_id,
        unit_id: conn.unit_id,
        offset,
        kind: kind.core_kind(),
        word_order: conn.word_order,
    }
    .request()
    .expect("validated numeric start offset")
}

fn step(send: [u8; 12]) -> session::Step {
    session::Step {
        send: send.to_vec(),
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

fn planned_read(conn: &ModbusConnection, specs: &[TagSpec]) -> Vec<read::Planned> {
    specs
        .iter()
        .enumerate()
        .filter_map(|(index, spec)| {
            let (offset, kind) =
                parse_holding(&spec.address, &spec.data_type, spec.string_words).ok()?;
            let transaction_id = conn.next_transaction_id.get();
            conn.next_transaction_id.set(transaction_id.wrapping_add(1));
            Some(read::Planned {
                step: step(request_bytes(conn, transaction_id, offset, kind)),
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
    let (offset, kind) = parse_holding(&spec.address, &spec.data_type, spec.string_words)
        .map_err(FatalError::Protocol)?;
    let send: [u8; 12] = p
        .step
        .send
        .as_slice()
        .try_into()
        .map_err(|_| FatalError::Protocol("FC3 要求長が不正です".into()))?;
    let transaction_id = u16::from_be_bytes([send[0], send[1]]);
    if send != request_bytes(conn, transaction_id, offset, kind) {
        return Err(FatalError::Protocol(
            "計画と要求バイトが一致しません".into(),
        ));
    }
    let decoded = ReadHoldingNumeric {
        transaction_id,
        unit_id: conn.unit_id,
        offset,
        kind: kind.core_kind(),
        word_order: conn.word_order,
    }
    .decode(response);
    let value = match decoded {
        Ok(NumericValue::Int(value)) => Ok(Value::Int(value)),
        Ok(NumericValue::Uint(value)) => Ok(Value::Uint(value)),
        Ok(NumericValue::Float(value)) => Ok(Value::Float(value)),
        Err(ReadError::Device(code)) => Err(ItemError::Device(banto::driver::types::DeviceError {
            code: u32::from(code),
            message: format!("FC=0x03 Modbus exception 0x{code:02X}"),
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
        }))
    }

    fn handshake_next(
        &self,
        _prev_response: Option<Vec<u8>>,
    ) -> Result<Option<session::Step>, FatalError> {
        Ok(None)
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
        Ok(planned_read(conn.get::<ModbusConnection>(), &specs))
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

impl address::Guest for Component {
    fn parse(
        text: String,
        data_type: String,
        string_words: Option<u32>,
    ) -> Result<address::Parsed, String> {
        let (offset, kind) = parse_holding(&text, &data_type, string_words)?;
        Ok(address::Parsed {
            canonical: canonical(offset),
            area: "holding_register".into(),
            writable_area: true,
            bit: None,
            word_count: kind.word_count(),
            index: Some(u64::from(offset)),
        })
    }

    fn offset(
        canonical: String,
        data_type: String,
        string_words: Option<u32>,
        n: i64,
    ) -> Result<String, String> {
        let (offset, kind) = parse_holding(&canonical, &data_type, string_words)?;
        let words = i64::from(kind.word_count())
            .checked_mul(n)
            .ok_or("オフセットが範囲外です")?;
        let next = i64::from(offset)
            .checked_add(words)
            .ok_or("オフセットが範囲外です")?;
        canonical_for(next, kind)
    }

    fn supported_kinds() -> Vec<String> {
        ["u16", "u32", "i16", "i32", "i64", "u64", "f32", "f64"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn offset_words(canonical: String, n: i64) -> Result<String, String> {
        let (offset, _) = parse_holding(&canonical, "u16", None)?;
        let next = i64::from(offset)
            .checked_add(n)
            .ok_or("オフセットが範囲外です")?;
        canonical_for(next, Kind::U16)
    }

    fn help() -> String {
        "FC3 numeric holding registers: 1/2/4 words; final starts 465536/465535/465533 respectively"
            .into()
    }

    fn probe() -> Option<TagSpec> {
        Some(TagSpec {
            address: "40001".into(),
            data_type: "u16".into(),
            string_words: None,
        })
    }
}

fn canonical_for(offset: i64, kind: Kind) -> Result<String, String> {
    let offset = u16::try_from(offset).map_err(|_| "保持レジスタの範囲を超えています")?;
    if u32::from(offset) + kind.word_count() > u32::from(u16::MAX) + 1 {
        return Err("numeric tag exceeds holding-register address space".into());
    }
    Ok(canonical(offset))
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
        ]
    }
}

#[cfg(target_arch = "wasm32")]
export!(Component);

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> ModbusConnection {
        ModbusConnection {
            unit_id: 1,
            word_order: WordOrder::default(),
            next_transaction_id: Cell::new(0),
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
        assert!(parse_holding("30001", "u16", None).is_err());
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
        assert_eq!(
            <Component as address::Guest>::probe().unwrap().address,
            "40001"
        );
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
}
