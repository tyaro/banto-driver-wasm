//! The integration test loads a built component and reuses the native FC3 fixtures.

#[cfg(test)]
mod tests {
    use banto_fc3_native_comparison::fixtures::{
        native, native_u32, Outcome, FIXTURES, U32_FIXTURES,
    };
    use serde_json::{json, Value as JsonValue};

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

    #[test]
    fn prototype_manifest_requires_positive_settings_version() {
        let valid: JsonValue =
            serde_json::from_str(include_str!("../../component/manifest.json")).unwrap();
        assert_eq!(manifest_settings_version(&valid), Ok(1));
        assert_eq!(valid["version"], "0.2.0");
        assert_eq!(valid["hubApi"], "1");
        assert_eq!(valid["displayName"], "Modbus TCP FC3 U16/U32 (WASM)");
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
    fn real_component_matches_native_fc3_fixtures() -> wasmtime::Result<()> {
        use banto::driver::types::{FatalError, ItemError, Setting, TagSpec, Value};

        let path = std::env::var("BANTO_COMPONENT_WASM")
            .expect("set BANTO_COMPONENT_WASM to a built component");
        let engine = wasmtime::Engine::default();
        let component = wasmtime::component::Component::from_file(&engine, path)?;
        let linker = wasmtime::component::Linker::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;

        let probe = driver
            .banto_driver_address()
            .call_probe(&mut store)?
            .expect("Modbus probe");
        assert_eq!(probe.address, "40001");
        assert_eq!(probe.data_type, "u16");
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
            ["u16", "u32"]
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
        assert_eq!(schema.len(), 4);
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
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("handshake")
            .is_none());

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
            ["u16", "u32"]
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
            let probe = driver
                .banto_driver_address()
                .call_probe(&mut store)?
                .expect("FC3 probe");
            let parsed = driver
                .banto_driver_address()
                .call_parse(
                    &mut store,
                    &probe.address,
                    &probe.data_type,
                    probe.string_words,
                )?
                .expect("probe address parses");
            let specs = [TagSpec {
                address: parsed.canonical,
                data_type: probe.data_type,
                string_words: probe.string_words,
            }];
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("valid connection settings");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("handshake")
                .is_none());
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
