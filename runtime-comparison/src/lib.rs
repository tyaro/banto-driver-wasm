//! The integration test loads a built component and reuses the native FC3 fixtures.

#[cfg(test)]
mod tests {
    use banto_fc3_native_comparison::fixtures::{
        native, native_numeric, native_u32, NumericValue, Outcome, FIXTURES, NUMERIC_FIXTURES,
        U32_FIXTURES,
    };
    use serde_json::{json, Value as JsonValue};

    #[test]
    fn real_component_register_bit_read_world() -> wasmtime::Result<()> {
        use banto::driver::types::{Setting, TagSpec, Value};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        for order in ["low_high", "high_low"] {
            let settings = [
                Setting {
                    key: "host".into(),
                    value: "fixture.invalid".into(),
                },
                Setting {
                    key: "port".into(),
                    value: "502".into(),
                },
                Setting {
                    key: "word_order".into(),
                    value: order.into(),
                },
            ];
            assert!(driver
                .banto_driver_config()
                .call_validate(&mut store, &settings)?
                .is_empty());
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .unwrap();
            let specs = [
                TagSpec {
                    address: "40001.0".into(),
                    data_type: "bool".into(),
                    string_words: None,
                },
                TagSpec {
                    address: "30001.15".into(),
                    data_type: "bool".into(),
                    string_words: None,
                },
            ];
            for (index, canonical, function, word) in
                [(0, "400001.0", 3, 0x0001u16), (1, "300001.15", 4, 0x8000)]
            {
                let parsed = driver
                    .banto_driver_address()
                    .call_parse(&mut store, &specs[index].address, "bool", None)?
                    .unwrap();
                assert_eq!(parsed.canonical, canonical);
                assert_eq!(parsed.bit, Some(if index == 0 { 0 } else { 15 }));
                assert_eq!(parsed.word_count, 1);
                let plan = driver
                    .banto_driver_read()
                    .call_plan(&mut store, conn, &specs[index..index + 1])?
                    .unwrap();
                assert_eq!(plan.len(), 1);
                assert_eq!(&plan[0].step.send[7..12], &[function, 0, 0, 0, 1]);
                let tid = &plan[0].step.send[0..2];
                let [hi, lo] = word.to_be_bytes();
                let response = [tid[0], tid[1], 0, 0, 0, 5, 1, function, 2, hi, lo];
                assert!(
                    matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plan[0], &specs[index..index + 1], &response)?, Ok(values) if matches!(values.as_slice(), [Ok(Value::Bool(true))]))
                );
            }
            for invalid in ["40001.16", "40001.F", "00001.3", "40001 .3"] {
                assert!(driver
                    .banto_driver_address()
                    .call_parse(&mut store, invalid, "bool", None)?
                    .is_err());
            }
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    wasmtime::component::bindgen!({ path: "../wit", world: "driver" });

    // Contract checks for the artifact. These do not register a driver in Hub.
    fn manifest_settings_version(manifest: &JsonValue) -> Result<u32, &'static str> {
        let version = manifest
            .get("settingsVersion")
            .and_then(JsonValue::as_u64)
            .ok_or("settingsVersion must be a positive integer")?;
        u32::try_from(version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or("settingsVersion must be a positive integer")
    }

    fn checked_fc3_u16(
        data_type: &str,
        value: &banto::driver::types::Value,
    ) -> Result<u16, &'static str> {
        use banto::driver::types::Value;

