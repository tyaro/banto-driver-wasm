//! Wasmtime fixtures for the separately built SLMP component.

#[cfg(test)]
mod tests {
    use wasmtime::component::{Component, Linker};
    use wasmtime::{Engine, Store};

    wasmtime::component::bindgen!({ path: "../wit", world: "driver" });

    fn response(serial: u16, body: &[u8]) -> Vec<u8> {
        let mut frame = vec![0xd4, 0];
        frame.extend_from_slice(&serial.to_le_bytes());
        frame.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
        frame.extend_from_slice(&(u16::try_from(body.len() + 2).unwrap()).to_le_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(body);
        frame
    }

    fn model_response(name: &str, code: u16) -> Vec<u8> {
        let mut data = [b' '; 18];
        data[0..name.len()].copy_from_slice(name.as_bytes());
        data[16..18].copy_from_slice(&code.to_le_bytes());
        response(1, &data)
    }

    fn load() -> wasmtime::Result<(Store<()>, Driver)> {
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")
            .expect("set BANTO_SLMP_COMPONENT_WASM to a built component");
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let driver = Driver::instantiate(&mut store, &component, &linker)?;
        Ok((store, driver))
    }

    fn settings() -> Vec<banto::driver::types::Setting> {
        vec![
            banto::driver::types::Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            banto::driver::types::Setting {
                key: "port".into(),
                value: "5000".into(),
            },
        ]
    }

    #[test]
    fn q_0101_sd302_then_d99_read_runs_in_real_component() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
        let (mut store, driver) = load()?;
        let parsed = driver
            .banto_driver_address()
            .call_parse(&mut store, "D99", "u16", None)?
            .expect("static syntax");
        assert_eq!(
            (parsed.canonical.as_str(), parsed.index, parsed.word_count),
            ("D99", Some(99), 1)
        );
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        let type_step = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("handshake")
            .expect("0101");
        assert_eq!(&type_step.send[15..19], &[1, 1, 0, 0]);
        assert_eq!(&type_step.send[2..4], &[1, 0]);
        assert_eq!(type_step.send.len(), 19);
        let sd_step = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
            .expect("type decode")
            .expect("SD302");
        assert_eq!(&sd_step.send[2..4], &[2, 0]);
        assert_eq!(&sd_step.send[17..25], &[0, 0, 0x2e, 1, 0, 0xa9, 1, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &[100, 0])))?
            .expect("SD decode")
            .is_none());

        let specs = [TagSpec {
            address: "D99".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("known D range");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(&plans[0].step.send[2..4], &[3, 0]);
        assert_eq!(&plans[0].step.send[17..25], &[0, 0, 99, 0, 0, 0xa8, 1, 0]);
        let decoded = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x34, 0x12]),
            )?
            .expect("D response");
        assert!(matches!(decoded.as_slice(), [Ok(Value::Uint(0x1234))]));
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    conn,
                    &plans[0],
                    &specs,
                    &response(3, &[0x34, 0x12])
                )?
                .is_err(),
            "a decoded plan cannot be replayed on the resource"
        );
        let outside = [TagSpec {
            address: "D100".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &outside)?
            .is_err());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn r_0101_sd280_pair_then_d_read_and_new_resource_reset() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
        let (mut store, driver) = load()?;
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &[])?
            .is_err());
        let first = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("start")
            .expect("0101");
        assert_eq!(&first.send[2..4], &[1, 0]);
        let sd = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("type decode")
            .expect("SD280 pair");
        assert_eq!(&sd.send[17..27], &[2, 0, 0x18, 1, 0, 0, 0xa9, 0, 2, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &[0x20, 0, 0, 0])))?
            .expect("SD decode")
            .is_none());
        let specs = [TagSpec {
            address: "D31".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("known D range");
        assert_eq!(
            &plans[0].step.send[17..27],
            &[2, 0, 31, 0, 0, 0, 0xa8, 0, 1, 0]
        );
        let decoded = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0xcd, 0xab]),
            )?
            .expect("D read");
        assert!(matches!(decoded.as_slice(), [Ok(Value::Uint(0xabcd))]));
        conn.resource_drop(&mut store)?;

        let fresh = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("fresh connection");
        let fresh_step = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, None)?
            .expect("fresh start")
            .expect("0101");
        assert_eq!(&fresh_step.send[2..4], &[1, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, Some(&model_response("R04CPU", 0x4800)))?
            .expect("fresh type")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, Some(&response(2, &[0x20, 0, 0, 0])))?
            .expect("fresh SD")
            .is_none());
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    fresh,
                    &plans[0],
                    &specs,
                    &response(3, &[0xcd, 0xab])
                )?
                .is_err(),
            "a prior connection's plan is not pending on the new resource"
        );
        fresh.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn unknown_0101_pair_does_not_plan_an_sd_read() -> wasmtime::Result<()> {
        let (mut store, driver) = load()?;
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("start")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("RCPU", 0x0360)))?
            .expect("valid but unknown model")
            .is_none());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn rejected_sd_handshake_never_creates_verified_range() -> wasmtime::Result<()> {
        let (mut store, driver) = load()?;
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("start")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
            .expect("type")
            .is_some());
        let mut refusal = response(2, &[]);
        refusal[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&refusal))?
            .is_err());
        conn.resource_drop(&mut store)?;
        Ok(())
    }
}
