//! The integration test loads a built component and reuses the native FC3 fixtures.

#[cfg(test)]
mod tests {
    use banto_fc3_native_comparison::fixtures::{native, Outcome, FIXTURES};

    wasmtime::component::bindgen!({ path: "../wit", world: "driver" });

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
            ["u16"]
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
        assert_eq!(
            driver.banto_driver_config().call_schema(&mut store)?.len(),
            3
        );
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
                    [Ok(Value::Uint(value))] => {
                        assert!(*value <= u16::MAX.into(), "U16 boundary: {}", fixture.name);
                        Outcome::Value(*value as u16)
                    }
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