        if data_type != "u16" {
            return Err("FC3 v1 requires u16");
        }
        match value {
            Value::Uint(raw) => u16::try_from(*raw).map_err(|_| "uint exceeds u16"),
            _ => Err("FC3 u16 requires uint"),
        }
    }

    fn checked_fc3_u32(
        data_type: &str,
        value: &banto::driver::types::Value,
    ) -> Result<u32, &'static str> {
        use banto::driver::types::Value;

        if data_type != "u32" {
            return Err("FC3 U32 requires u32");
        }
        match value {
            Value::Uint(raw) => u32::try_from(*raw).map_err(|_| "uint exceeds u32"),
            _ => Err("FC3 u32 requires uint"),
        }
    }

    fn checked_numeric(
        kind: &str,
        value: &banto::driver::types::Value,
    ) -> Result<NumericValue, &'static str> {
        use banto::driver::types::Value;
        match (kind, value) {
            ("i16", Value::Int(n)) if i16::try_from(*n).is_ok() => Ok(NumericValue::Int(*n)),
            ("i32", Value::Int(n)) if i32::try_from(*n).is_ok() => Ok(NumericValue::Int(*n)),
            ("i64", Value::Int(n)) => Ok(NumericValue::Int(*n)),
            ("u16", Value::Uint(n)) if u16::try_from(*n).is_ok() => Ok(NumericValue::Uint(*n)),
            ("u32", Value::Uint(n)) if u32::try_from(*n).is_ok() => Ok(NumericValue::Uint(*n)),
            ("u64", Value::Uint(n)) => Ok(NumericValue::Uint(*n)),
            ("f32", Value::Float(n))
                if n.is_nan() || (*n as f32 as f64).to_bits() == n.to_bits() =>
            {
                Ok(NumericValue::Float(*n))
            }
            ("f64", Value::Float(n)) => Ok(NumericValue::Float(*n)),
            _ => Err("numeric WIT variant or range does not match tag type"),
        }
    }

    #[test]
    fn numeric_boundary_rejects_wrong_variant_and_width() {
        use banto::driver::types::Value;
        assert_eq!(
            checked_numeric("i16", &Value::Int(i16::MIN.into())),
            Ok(NumericValue::Int(i16::MIN.into()))
        );
        assert!(checked_numeric("i16", &Value::Int(i64::from(i16::MAX) + 1)).is_err());
        assert!(checked_numeric("i32", &Value::Int(i64::from(i32::MIN) - 1)).is_err());
        assert!(checked_numeric("i32", &Value::Uint(1)).is_err());
        assert!(checked_numeric("u64", &Value::Int(1)).is_err());
        assert!(checked_numeric("f32", &Value::Float(1.0e100)).is_err());
        assert!(checked_numeric("f32", &Value::Float(f64::NAN)).is_ok());
        assert!(checked_numeric("f32", &Value::Float(f64::INFINITY)).is_ok());
        assert!(checked_numeric("f64", &Value::Uint(1)).is_err());
    }

    #[test]
    fn prototype_manifest_requires_positive_settings_version() {
        let valid: JsonValue =
            serde_json::from_str(include_str!("../../component/manifest.json")).unwrap();
        assert_eq!(manifest_settings_version(&valid), Ok(1));
        assert_eq!(valid["version"], "0.4.0");
        assert_eq!(valid["hubApi"], "1");
        assert_eq!(
            valid["displayName"],
            "Modbus TCP FC1-FC4 reads and optional writes (WASM)"
        );
        for invalid in [
            json!({}),
            json!({ "settingsVersion": null }),
            json!({ "settingsVersion": 0 }),
            json!({ "settingsVersion": -1 }),
            json!({ "settingsVersion": 1.0 }),
            json!({ "settingsVersion": "1" }),
            json!({ "settingsVersion": true }),
            json!({ "settingsVersion": 4294967296u64 }),
        ] {
            assert!(manifest_settings_version(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn packaged_manifest_matches_hub_write_negotiation_shape() {
        // Hub Manifest::parse requires this exact ordered capability pair and
        // string writeApi version before it attempts the optional world bind.
        let manifest: JsonValue =
            serde_json::from_str(include_str!("../../component/manifest.json")).unwrap();
        let accepted_write_request = |m: &JsonValue| {
            m.get("capabilities") == Some(&json!(["read", "write"]))
                && m.get("writeApi") == Some(&json!("1"))
        };
        assert!(accepted_write_request(&manifest));
        let mut missing = manifest.clone();
        missing.as_object_mut().unwrap().remove("writeApi");
        assert!(!accepted_write_request(&missing));
        let mut wrong_type = manifest.clone();
        wrong_type["writeApi"] = json!(1);
        assert!(!accepted_write_request(&wrong_type));
        let mut wrong_capabilities = manifest;
        wrong_capabilities["capabilities"] = json!(["write", "read"]);
        assert!(!accepted_write_request(&wrong_capabilities));
    }

    #[test]
    fn fc3_u16_boundary_refuses_other_variants_and_overflow() {
        use banto::driver::types::Value;

        assert_eq!(checked_fc3_u16("u16", &Value::Uint(0)), Ok(0));
        assert_eq!(checked_fc3_u16("u16", &Value::Uint(65535)), Ok(65535));
        assert!(checked_fc3_u16("u16", &Value::Uint(65536)).is_err());
        assert!(checked_fc3_u16("u16", &Value::Int(42)).is_err());
        assert!(checked_fc3_u16("u16", &Value::Float(42.0)).is_err());
        assert!(checked_fc3_u16("i16", &Value::Uint(42)).is_err());
    }

    #[test]
    fn fc3_u32_boundary_refuses_other_variants_and_overflow() {
        use banto::driver::types::Value;

        assert_eq!(checked_fc3_u32("u32", &Value::Uint(0)), Ok(0));
        assert_eq!(
            checked_fc3_u32("u32", &Value::Uint(u32::MAX.into())),
            Ok(u32::MAX)
        );
        assert!(checked_fc3_u32("u32", &Value::Uint(u64::from(u32::MAX) + 1)).is_err());
        assert!(checked_fc3_u32("u32", &Value::Int(42)).is_err());
        assert!(checked_fc3_u32("u16", &Value::Uint(42)).is_err());
    }

    #[test]
    fn real_component_fc4_input_reads_keep_fc3_probe() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec, Value};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        assert!(driver
            .banto_driver_address()
            .call_probe(&mut store)?
            .is_none());
        let parsed = driver
            .banto_driver_address()
            .call_parse(&mut store, "30001", "u16", None)?
            .unwrap();
        assert_eq!(parsed.canonical, "300001");
        assert_eq!(parsed.area, "input_register");
        assert!(!parsed.writable_area);
        let settings = [
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
        ];
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .unwrap();
        let specs = [TagSpec {
            address: "30001".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        let plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .unwrap();
        assert_eq!(plan[0].covers, [0]);
        assert_eq!(plan[0].step.send, [0, 0, 0, 0, 0, 6, 1, 4, 0, 0, 0, 1]);
        assert!(
            matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plan[0], &specs,
            &[0,0,0,0,0,5,1,4,2,0x12,0x34])?, Ok(v) if matches!(v.as_slice(), [Ok(Value::Uint(0x1234))]))
        );
        assert!(
            matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plan[0], &specs,
            &[0,0,0,0,0,3,1,0x84,2])?, Ok(v) if matches!(v.as_slice(), [Err(ItemError::Device(_))]))
        );
        assert!(matches!(
            driver.banto_driver_read().call_decode(
                &mut store,
                conn,
                &plan[0],
                &specs,
                &[0, 0, 0, 0, 0, 5, 1, 3, 2, 0x12, 0x34]
            )?,
            Err(FatalError::Framing(_))
        ));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn real_component_fc4_only_handshake_and_bool_reads() -> wasmtime::Result<()> {
        use banto::driver::types::{Setting, TagSpec, Value};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        let settings = [
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
            Setting {
                key: "probe_address".into(),
                value: "30001".into(),
            },
        ];
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .unwrap();
        let probe = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .unwrap()
            .unwrap();
        assert_eq!(probe.send, [0, 0, 0, 0, 0, 6, 1, 4, 0, 0, 0, 1]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&[0, 0, 0, 0, 0, 5, 1, 4, 2, 0, 9]))?
            .unwrap()
            .is_none());
        for (address, function, tid) in [("00001", 1, 1), ("10001", 2, 2)] {
            let specs = [TagSpec {
                address: address.into(),
                data_type: "bool".into(),
                string_words: None,
            }];
            let plan = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .unwrap();
            assert_eq!(
                plan[0].step.send,
                [0, tid, 0, 0, 0, 6, 1, function, 0, 0, 0, 1]
            );
            let response = [0, tid, 0, 0, 0, 4, 1, function, 1, 1];
            assert!(
                matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plan[0], &specs, &response)?, Ok(v) if matches!(v.as_slice(), [Ok(Value::Bool(true))]))
            );
        }
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn real_component_matches_native_fc3_fixtures() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec, Value};

        let path = std::env::var("BANTO_COMPONENT_WASM")
            .expect("set BANTO_COMPONENT_WASM to a built component");
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;

        assert!(driver
            .banto_driver_address()
            .call_probe(&mut store)?
            .is_none());
        let probe = TagSpec {
            address: "40001".into(),
            data_type: "u16".into(),
            string_words: None,
        };
        let parsed = driver
            .banto_driver_address()
            .call_parse(
                &mut store,
                &probe.address,
                &probe.data_type,
                probe.string_words,
            )?
            .expect("probe address parses");
        assert_eq!(parsed.canonical, "400001");
        assert_eq!(parsed.index, Some(0));
        assert_eq!(
            driver
                .banto_driver_address()
                .call_supported_kinds(&mut store)?,
            ["bool", "u16", "u32", "i16", "i32", "i64", "u64", "f32", "f64"]
        );

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
                key: "unit_id".into(),
                value: "1".into(),
            },
        ];
        assert!(driver
            .banto_driver_config()
            .call_validate(&mut store, &settings)?
            .is_empty());
        assert!(driver
            .banto_driver_config()
            .call_secret_keys(&mut store)?
            .is_empty());
        let schema = driver.banto_driver_config().call_schema(&mut store)?;
        assert_eq!(schema.len(), 7);
        assert!(matches!(
            schema.iter().find(|field| field.key == "word_order")
                .and_then(|field| field.default.as_ref()),
            Some(exports::banto::driver::config::SettingValue::Text(value)) if value == "low_high"
        ));
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("valid connection settings");
        let specs = [TagSpec {
            address: parsed.canonical,
            data_type: "u16".into(),
            string_words: None,
        }];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("FC3 plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[0].step.send, [0, 0, 0, 0, 0, 6, 1, 3, 0, 0, 0, 1]);
        assert_eq!(plans[0].step.max_len, 260);

        for fixture in FIXTURES {
            let expected_native = native(fixture.response);
            assert_eq!(
                expected_native, fixture.expected,
                "native: {}",
                fixture.name
            );
            let decoded = driver.banto_driver_read().call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                fixture.response,
            )?;
            let wasm = match decoded {
                Ok(values) => match values.as_slice() {
                    [Ok(Value::Uint(value))] => Outcome::Value(
                        checked_fc3_u16(&specs[0].data_type, &Value::Uint(*value))
                            .expect("FC3 U16 boundary"),
                    ),
                    [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                    other => panic!("unexpected WASM values for {}: {other:?}", fixture.name),
                },
                Err(FatalError::Framing(_)) => Outcome::Framing,
                Err(FatalError::Protocol(_)) => Outcome::Protocol,
                other => panic!("unexpected WASM error for {}: {other:?}", fixture.name),
            };
            if fixture.native_accepts_extra {
                assert_eq!(
                    wasm,
                    Outcome::Framing,
                    "intentional strictness: {}",
                    fixture.name
                );
                assert_ne!(wasm, expected_native);
            } else {
                assert_eq!(wasm, expected_native, "WASM/native: {}", fixture.name);
            }
        }
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn real_component_matches_native_numeric_fixtures() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        for (kind, last, after, words) in [
            ("i16", "465536", None, 1),
            ("i32", "465535", Some("465536"), 2),
            ("f32", "465535", Some("465536"), 2),
            ("i64", "465533", Some("465534"), 4),
            ("u64", "465533", Some("465534"), 4),
            ("f64", "465533", Some("465534"), 4),
        ] {
            let parsed = driver
                .banto_driver_address()
                .call_parse(&mut store, last, kind, None)?
                .expect("last whole tag");
            assert_eq!(parsed.word_count, words);
            if let Some(after) = after {
                assert!(driver
                    .banto_driver_address()
                    .call_parse(&mut store, after, kind, None)?
                    .is_err());
            }
            assert_eq!(
                driver
                    .banto_driver_address()
                    .call_offset(&mut store, "400001", kind, None, 1)?
                    .unwrap(),
                format!("4{:05}", 1 + words)
            );
        }
        for fixture in NUMERIC_FIXTURES {
            let mut settings = vec![
                Setting {
                    key: "host".into(),
                    value: "example.test".into(),
                },
                Setting {
                    key: "port".into(),
                    value: "502".into(),
                },
            ];
            if let Some(order) = fixture.word_order {
                settings.push(Setting {
                    key: "word_order".into(),
                    value: order.into(),
                });
            }
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("settings");
            let specs = [TagSpec {
                address: "400001".into(),
                data_type: fixture.kind.into(),
                string_words: None,
            }];
            let plans = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .expect("numeric plan");
            assert_eq!(plans.len(), 1, "{}", fixture.name);
            assert_eq!(plans[0].covers, [0], "{}", fixture.name);
            let words: u16 = match fixture.kind {
                "i16" | "u16" => 1,
                "i32" | "u32" | "f32" => 2,
                _ => 4,
            };
            assert_eq!(
                plans[0].step.send[10..12],
                words.to_be_bytes(),
                "{}",
                fixture.name
            );
            let native = native_numeric(fixture);
            assert_eq!(native, fixture.expected, "native: {}", fixture.name);
            let decoded = driver.banto_driver_read().call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                fixture.response,
            )?;
            let wasm = match decoded {
                Ok(values) => match values.as_slice() {
                    [Ok(value)] => Outcome::Value(
                        checked_numeric(fixture.kind, value).expect("typed numeric result"),
                    ),
                    [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                    other => panic!(
                        "unexpected numeric response for {}: {other:?}",
                        fixture.name
                    ),
                },
                Err(FatalError::Framing(_)) => Outcome::Framing,
                Err(FatalError::Protocol(_)) => Outcome::Protocol,
                other => panic!("unexpected numeric error for {}: {other:?}", fixture.name),
            };
            assert_eq!(wasm, native, "WASM/native: {}", fixture.name);
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    #[test]
    fn real_component_keeps_u64_raw_bits_and_nonfinite_float_per_tag() -> wasmtime::Result<()> {
        use banto::driver::types::{Setting, TagSpec, Value};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        let settings = [
            Setting {
                key: "host".into(),
                value: "example.test".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
        ];
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("settings");
        let specs = [
            TagSpec {
                address: "400001".into(),
                data_type: "u64".into(),
                string_words: None,
            },
            TagSpec {
                address: "400005".into(),
                data_type: "f32".into(),
                string_words: None,
            },
        ];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[1].covers, [1]);
        assert_eq!(plans[0].step.send[10..12], [0, 4]);
        assert_eq!(plans[1].step.send[10..12], [0, 2]);
        let precise = [0, 0, 0, 0, 0, 11, 1, 3, 8, 0, 1, 0, 0, 0, 0, 0, 0x20];
        let raw = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[0], &specs, &precise)?
            .expect("raw U64");
        assert!(matches!(
            raw.as_slice(),
            [Ok(Value::Uint(9_007_199_254_740_993))]
        ));
        let nan = [0, 1, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0x7f, 0xc0];
        let raw = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[1], &specs, &nan)?
            .expect("raw F32 NaN");
        assert!(matches!(raw.as_slice(), [Ok(Value::Float(value))] if value.is_nan()));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn real_component_matches_native_fc3_u32_fixtures() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec, Value};

        let path = std::env::var("BANTO_COMPONENT_WASM")
            .expect("set BANTO_COMPONENT_WASM to a built component");
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;

        assert_eq!(
            driver
                .banto_driver_address()
                .call_supported_kinds(&mut store)?,
            ["bool", "u16", "u32", "i16", "i32", "i64", "u64", "f32", "f64"]
        );
        let first = driver
            .banto_driver_address()
            .call_parse(&mut store, "40001", "u32", None)?
            .expect("first U32 start");
        assert_eq!(first.canonical, "400001");
        assert_eq!(first.index, Some(0));
        assert_eq!(first.word_count, 2);
        let parsed = driver
            .banto_driver_address()
            .call_parse(&mut store, "465535", "u32", None)?
            .expect("last U32 start");
        assert_eq!(parsed.canonical, "465535");
        assert_eq!(parsed.index, Some(65_534));
        assert_eq!(parsed.word_count, 2);
        assert!(driver
            .banto_driver_address()
            .call_parse(&mut store, "465536", "u32", None)?
            .is_err());
        assert_eq!(
            driver
                .banto_driver_address()
                .call_parse(&mut store, "465536", "u16", None)?
                .unwrap()
                .word_count,
            1
        );
        for (start, n, expected) in [
            ("400001", 1, "400003"),
            ("400003", -1, "400001"),
            ("465533", 1, "465535"),
        ] {
            assert_eq!(
                driver
                    .banto_driver_address()
                    .call_offset(&mut store, start, "u32", None, n)?
                    .unwrap(),
                expected
            );
        }
        for (start, n) in [
            ("400002", -1),
            ("465534", 1),
            ("465535", 1),
            ("400001", i64::MAX),
            ("400001", i64::MIN),
        ] {
            assert!(driver
                .banto_driver_address()
                .call_offset(&mut store, start, "u32", None, n)?
                .is_err());
        }
        assert_eq!(
            driver
                .banto_driver_address()
                .call_offset(&mut store, "400001", "u16", None, 1)?
                .unwrap(),
            "400002"
        );
        assert_eq!(
            driver
                .banto_driver_address()
                .call_offset_words(&mut store, "465535", 1)?
                .unwrap(),
            "465536"
        );

        let base_settings = [
            Setting {
                key: "host".into(),
                value: "example.test".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
        ];
        for fixture in U32_FIXTURES {
            let mut settings = base_settings.to_vec();
            if let Some(order) = fixture.word_order {
                settings.push(Setting {
                    key: "word_order".into(),
                    value: order.into(),
                });
            }
            assert!(driver
                .banto_driver_config()
                .call_validate(&mut store, &settings)?
                .is_empty());
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("valid settings");
            let specs = [TagSpec {
                address: "400001".into(),
                data_type: "u32".into(),
                string_words: None,
            }];
            let plans = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .expect("U32 plan");
            assert_eq!(plans.len(), 1, "{}", fixture.name);
            assert_eq!(plans[0].covers, [0], "{}", fixture.name);
            assert_eq!(
                plans[0].step.send,
                [0, 0, 0, 0, 0, 6, 1, 3, 0, 0, 0, 2],
                "{}",
                fixture.name
            );
            assert_eq!(plans[0].step.max_len, 260);

            let expected_native = native_u32(fixture.response, fixture.word_order, "40001", 0);
            assert_eq!(
                expected_native, fixture.expected,
                "native: {}",
                fixture.name
            );
            let decoded = driver.banto_driver_read().call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                fixture.response,
            )?;
            let wasm = match decoded {
                Ok(values) => match values.as_slice() {
                    [Ok(value @ Value::Uint(_))] => Outcome::Value(
                        checked_fc3_u32(&specs[0].data_type, value).expect("FC3 U32 boundary"),
                    ),
                    [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                    other => panic!("unexpected WASM U32 values for {}: {other:?}", fixture.name),
                },
                Err(FatalError::Framing(_)) => Outcome::Framing,
                Err(FatalError::Protocol(_)) => Outcome::Protocol,
                other => panic!("unexpected WASM U32 error for {}: {other:?}", fixture.name),
            };
            if fixture.native_accepts_extra {
                assert_eq!(
                    wasm,
                    Outcome::Framing,
                    "intentional strictness: {}",
                    fixture.name
                );
                assert_ne!(wasm, expected_native);
            } else {
                assert_eq!(wasm, expected_native, "WASM/native: {}", fixture.name);
            }
            conn.resource_drop(&mut store)?;
        }

        let settings = base_settings.to_vec();
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("valid settings");
        let last = [TagSpec {
            address: "465535".into(),
            data_type: "u32".into(),
            string_words: None,
        }];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &last)?
            .expect("last U32 plan");
        assert_eq!(plans[0].step.send, [0, 0, 0, 0, 0, 6, 1, 3, 255, 254, 0, 2]);
        let response = [0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd];
        assert!(
            matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plans[0], &last, &response)?, Ok(values) if matches!(values.as_slice(), [Ok(Value::Uint(0xabcd_1234))]))
        );
        let beyond = [TagSpec {
            address: "465536".into(),
            data_type: "u32".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &beyond)?
            .unwrap()
            .is_empty());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn local_tcp_host_exchanges_probe_frames_with_component() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec, Value};
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::time::Duration;

        let path = std::env::var("BANTO_COMPONENT_WASM")
            .expect("set BANTO_COMPONENT_WASM to a built component");
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);

        for (case, expected) in [
            ("value", Outcome::Value(42)),
            ("exception", Outcome::Device(2)),
            ("framing", Outcome::Framing),
            ("wrong-id", Outcome::Protocol),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            let server = std::thread::spawn(move || -> std::io::Result<()> {
                let (mut stream, _) = listener.accept()?;
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut request = [0; 12];
                stream.read_exact(&mut request)?;
                assert_eq!(&request[2..], &[0, 0, 0, 6, 1, 3, 0, 0, 0, 1]);
                let mut response = request[..2].to_vec();
                match case {
                    "value" => response.extend_from_slice(&[0, 0, 0, 5, 1, 3, 2, 0, 42]),
                    "exception" => response.extend_from_slice(&[0, 0, 0, 3, 1, 0x83, 2]),
                    "framing" => response.extend_from_slice(&[0, 0, 0, 5, 1, 3, 3, 0, 42]),
                    "wrong-id" => {
                        response[1] ^= 1;
                        response.extend_from_slice(&[0, 0, 0, 5, 1, 3, 2, 0, 42]);
                    }
                    _ => unreachable!(),
                }
                stream.write_all(&response)
            });

            let settings = [
                Setting {
                    key: "host".into(),
                    value: "127.0.0.1".into(),
                },
                Setting {
                    key: "port".into(),
                    value: port.to_string(),
                },
            ];
            let mut store = wasmtime::Store::new(&engine, ());
            let driver = Driver::instantiate(&mut store, &component, &linker)?;
            assert!(driver
                .banto_driver_config()
                .call_validate(&mut store, &settings)?
                .is_empty());
            let specs = [TagSpec {
                address: "40001".into(),
                data_type: "u16".into(),
                string_words: None,
            }];
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("valid connection settings");
            let plans = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .expect("FC3 plan");
            let [plan] = plans.as_slice() else {
                panic!("FC3 probe must have one plan");
            };
            assert_eq!(plan.covers, [0]);

            // This is a controlled test host, not Hub's FrameSession. Keep
            // transport I/O on this side of the component boundary.
            let mut stream = TcpStream::connect(("127.0.0.1", port))?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.write_all(&plan.step.send)?;
            let mut header = [0; 6];
            stream.read_exact(&mut header)?;
            let frame_len = 6 + usize::from(u16::from_be_bytes([header[4], header[5]]));
            assert!(frame_len <= plan.step.max_len as usize);
            let mut response = header.to_vec();
            response.resize(frame_len, 0);
            stream.read_exact(&mut response[6..])?;
            server.join().expect("local server thread")?;
            assert_eq!(native(&response), expected, "native {case}");

            let decoded = driver
                .banto_driver_read()
                .call_decode(&mut store, conn, plan, &specs, &response)?;
            let actual = match decoded {
                Ok(values) => match values.as_slice() {
                    [Ok(value @ Value::Uint(_))] => {
                        Outcome::Value(checked_fc3_u16(&specs[0].data_type, value).unwrap())
                    }
                    [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                    other => panic!("unexpected probe values for {case}: {other:?}"),
                },
                Err(FatalError::Framing(_)) => Outcome::Framing,
                Err(FatalError::Protocol(_)) => Outcome::Protocol,
                other => panic!("unexpected probe error for {case}: {other:?}"),
            };
            assert_eq!(actual, expected, "{case}");
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    #[test]
    fn wasmtime_memory_grow_failure_is_not_assumed_to_trap() -> wasmtime::Result<()> {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
                (memory 1 1)
                (func (export "grow") (result i32)
                    i32.const 1
                    memory.grow))"#,
        )?;
        let mut store = wasmtime::Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &module, &[])?;
        let grow = instance.get_typed_func::<(), i32>(&mut store, "grow")?;
        assert_eq!(grow.call(&mut store, ())?, -1);
        Ok(())
    }

    #[test]
    fn wasmtime_trap_and_epoch_have_typed_causes() -> wasmtime::Result<()> {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        let engine = wasmtime::Engine::new(&config)?;
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
                (func (export "trap") unreachable)
                (func (export "loop") (loop br 0)))"#,
        )?;
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_epoch_deadline(1_000_000);
        let instance = wasmtime::Instance::new(&mut store, &module, &[])?;
        let trap = instance.get_typed_func::<(), ()>(&mut store, "trap")?;
        let error = trap
            .call(&mut store, ())
            .expect_err("unreachable must trap");
        assert!(matches!(
            error.downcast_ref::<wasmtime::Trap>(),
            Some(wasmtime::Trap::UnreachableCodeReached)
        ));

        store.set_epoch_deadline(1);
        engine.increment_epoch();
        let loop_forever = instance.get_typed_func::<(), ()>(&mut store, "loop")?;
        let error = loop_forever
            .call(&mut store, ())
            .expect_err("epoch must interrupt");
        assert!(matches!(
            error.downcast_ref::<wasmtime::Trap>(),
            Some(wasmtime::Trap::Interrupt)
        ));
        Ok(())
    }
}

