//! Read-only Modbus TCP component for the full `banto:driver/driver` world.
//! The host owns sockets, timeouts, retries, and connection policy.

use std::cell::Cell;

use banto::driver::types::{FatalError, ItemError, TagSpec, Value};
use banto_modbus_read_core::{ReadError, ReadHoldingU16, MAX_RESPONSE_LEN};
use exports::banto::driver::{address, config, read, session};

wit_bindgen::generate!({ path: "../wit", world: "driver" });

struct Component;

struct ModbusConnection {
    unit_id: u8,
    next_transaction_id: Cell<u16>,
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

/// Only FC3 holding-register U16 addresses are supported. Preserve the
/// native parser's 5/6 digit reference and canonical six-digit form.
fn parse_holding_u16(text: &str, data_type: &str, words: Option<u32>) -> Result<u16, String> {
    if data_type != "u16" || words.is_some() {
        return Err("初版は u16 の単一保持レジスタのみ対応します".into());
    }
    let text = text.trim();
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 5 | 6) || bytes[0] != b'4' || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(format!("{text:?} は保持レジスタの参照番号ではありません"));
    }
    let number = text[1..]
        .parse::<u32>()
        .map_err(|_| format!("{text:?} は保持レジスタの参照番号ではありません"))?;
    u16::try_from(number.checked_sub(1).ok_or("レジスタ番号は1からです")?)
        .map_err(|_| "保持レジスタの範囲を超えています".into())
}

fn canonical(offset: u16) -> String {
    format!("4{:05}", u32::from(offset) + 1)
}

fn step(request: ReadHoldingU16) -> session::Step {
    session::Step {
        send: request.request().to_vec(),
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
            let offset =
                parse_holding_u16(&spec.address, &spec.data_type, spec.string_words).ok()?;
            let transaction_id = conn.next_transaction_id.get();
            conn.next_transaction_id.set(transaction_id.wrapping_add(1));
            let request = ReadHoldingU16 {
                transaction_id,
                unit_id: conn.unit_id,
                offset,
            };
            Some(read::Planned {
                step: step(request),
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
    let offset = parse_holding_u16(&spec.address, &spec.data_type, spec.string_words)
        .map_err(FatalError::Protocol)?;
    let send: [u8; 12] = p
        .step
        .send
        .as_slice()
        .try_into()
        .map_err(|_| FatalError::Protocol("FC3 要求長が不正です".into()))?;
    let request = ReadHoldingU16 {
        transaction_id: u16::from_be_bytes([send[0], send[1]]),
        unit_id: conn.unit_id,
        offset,
    };
    if send != request.request() {
        return Err(FatalError::Protocol(
            "計画と要求バイトが一致しません".into(),
        ));
    }
    let value = match request.decode(response) {
        Ok(value) => Ok(Value::Uint(u64::from(value))),
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
        let offset = parse_holding_u16(&text, &data_type, string_words)?;
        Ok(address::Parsed {
            canonical: canonical(offset),
            area: "holding_register".into(),
            writable_area: true,
            bit: None,
            word_count: 1,
            index: Some(u64::from(offset)),
        })
    }

    fn offset(
        canonical: String,
        data_type: String,
        string_words: Option<u32>,
        n: i64,
    ) -> Result<String, String> {
        let offset = parse_holding_u16(&canonical, &data_type, string_words)?;
        let next = i64::from(offset)
            .checked_add(n)
            .ok_or("オフセットが範囲外です")?;
        canonical_for(next)
    }

    fn supported_kinds() -> Vec<String> {
        vec!["u16".into()]
    }

    fn offset_words(canonical: String, n: i64) -> Result<String, String> {
        let offset = parse_holding_u16(&canonical, "u16", None)?;
        let next = i64::from(offset)
            .checked_add(n)
            .ok_or("オフセットが範囲外です")?;
        canonical_for(next)
    }

    fn help() -> String {
        "FC3 の単一保持レジスタ U16: 40001〜49999 または 400001〜465536".into()
    }

    fn probe() -> Option<TagSpec> {
        Some(TagSpec {
            address: "40001".into(),
            data_type: "u16".into(),
            string_words: None,
        })
    }
}

fn canonical_for(offset: i64) -> Result<String, String> {
    let offset = u16::try_from(offset).map_err(|_| "保持レジスタの範囲を超えています")?;
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
        use config::{FieldGroup, FieldKind, IntegerSpec, SettingField, SettingValue};
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
        assert_eq!(parse_holding_u16("40001", "u16", None), Ok(0));
        assert_eq!(parse_holding_u16("465536", "u16", None), Ok(u16::MAX));
        assert_eq!(canonical(0), "400001");
        assert!(parse_holding_u16("30001", "u16", None).is_err());
        assert!(parse_holding_u16("40001.3", "u16", None).is_err());
        assert!(parse_holding_u16("40001", "i16", None).is_err());
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