#[cfg(test)]
mod write_runtime {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-v1" });

    #[test]
    fn real_component_register_bit_write_world_rejects_before_send() -> wasmtime::Result<()> {
        use banto::driver::types::{ItemError, Setting, TagSpec, Value};
        use exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        let settings = [
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
            Setting {
                key: "write_mode".into(),
                value: "enabled".into(),
            },
            Setting {
                key: "write_allowlist".into(),
                value: "400001-400010".into(),
            },
        ];
        assert!(driver
            .banto_driver_config()
            .call_validate(&mut store, &settings)?
            .is_empty());
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .unwrap();
        let parsed = driver
            .banto_driver_address()
            .call_parse(&mut store, "40001.3", "bool", None)?
            .unwrap();
        assert_eq!(parsed.canonical, "400001.3");
        assert_eq!(parsed.bit, Some(3));
        assert_eq!(parsed.word_count, 1);
        assert!(driver
            .banto_driver_address()
            .call_parse(&mut store, "40001.16", "bool", None)?
            .is_err());
        assert!(driver
            .banto_driver_address()
            .call_parse(&mut store, "40001 .3", "bool", None)?
            .is_err());
        let read_spec = [TagSpec {
            address: "40001.3".into(),
            data_type: "bool".into(),
            string_words: None,
        }];
        let plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &read_spec)?
            .unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(&plan[0].step.send[7..12], &[3, 0, 0, 0, 1]);
        let tid = &plan[0].step.send[0..2];
        let response = [tid[0], tid[1], 0, 0, 0, 5, 1, 3, 2, 0, 8];
        assert!(
            matches!(driver.banto_driver_read().call_decode(&mut store, conn, &plan[0], &read_spec, &response)?, Ok(values) if matches!(values.as_slice(), [Ok(Value::Bool(true))]))
        );
        let input_spec = [TagSpec {
            address: "30001.15".into(),
            data_type: "bool".into(),
            string_words: None,
        }];
        let input_plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &input_spec)?
            .unwrap();
        assert_eq!(input_plan.len(), 1);
        assert_eq!(&input_plan[0].step.send[7..12], &[4, 0, 0, 0, 1]);
        let tid = &input_plan[0].step.send[0..2];
        let response = [tid[0], tid[1], 0, 0, 0, 5, 1, 4, 2, 0x80, 0];
        assert!(
            matches!(driver.banto_driver_read().call_decode(&mut store, conn, &input_plan[0], &input_spec, &response)?, Ok(values) if matches!(values.as_slice(), [Ok(Value::Bool(true))]))
        );
        let bit = WriteItem {
            spec: TagSpec {
                address: "40001.3".into(),
                data_type: "bool".into(),
                string_words: None,
            },
            value: Value::Bool(true),
        };
        let word = WriteItem {
            spec: TagSpec {
                address: "40002".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Uint(7),
        };
        for items in [
            vec![bit.clone()],
            vec![word.clone(), bit.clone()],
            vec![bit.clone(), word.clone()],
        ] {
            let progress = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .unwrap();
            assert!(
                matches!(progress, WriteProgress::Done(results) if results.len() == items.len() && results.iter().all(|result| matches!(result, Err(ItemError::Unsupported(_)))))
            );
        }
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn real_component_write_world_preflights_and_checks_ack() -> wasmtime::Result<()> {
        use banto::driver::types::{Setting, TagSpec, Value};
        use exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_COMPONENT_WASM")?;
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        let settings = [
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "502".into(),
            },
            Setting {
                key: "write_mode".into(),
                value: "enabled".into(),
            },
            Setting {
                key: "write_allowlist".into(),
                value: "000001-000016,400001-400010".into(),
            },
        ];
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .unwrap();
        let items = [WriteItem {
            spec: TagSpec {
                address: "40001".into(),
                data_type: "u32".into(),
                string_words: None,
            },
            value: Value::Uint(0x1234_abcd),
        }];
        let invalid = [WriteItem {
            spec: TagSpec {
                address: "30001".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Uint(7),
        }];
        assert!(matches!(
            driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &invalid, None)?,
            Ok(WriteProgress::Done(_))
        ));
        let send = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, None)?
            .unwrap();
        let WriteProgress::Send(plan) = send else {
            panic!("expected FC16 send")
        };
        assert_eq!(
            plan.step.send,
            [0, 0, 0, 0, 0, 11, 1, 16, 0, 0, 0, 2, 4, 0xab, 0xcd, 0x12, 0x34]
        );
        assert_eq!(plan.covers, [0]);
        assert!(matches!(
            driver.banto_driver_write_v1().call_write_next(
                &mut store,
                conn,
                &items,
                Some(&[0, 0, 0, 0, 0, 6, 1, 16, 0, 0, 0, 2])
            )?,
            Ok(WriteProgress::Done(_))
        ));
        let coil = [WriteItem {
            spec: TagSpec {
                address: "00001".into(),
                data_type: "bool".into(),
                string_words: None,
            },
            value: Value::Bool(true),
        }];
        let send = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &coil, None)?
            .unwrap();
        let WriteProgress::Send(plan) = send else {
            panic!("expected FC5 send")
        };
        assert_eq!(plan.step.send, [0, 1, 0, 0, 0, 6, 1, 5, 0, 0, 255, 0]);
        assert!(driver
            .banto_driver_write_v1()
            .call_write_next(
                &mut store,
                conn,
                &coil,
                Some(&[0, 1, 0, 0, 0, 6, 1, 5, 0, 0, 0, 0])
            )?
            .is_err());
        assert!(driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, None)?
            .is_err());
        conn.resource_drop(&mut store)?;
        Ok(())
    }
}
