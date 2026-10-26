//! Wasmtime fixtures for the separately built SLMP component.

#[cfg(test)]
mod tests {
    use wasmtime::component::{Component, Linker};
    use wasmtime::{Engine, Store};

    wasmtime::component::bindgen!({ path: "../wit", world: "driver" });

    mod write_host {
        wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-write-v1" });
    }

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

    #[test]
    fn old_and_extended_worlds_negotiate_without_changing_old_exports() -> wasmtime::Result<()> {
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let new_component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let new_read = Driver::instantiate(&mut store, &new_component, &linker)?;
        assert!(
            new_read
                .banto_driver_address()
                .call_parse(&mut store, "D31", "u16", None)?
                .expect("D/u16 syntax")
                .writable_area
        );
        write_host::DriverWithWriteV1::instantiate(&mut store, &new_component, &linker)?;
        if let Ok(old_path) = std::env::var("BANTO_SLMP_LEGACY_COMPONENT_WASM") {
            let old_component = Component::from_file(&engine, old_path)?;
            let old_read = Driver::instantiate(&mut store, &old_component, &linker)?;
            assert!(
                !old_read
                    .banto_driver_address()
                    .call_parse(&mut store, "D31", "u16", None)?
                    .expect("old guest syntax")
                    .writable_area
            );
            assert!(write_host::DriverWithWriteV1::instantiate(
                &mut store,
                &old_component,
                &linker
            )
            .is_err());
        }
        Ok(())
    }

    #[test]
    fn write_extension_borrows_the_same_discovered_resource() -> wasmtime::Result<()> {
        use write_host::banto::driver::types::{ItemError, Setting, TagSpec, Value};
        use write_host::exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let driver = write_host::DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        assert!(
            driver
                .banto_driver_address()
                .call_parse(&mut store, "D31", "u16", None)?
                .expect("D/u16 syntax")
                .writable_area
        );
        assert!(
            !driver
                .banto_driver_address()
                .call_parse(&mut store, "D31", "u32", None)?
                .expect("read-only D/u32 syntax")
                .writable_area
        );
        assert!(driver
            .banto_driver_address()
            .call_parse(&mut store, "D31", "u16", Some(1))?
            .is_err());
        let settings = vec![
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "5000".into(),
            },
            Setting {
                key: "d_u16_write_mode".into(),
                value: "enabled".into(),
            },
            Setting {
                key: "d_u16_write_allowlist".into(),
                value: "D31-D31".into(),
            },
        ];
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("settings");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101")
            .expect("type send");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("SD280")
            .expect("SD send");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[32, 0, 0, 0, 32, 0, 0, 0])),
            )?
            .expect("ready");
        let item = WriteItem {
            spec: TagSpec {
                address: "D31".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Float(0x1234 as f64),
        };
        let progress = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, std::slice::from_ref(&item), None)?
            .expect("preflight");
        let WriteProgress::Send(plan) = progress else {
            panic!("valid write must send")
        };
        assert_eq!(plan.covers, [0]);
        assert_eq!(&plan.step.send[2..4], &[3, 0]);
        assert_eq!(
            &plan.step.send[15..],
            &[1, 0x14, 2, 0, 31, 0, 0, 0, 0xa8, 0, 1, 0, 0x34, 0x12]
        );
        let done = driver
            .banto_driver_write_v1()
            .call_write_next(
                &mut store,
                conn,
                std::slice::from_ref(&item),
                Some(&response(3, &[])),
            )?
            .expect("ACK");
        assert!(matches!(done, WriteProgress::Done(items) if matches!(items.as_slice(), [Ok(())])));
        for invalid in [
            WriteItem {
                spec: TagSpec {
                    address: "M31".into(),
                    data_type: "bit".into(),
                    ..item.spec.clone()
                },
                ..item.clone()
            },
            WriteItem {
                spec: TagSpec {
                    address: "D32".into(),
                    ..item.spec.clone()
                },
                ..item.clone()
            },
            WriteItem {
                spec: TagSpec {
                    data_type: "u32".into(),
                    ..item.spec.clone()
                },
                ..item.clone()
            },
            WriteItem {
                value: Value::Float(f64::NAN),
                ..item.clone()
            },
            WriteItem {
                value: Value::Float(-1.0),
                ..item.clone()
            },
            WriteItem {
                value: Value::Float(0.5),
                ..item.clone()
            },
            WriteItem {
                value: Value::Float(65536.0),
                ..item.clone()
            },
            WriteItem {
                value: Value::Uint(1),
                ..item.clone()
            },
        ] {
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &[invalid], None)?
                .expect("preflight rejection");
            assert!(
                matches!(outcome, WriteProgress::Done(items) if matches!(items.as_slice(), [Err(_)]))
            );
        }
        let duplicate = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &[item.clone(), item.clone()], None)?
            .expect("duplicate preflight");
        assert!(
            matches!(duplicate, WriteProgress::Done(results)
            if results.len() == 2 && results.iter().all(Result::is_err)),
            "duplicate addresses reject the entire request before send"
        );
        let next = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, std::slice::from_ref(&item), None)?
            .expect("serial unchanged by rejection");
        let WriteProgress::Send(next) = next else {
            panic!("valid write must send")
        };
        assert_eq!(&next.step.send[2..4], &[4, 0]);
        let mut refused = response(4, &[]);
        refused[13..15].copy_from_slice(&0xc051u16.to_le_bytes());
        let rejected = driver
            .banto_driver_write_v1()
            .call_write_next(
                &mut store,
                conn,
                std::slice::from_ref(&item),
                Some(&refused),
            )?
            .expect("device refusal");
        assert!(
            matches!(rejected, WriteProgress::Done(items) if matches!(items.as_slice(), [Err(_)]))
        );
        let send = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, std::slice::from_ref(&item), None)?
            .expect("next write after refusal");
        assert!(matches!(send, WriteProgress::Send(_)));
        assert!(
            driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, std::slice::from_ref(&item), None)?
                .is_err(),
            "pending write cannot be resent"
        );
        let bad_ack = response(99, &[]);
        assert!(
            driver
                .banto_driver_write_v1()
                .call_write_next(
                    &mut store,
                    conn,
                    std::slice::from_ref(&item),
                    Some(&bad_ack)
                )?
                .is_err(),
            "wrong serial is fatal"
        );
        assert!(
            driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &[item], None)?
                .is_err(),
            "fatal ACK poisons resource until reconnect"
        );
        conn.resource_drop(&mut store)?;

        let undiscovered = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings)?
            .expect("settings");
        let item = WriteItem {
            spec: TagSpec {
                address: "D0".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            value: Value::Float(1.0),
        };
        let outcome = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, undiscovered, &[item.clone(), item], None)?
            .expect("initial discovery rejection");
        assert!(matches!(outcome, WriteProgress::Done(results)
            if results.len() == 2 && results.iter().all(Result::is_err)));
        undiscovered.resource_drop(&mut store)?;

        for (name, code, sd_count) in [
            ("Q25PRHCPU", 0x004c, Some(3014u16.to_le_bytes().to_vec())),
            ("Q02UCPU", 0x0263, Some(vec![32, 0])),
            ("R04CPU", 0x4800, Some(vec![])),
        ] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("settings");
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101");
            let sd = driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
                .expect("type");
            if let Some(words) = sd_count {
                assert!(sd.is_some());
                let mut sd_response = response(2, &words);
                if words.is_empty() {
                    sd_response[13] = 1;
                }
                driver
                    .banto_driver_session()
                    .connection()
                    .call_handshake_next(&mut store, conn, Some(&sd_response))?
                    .expect("SD");
            } else {
                assert!(sd.is_none());
            }
            let item = WriteItem {
                spec: TagSpec {
                    // D31 is allowlisted and was sent above with valid R SD
                    // evidence. A refusal here must be caused by missing SD.
                    address: if name == "R04CPU" { "D31" } else { "D0" }.into(),
                    data_type: "u16".into(),
                    string_words: None,
                },
                value: Value::Float(1.0),
            };
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, std::slice::from_ref(&item), None)?
                .expect("no-send rejection");
            if name == "R04CPU" {
                assert!(
                    matches!(&outcome, WriteProgress::Done(results)
                    if matches!(results.as_slice(), [Err(ItemError::Rejected(message))]
                        if message.contains("same-connection SD range"))),
                    "allowlisted D31 must be rejected specifically for missing R SD evidence"
                );
            }
            assert!(
                matches!(outcome, WriteProgress::Done(items) if matches!(items.as_slice(), [Err(_)])),
                "{name}"
            );
            let two_items = [item.clone(), item];
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &two_items, None)?
                .expect("whole non-ready write rejection");
            assert!(
                matches!(outcome, WriteProgress::Done(results)
                    if results.len() == 2 && results.iter().all(Result::is_err)),
                "all {name} items need a result"
            );
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    #[test]
    fn four_word_write_loopback_preflights_and_maps_all_items() -> wasmtime::Result<()> {
        use write_host::banto::driver::types::{ItemError, TagSpec, Value};
        use write_host::exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let driver = write_host::DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        let mut write_settings = settings();
        write_settings.push(banto::driver::types::Setting {
            key: "d_u16_write_mode".into(),
            value: "enabled".into(),
        });
        write_settings.push(banto::driver::types::Setting {
            key: "d_u16_write_allowlist".into(),
            value: "D3010-D3013".into(),
        });
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(
                &mut store,
                &write_settings
                    .into_iter()
                    .map(|s| write_host::banto::driver::types::Setting {
                        key: s.key,
                        value: s.value,
                    })
                    .collect::<Vec<_>>(),
            )?
            .expect("settings");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("SD280-283")
            .is_some());
        let mut sd_counts = 3014u32.to_le_bytes().to_vec();
        sd_counts.extend_from_slice(&8192u32.to_le_bytes());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &sd_counts)))?
            .expect("ready")
            .is_none());
        let items: Vec<WriteItem> = (0..4)
            .map(|i| WriteItem {
                spec: TagSpec {
                    address: format!("D{}", 3010 + i),
                    data_type: "u16".into(),
                    string_words: None,
                },
                value: Value::Float(f64::from(i + 1)),
            })
            .collect();
        let invalid_runs = vec![
            vec![items[0].clone(), items[2].clone()], // hole
            vec![items[0].clone(), items[0].clone()], // duplicate
            vec![items[1].clone(), items[0].clone()], // reverse
            vec![
                items[1].clone(),
                items[2].clone(),
                items[3].clone(),
                WriteItem {
                    spec: TagSpec {
                        address: "D3014".into(),
                        ..items[0].spec.clone()
                    },
                    ..items[0].clone()
                },
            ], // last point outside verified SD count
            vec![
                items[0].clone(),
                WriteItem {
                    value: Value::Float(f64::NAN),
                    ..items[1].clone()
                },
            ],
            vec![
                items[0].clone(),
                WriteItem {
                    spec: TagSpec {
                        data_type: "u32".into(),
                        ..items[1].spec.clone()
                    },
                    ..items[1].clone()
                },
            ],
        ];
        for invalid in invalid_runs {
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &invalid, None)?
                .expect("preflight");
            assert!(matches!(outcome, WriteProgress::Done(results)
                if results.len() == invalid.len() && results.iter().all(Result::is_err)));
        }
        let too_many = vec![items[0].clone(); 961];
        let over_limit = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &too_many, None)?
            .expect("word limit preflight");
        assert!(matches!(over_limit, WriteProgress::Done(results)
            if results.len() == 961 && results.iter().all(Result::is_err)));
        assert!(driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &[], None)?
            .is_err());
        let progress = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, None)?
            .expect("valid group");
        let WriteProgress::Send(plan) = progress else {
            panic!("expected one grouped request")
        };
        assert_eq!(plan.covers, [0, 1, 2, 3]);
        assert_eq!(
            &plan.step.send[2..4],
            &[3, 0],
            "rejected runs did not use serials"
        );
        assert_eq!(&plan.step.send[11..13], &[22, 0]);
        assert_eq!(
            &plan.step.send[15..],
            &[1, 0x14, 2, 0, 0xc2, 0x0b, 0, 0, 0xa8, 0, 4, 0, 1, 0, 2, 0, 3, 0, 4, 0,]
        );
        // A local PLC loopback applies only this one frame; no socket is opened.
        let memory = plan.step.send[27..].to_vec();
        let done = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, Some(&response(3, &[])))?
            .expect("ACK");
        assert!(matches!(done, WriteProgress::Done(results)
            if results.len() == 4 && results.iter().all(Result::is_ok)));
        let specs: Vec<TagSpec> = items.iter().map(|item| item.spec.clone()).collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("readback plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0, 1, 2, 3]);
        let readback = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[0], &specs, &response(4, &memory))?
            .expect("loopback readback");
        assert!(matches!(
            readback.as_slice(),
            [
                Ok(Value::Uint(1)),
                Ok(Value::Uint(2)),
                Ok(Value::Uint(3)),
                Ok(Value::Uint(4))
            ]
        ));
        let second = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, None)?
            .expect("second group");
        assert!(matches!(second, WriteProgress::Send(_)));
        let mut refused = response(5, &[]);
        refused[13..15].copy_from_slice(&0xc051u16.to_le_bytes());
        let result = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, Some(&refused))?
            .expect("device refusal");
        assert!(matches!(result, WriteProgress::Done(results)
            if results.len() == 4 && results.iter().all(|r| matches!(r, Err(ItemError::Device(e)) if e.code == 0xc051))));
        let third = driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, None)?
            .expect("third group");
        assert!(matches!(third, WriteProgress::Send(_)));
        assert!(
            driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .is_err(),
            "pending write cannot be resent"
        );
        assert!(driver
            .banto_driver_write_v1()
            .call_write_next(&mut store, conn, &items, Some(&response(99, &[])))?
            .is_err());
        assert!(
            driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .is_err(),
            "uncertain ACK poisons session"
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn q25prh_test_write_requires_opt_in_exact_pair_and_fixed_four_words() -> wasmtime::Result<()> {
        use write_host::banto::driver::types::{Setting, TagSpec, Value};
        use write_host::exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let driver = write_host::DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        let basic = vec![
            Setting {
                key: "host".into(),
                value: "fixture.invalid".into(),
            },
            Setting {
                key: "port".into(),
                value: "5000".into(),
            },
        ];
        let mut enabled = basic.clone();
        enabled.push(Setting {
            key: "q25prh_d3010_3013_test_write".into(),
            value: "enabled".into(),
        });
        let items: Vec<WriteItem> = (0..4)
            .map(|index| WriteItem {
                spec: TagSpec {
                    address: format!("D{}", 3010 + index),
                    data_type: "u16".into(),
                    string_words: None,
                },
                value: Value::Float(f64::from(index + 1)),
            })
            .collect();
        for (settings, name, code, allowed) in [
            (&basic, "Q25PRHCPU", 0x004c, false),
            (&enabled, "Q25PRHCPU", 0x004d, false),
            (&enabled, "Q25PRHCPX", 0x004c, false),
            (&enabled, "Q25PRHCPU", 0x004c, true),
        ] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, settings)?
                .expect("settings");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101")
                .is_some());
            let sd = driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
                .expect("0101 response");
            if name == "Q25PRHCPU" && code == 0x004c {
                let sd = sd.expect("Q25PRH SD302 request");
                assert_eq!(&sd.send[2..4], &[2, 0]);
                assert_eq!(&sd.send[17..25], &[0, 0, 0x2e, 1, 0, 0xa9, 1, 0]);
                let before_sd = driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &items, None)?
                    .expect("write requires SD evidence");
                assert!(matches!(before_sd, WriteProgress::Done(results)
                    if results.len() == 4 && results.iter().all(Result::is_err)));
                assert!(driver
                    .banto_driver_session()
                    .connection()
                    .call_handshake_next(
                        &mut store,
                        conn,
                        Some(&response(2, &3014u16.to_le_bytes()))
                    )?
                    .expect("SD302 count")
                    .is_none());
            } else {
                assert!(sd.is_none());
            }
            if !allowed {
                let outcome = driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &items, None)?
                    .expect("fail-closed write gate");
                assert!(matches!(outcome, WriteProgress::Done(results)
                    if results.len() == 4 && results.iter().all(Result::is_err)));
                conn.resource_drop(&mut store)?;
                continue;
            }
            for invalid in [
                items[..3].to_vec(),
                (3011..=3014)
                    .map(|number| WriteItem {
                        spec: TagSpec {
                            address: format!("D{number}"),
                            ..items[0].spec.clone()
                        },
                        value: Value::Float(1.0),
                    })
                    .collect(),
                (3000..=3003)
                    .map(|number| WriteItem {
                        spec: TagSpec {
                            address: format!("D{number}"),
                            ..items[0].spec.clone()
                        },
                        value: Value::Float(1.0),
                    })
                    .collect(),
                vec![
                    items[0].clone(),
                    items[1].clone(),
                    items[2].clone(),
                    WriteItem {
                        value: Value::Float(f64::NAN),
                        ..items[3].clone()
                    },
                ],
            ] {
                let outcome = driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &invalid, None)?
                    .expect("whole-run rejection");
                assert!(matches!(outcome, WriteProgress::Done(results)
                    if results.len() == invalid.len() && results.iter().all(Result::is_err)));
            }
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("opt-in Q test");
            let WriteProgress::Send(plan) = outcome else {
                panic!("Q test must send one frame")
            };
            assert_eq!(plan.covers, [0, 1, 2, 3]);
            assert_eq!(&plan.step.send[2..4], &[3, 0], "rejections used no serial");
            assert_eq!(&plan.step.send[11..13], &[20, 0]);
            assert_eq!(
                &plan.step.send[15..],
                &[1, 0x14, 0, 0, 0xc2, 0x0b, 0, 0xa8, 4, 0, 1, 0, 2, 0, 3, 0, 4, 0,]
            );
            let done = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, Some(&response(3, &[])))?
                .expect("ACK");
            assert!(matches!(done, WriteProgress::Done(results)
                if results.len() == 4 && results.iter().all(Result::is_ok)));
            let second = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("second plan");
            assert!(matches!(second, WriteProgress::Send(_)));
            assert!(
                driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &items, None)?
                    .is_err(),
                "pending Q write cannot resend"
            );
            assert!(driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, Some(&response(99, &[])))?
                .is_err());
            assert!(
                driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &items, None)?
                    .is_err(),
                "uncertain ACK poisons Q session"
            );
            conn.resource_drop(&mut store)?;
        }

        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &enabled)?
            .expect("settings");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101");
        let sd = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("Q25PRHCPU", 0x004c)))?
            .expect("type and SD302")
            .expect("SD302 request");
        assert_eq!(&sd.send[17..25], &[0, 0, 0x2e, 1, 0, 0xa9, 1, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &3014u16.to_le_bytes())))?
            .expect("SD302 reply")
            .is_none());
        let specs: Vec<TagSpec> = items.iter().map(|item| item.spec.clone()).collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("Q range remains unknown");
        assert_eq!(plans.len(), 4, "opt-in write must not verify Q D range");
        assert!(plans
            .iter()
            .enumerate()
            .all(|(i, plan)| plan.covers == [i as u32]));
        conn.resource_drop(&mut store)?;

        for (count, allowed) in [(3013u16, false), (32768u16, true)] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &enabled)?
                .expect("test settings");
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response("Q25PRHCPU", 0x004c)))?
                .expect("SD302")
                .is_some());
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&response(2, &count.to_le_bytes())))?
                .expect("SD302 count");
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("whole-span check");
            if allowed {
                assert!(matches!(outcome, WriteProgress::Send(plan)
                    if plan.covers == [0, 1, 2, 3] && plan.step.send[2..4] == [3, 0]));
            } else {
                assert!(matches!(outcome, WriteProgress::Done(results)
                    if results.len() == 4 && results.iter().all(Result::is_err)));
            }
            conn.resource_drop(&mut store)?;
        }

        for bad_sd in [false, true] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &enabled)?
                .expect("test settings");
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response("Q25PRHCPU", 0x004c)))?
                .expect("SD302")
                .is_some());
            let mut reply = response(2, &[]);
            if !bad_sd {
                reply[13] = 1; // PLC end-code refusal
            }
            let ready = driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&reply))?;
            assert_eq!(
                ready.is_err(),
                bad_sd,
                "short SD is fatal; refusal remains unknown"
            );
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("unknown SD count rejects write");
            assert!(matches!(outcome, WriteProgress::Done(results)
                if results.len() == 4 && results.iter().all(Result::is_err)));
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    #[test]
    fn normal_q_write_requires_saved_policy_and_same_connection_sd302() -> wasmtime::Result<()> {
        use write_host::banto::driver::types::{Setting, TagSpec, Value};
        use write_host::exports::banto::driver::write_v1::{WriteItem, WriteProgress};
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let linker = Linker::new(&engine);
        let mut store = Store::new(&engine, ());
        let driver = write_host::DriverWithWriteV1::instantiate(&mut store, &component, &linker)?;
        let items: Vec<WriteItem> = (0..3)
            .map(|index| WriteItem {
                spec: TagSpec {
                    address: format!("D{}", 4000 + index),
                    data_type: "u16".into(),
                    string_words: None,
                },
                value: Value::Float(f64::from(index + 1)),
            })
            .collect();
        for (mode, sd_count, allowed) in [
            ("disabled", 5000u16, false),
            ("enabled", 4002, false),
            ("enabled", 5000, true),
        ] {
            let settings = vec![
                Setting {
                    key: "host".into(),
                    value: "fixture.invalid".into(),
                },
                Setting {
                    key: "port".into(),
                    value: "5000".into(),
                },
                Setting {
                    key: "d_u16_write_mode".into(),
                    value: mode.into(),
                },
                Setting {
                    key: "d_u16_write_allowlist".into(),
                    value: "D3010-D3013,D4000-D4009".into(),
                },
            ];
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings)?
                .expect("valid policy");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101")
                .is_some());
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response("Q25PRHCPU", 0x004c)))?
                .expect("SD302")
                .is_some());
            let before_sd = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("no SD proof");
            assert!(matches!(before_sd, WriteProgress::Done(results)
                if results.len() == items.len() && results.iter().all(Result::is_err)));
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(
                    &mut store,
                    conn,
                    Some(&response(2, &sd_count.to_le_bytes())),
                )?
                .expect("SD302 response");
            for invalid in [
                vec![WriteItem {
                    spec: TagSpec {
                        address: "D3000".into(),
                        ..items[0].spec.clone()
                    },
                    ..items[0].clone()
                }],
                vec![items[0].clone(), items[2].clone()],
                vec![WriteItem {
                    spec: TagSpec {
                        address: "D4010".into(),
                        ..items[0].spec.clone()
                    },
                    ..items[0].clone()
                }],
                vec![WriteItem {
                    spec: TagSpec {
                        data_type: "u32".into(),
                        ..items[0].spec.clone()
                    },
                    ..items[0].clone()
                }],
                vec![WriteItem {
                    value: Value::Float(f64::NAN),
                    ..items[0].clone()
                }],
            ] {
                let outcome = driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &invalid, None)?
                    .expect("preflight");
                assert!(matches!(outcome, WriteProgress::Done(results)
                    if results.len() == invalid.len() && results.iter().all(Result::is_err)));
            }
            let outcome = driver
                .banto_driver_write_v1()
                .call_write_next(&mut store, conn, &items, None)?
                .expect("policy");
            if allowed {
                let WriteProgress::Send(plan) = outcome else {
                    panic!("enabled policy must send")
                };
                assert_eq!(plan.covers, [0, 1, 2]);
                assert_eq!(&plan.step.send[2..4], &[3, 0], "rejections use no serial");
                assert_eq!(&plan.step.send[11..13], &[18, 0]);
                assert_eq!(
                    &plan.step.send[15..],
                    &[1, 0x14, 0, 0, 0xa0, 0x0f, 0, 0xa8, 3, 0, 1, 0, 2, 0, 3, 0]
                );
                let done = driver
                    .banto_driver_write_v1()
                    .call_write_next(&mut store, conn, &items, Some(&response(3, &[])))?
                    .expect("ACK");
                assert!(matches!(done, WriteProgress::Done(results)
                    if results.len() == items.len() && results.iter().all(Result::is_ok)));
            } else {
                assert!(matches!(outcome, WriteProgress::Done(results)
                    if results.len() == items.len() && results.iter().all(Result::is_err)));
            }
            conn.resource_drop(&mut store)?;
        }
        Ok(())
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

    fn d_specs(addresses: &[u32]) -> Vec<banto::driver::types::TagSpec> {
        addresses
            .iter()
            .map(|address| banto::driver::types::TagSpec {
                address: format!("D{address}"),
                data_type: "u16".into(),
                string_words: None,
            })
            .collect()
    }

    fn bit_spec(address: &str) -> banto::driver::types::TagSpec {
        banto::driver::types::TagSpec {
            address: address.into(),
            data_type: "bit".into(),
            string_words: None,
        }
    }

    #[test]
    fn mxyb_bits_use_separate_q_requests_and_typed_boolean_values() -> wasmtime::Result<()> {
        use banto::driver::types::{ItemError, Value};
        let (mut store, driver) = load()?;
        let conn = ready_q(&mut store, &driver, 100)?;
        let specs = [
            bit_spec("M10"),
            bit_spec("X0F"),
            bit_spec("Y10"),
            bit_spec("B1A"),
            banto::driver::types::TagSpec {
                address: "D10".into(),
                data_type: "u16".into(),
                string_words: None,
            },
        ];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(
            plans.len(),
            5,
            "unknown bit counts never cause adjacent batching"
        );
        for (index, plan) in plans.iter().enumerate() {
            assert_eq!(plan.covers, [index as u32]);
            let expected = match index {
                0 => [1, 0, 10, 0, 0, 0x90, 1, 0],
                1 => [1, 0, 15, 0, 0, 0x9c, 1, 0],
                2 => [1, 0, 16, 0, 0, 0x9d, 1, 0],
                3 => [1, 0, 26, 0, 0, 0xa0, 1, 0],
                _ => [0, 0, 10, 0, 0, 0xa8, 1, 0],
            };
            assert_eq!(&plan.step.send[17..25], &expected);
            let serial = u16::from_le_bytes([plan.step.send[2], plan.step.send[3]]);
            let body: &[u8] = if index == 4 {
                &[0x34, 0x12]
            } else if index % 2 == 0 {
                &[0x10]
            } else {
                &[0]
            };
            let results = driver
                .banto_driver_read()
                .call_decode(&mut store, conn, plan, &specs, &response(serial, body))?
                .expect("decode");
            assert!(
                matches!(results.as_slice(), [Ok(Value::Uint(0x1234))]) && index == 4
                    || matches!(results.as_slice(), [Ok(Value::Bool(true))])
                        && index < 4
                        && index % 2 == 0
                    || matches!(results.as_slice(), [Ok(Value::Bool(false))])
                        && index < 4
                        && index % 2 == 1
            );
        }
        let plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &[bit_spec("Y11")])?
            .expect("refusal plan")
            .remove(0);
        let serial = u16::from_le_bytes([plan.step.send[2], plan.step.send[3]]);
        let mut refused = response(serial, &[]);
        refused[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        let result = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plan, &[bit_spec("Y11")], &refused)?
            .expect("item error");
        assert!(
            matches!(result.as_slice(), [Err(ItemError::Device(error))] if error.code == 0xc05b)
        );
        let plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &[bit_spec("B1B")])?
            .expect("bad bit plan")
            .remove(0);
        let serial = u16::from_le_bytes([plan.step.send[2], plan.step.send[3]]);
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    conn,
                    &plan,
                    &[bit_spec("B1B")],
                    &response(serial, &[0x11])
                )
                .expect("component call")
                .is_err(),
            "odd final padding nibble must be zero"
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn additional_q_r_areas_read_with_unknown_counts_and_no_write_capability(
    ) -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
        let (mut store, driver) = load()?;
        for (name, code, sd_words, bit_sub, word_sub, code_at) in [
            ("Q02UCPU", 0x0263, vec![100], [1, 0], [0, 0], 22),
            ("R04CPU", 0x4800, vec![100, 0, 100, 0], [3, 0], [2, 0], 23),
        ] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings())?
                .expect("settings");
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101")
                .is_some());
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
                .expect("SD request")
                .is_some());
            let sd_bytes: Vec<u8> = sd_words.into_iter().flat_map(u16::to_le_bytes).collect();
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&response(2, &sd_bytes)))?
                .expect("ready")
                .is_none());
            let mut specs = ["L10", "F11", "V12", "SB1A", "SM13"].map(bit_spec).to_vec();
            specs.extend(
                [("SW1A", "u32"), ("SD290", "u16")].map(|(address, data_type)| TagSpec {
                    address: address.into(),
                    data_type: data_type.into(),
                    string_words: None,
                }),
            );
            let plans = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .expect("plans");
            assert_eq!(plans.len(), specs.len(), "unknown counts remain individual");
            for (index, plan) in plans.iter().enumerate() {
                let (device_code, subcommand, body): (u8, [u8; 2], &[u8]) = match index {
                    0 => (0x92, bit_sub, &[0x10]),
                    1 => (0x93, bit_sub, &[0]),
                    2 => (0x94, bit_sub, &[0x10]),
                    3 => (0xa1, bit_sub, &[0]),
                    4 => (0x91, bit_sub, &[0x10]),
                    5 => (0xb5, word_sub, &[0x34, 0x12, 0xcd, 0xab]),
                    _ => (0xa9, word_sub, &[0x78, 0x56]),
                };
                assert_eq!(plan.covers, [index as u32]);
                assert_eq!(&plan.step.send[17..19], &subcommand);
                assert_eq!(plan.step.send[code_at], device_code);
                let parsed = driver
                    .banto_driver_address()
                    .call_parse(
                        &mut store,
                        &specs[index].address,
                        &specs[index].data_type,
                        None,
                    )?
                    .expect("static address");
                assert!(!parsed.writable_area);
                let serial = u16::from_le_bytes([plan.step.send[2], plan.step.send[3]]);
                let values = driver
                    .banto_driver_read()
                    .call_decode(&mut store, conn, plan, &specs, &response(serial, body))?
                    .expect("values");
                assert!(matches!(
                    (index, values.as_slice()),
                    (0 | 2 | 4, [Ok(Value::Bool(true))])
                        | (1 | 3, [Ok(Value::Bool(false))])
                        | (5, [Ok(Value::Uint(0xabcd_1234))])
                        | (6, [Ok(Value::Uint(0x5678))])
                ));
            }
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    #[test]
    fn r_and_q25prh_bit_reads_keep_unproven_ranges_unknown() -> wasmtime::Result<()> {
        use banto::driver::types::Value;
        let (mut store, driver) = load()?;
        for (name, code, sd_words, subcommand, width) in [
            ("R04CPU", 0x4800, vec![32, 0, 32, 0], [3, 0], 10),
            ("Q25PRHCPU", 0x004c, vec![32], [1, 0], 8),
        ] {
            let conn = driver
                .banto_driver_session()
                .connection()
                .call_create(&mut store, &settings())?
                .expect("settings");
            driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, None)?
                .expect("0101");
            let sd = driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&model_response(name, code)))?
                .expect("SD request")
                .expect("SD step");
            assert_eq!(
                &sd.send[17..19],
                if width == 10 { &[2, 0] } else { &[0, 0] }
            );
            let sd_bytes: Vec<u8> = sd_words.into_iter().flat_map(u16::to_le_bytes).collect();
            assert!(driver
                .banto_driver_session()
                .connection()
                .call_handshake_next(&mut store, conn, Some(&response(2, &sd_bytes)))?
                .expect("ready")
                .is_none());
            let specs = [bit_spec("B1A"), bit_spec("B1B")];
            let plans = driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &specs)?
                .expect("bit plans");
            assert_eq!(plans.len(), 2, "SD D/W values must not verify the B range");
            assert_eq!(plans[0].covers, [0]);
            assert_eq!(plans[1].covers, [1]);
            for (index, plan) in plans.iter().enumerate() {
                assert_eq!(&plan.step.send[17..19], &subcommand);
                assert_eq!(plan.step.send[if width == 10 { 23 } else { 22 }], 0xa0);
                let serial = u16::from_le_bytes([plan.step.send[2], plan.step.send[3]]);
                let result = driver
                    .banto_driver_read()
                    .call_decode(
                        &mut store,
                        conn,
                        plan,
                        &specs,
                        &response(serial, &[if index == 0 { 0x10 } else { 0 }]),
                    )?
                    .expect("bit value");
                assert!(
                    matches!(result.as_slice(), [Ok(Value::Bool(true))]) && index == 0
                        || matches!(result.as_slice(), [Ok(Value::Bool(false))]) && index == 1
                );
            }
            conn.resource_drop(&mut store)?;
        }
        Ok(())
    }

    fn ready_q(
        store: &mut Store<()>,
        driver: &Driver,
        count: u16,
    ) -> wasmtime::Result<exports::banto::driver::session::Connection> {
        ready_q_with_settings(store, driver, count, &settings())
    }

    fn ready_q_with_settings(
        store: &mut Store<()>,
        driver: &Driver,
        count: u16,
        settings: &[banto::driver::types::Setting],
    ) -> wasmtime::Result<exports::banto::driver::session::Connection> {
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut *store, settings)?
            .expect("settings");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut *store, conn, None)?
            .expect("0101")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut *store, conn, Some(&model_response("Q02UCPU", 0x0263)))?
            .expect("SD302")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut *store, conn, Some(&response(2, &count.to_le_bytes())))?
            .expect("ready")
            .is_none());
        Ok(conn)
    }

    #[test]
    fn mixed_width_numeric_batch_maps_words_to_tag_covers() -> wasmtime::Result<()> {
        use banto::driver::types::{ItemError, TagSpec, Value};
        let (mut store, driver) = load()?;
        let conn = ready_q(&mut store, &driver, 100)?;
        let specs: Vec<TagSpec> = [
            (10, "u16"),
            (11, "i16"),
            (12, "i32"),
            (14, "u32"),
            (16, "f32"),
            (18, "i64"),
            (22, "u64"),
            (26, "f64"),
        ]
        .into_iter()
        .map(|(address, kind)| TagSpec {
            address: format!("D{address}"),
            data_type: kind.into(),
            string_words: None,
        })
        .collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(&plans[0].step.send[19..25], &[10, 0, 0, 0xa8, 20, 0]);
        let words: [u16; 20] = [
            0x1234, 0x8000, 0, 0x8000, 0xabcd, 0x1234, 0, 0x3fc0, 0, 0, 0, 0x8000, 1, 0, 0, 0x20,
            0, 0, 0, 0x3ff8,
        ];
        let data: Vec<u8> = words.into_iter().flat_map(u16::to_le_bytes).collect();
        let values = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[0], &specs, &response(3, &data))?
            .expect("values");
        assert!(matches!(
            values.as_slice(),
            [
                Ok(Value::Uint(0x1234)),
                Ok(Value::Int(-32768)),
                Ok(Value::Int(-2147483648)),
                Ok(Value::Uint(0x1234_abcd)),
                Ok(Value::Float(1.5)),
                Ok(Value::Int(i64::MIN)),
                Ok(Value::Uint(9_007_199_254_740_993)),
                Ok(Value::Float(1.5)),
            ]
        ));
        let retry = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("new plan");
        let mut refused = response(4, &[]);
        refused[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        let errors = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &retry[0], &specs, &refused)?
            .expect("per-tag refusal");
        assert_eq!(errors.len(), 8);
        assert!(errors
            .iter()
            .all(|error| matches!(error, Err(ItemError::Device(e)) if e.code == 0xc05b)));
        let nan = [TagSpec {
            address: "D30".into(),
            data_type: "f32".into(),
            string_words: None,
        }];
        let nan_plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &nan)?
            .expect("nan plan");
        let nan_values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &nan_plan[0],
                &nan,
                &response(5, &[0, 0, 0xc0, 0x7f]),
            )?
            .expect("nan value");
        assert!(matches!(nan_values.as_slice(), [Ok(Value::Float(value))] if value.is_nan()));
        conn.resource_drop(&mut store)?;

        let mut high_low = settings();
        high_low.push(banto::driver::types::Setting {
            key: "word_order".into(),
            value: "high_low".into(),
        });
        let conn = ready_q_with_settings(&mut store, &driver, 100, &high_low)?;
        let high_specs = [
            TagSpec {
                address: "D0".into(),
                data_type: "i32".into(),
                string_words: None,
            },
            TagSpec {
                address: "D2".into(),
                data_type: "f64".into(),
                string_words: None,
            },
        ];
        let high_plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &high_specs)?
            .expect("high-low plan");
        assert_eq!(high_plan[0].covers, [0, 1]);
        assert_eq!(&high_plan[0].step.send[19..25], &[0, 0, 0, 0xa8, 6, 0]);
        let high_words: [u16; 6] = [0x8000, 0, 0x3ff8, 0, 0, 0];
        let data: Vec<u8> = high_words.into_iter().flat_map(u16::to_le_bytes).collect();
        let high_values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &high_plan[0],
                &high_specs,
                &response(3, &data),
            )?
            .expect("high-low values");
        assert!(matches!(
            high_values.as_slice(),
            [Ok(Value::Int(-2147483648)), Ok(Value::Float(1.5))]
        ));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn multiword_range_boundary_unknown_split_and_960_word_limit() -> wasmtime::Result<()> {
        use banto::driver::types::TagSpec;
        let (mut store, driver) = load()?;
        let conn = ready_q(&mut store, &driver, 20)?;
        let specs: Vec<TagSpec> = [(16, "f64"), (20, "i16"), (21, "u32")]
            .into_iter()
            .map(|(address, kind)| TagSpec {
                address: format!("D{address}"),
                data_type: kind.into(),
                string_words: None,
            })
            .collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(
            plans
                .iter()
                .map(|p| p.covers.as_slice())
                .collect::<Vec<_>>(),
            vec![&[0][..], &[1][..], &[2][..]]
        );
        assert_eq!(&plans[0].step.send[19..25], &[16, 0, 0, 0xa8, 4, 0]);
        assert_eq!(&plans[1].step.send[19..25], &[20, 0, 0, 0xa8, 1, 0]);
        assert_eq!(&plans[2].step.send[19..25], &[21, 0, 0, 0xa8, 2, 0]);
        conn.resource_drop(&mut store)?;

        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("R settings");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("R 0101")
            .expect("R type send");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("R SD")
            .expect("R SD send");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[100, 0, 0, 0, 32, 0, 0, 0]))
            )?
            .expect("R ready")
            .is_none());
        let specs: Vec<TagSpec> = [(98, "u32"), (99, "i32"), (100, "u16")]
            .into_iter()
            .map(|(address, kind)| TagSpec {
                address: format!("D{address}"),
                data_type: kind.into(),
                string_words: None,
            })
            .collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("R plans");
        assert_eq!(
            plans.len(),
            1,
            "R exact count rejects a tag crossing its end"
        );
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(&plans[0].step.send[19..27], &[98, 0, 0, 0, 0xa8, 0, 2, 0]);
        let result = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x34, 0x12, 0x78, 0x56]),
            )?
            .expect("R value");
        assert!(matches!(
            result.as_slice(),
            [Ok(banto::driver::types::Value::Uint(0x5678_1234))]
        ));
        conn.resource_drop(&mut store)?;

        let conn = ready_q(&mut store, &driver, 1000)?;
        let specs: Vec<TagSpec> = (0..241)
            .map(|n| TagSpec {
                address: format!("D{}", n * 4),
                data_type: "f64".into(),
                string_words: None,
            })
            .collect();
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("word limit plans");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers.len(), 240);
        assert_eq!(plans[1].covers, [240]);
        assert_eq!(&plans[0].step.send[19..25], &[0, 0, 0, 0xa8, 0xc0, 3]);
        assert_eq!(&plans[1].step.send[19..25], &[0xc0, 3, 0, 0xa8, 4, 0]);
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn q25prh_exact_pair_reads_sd302_but_keeps_general_reads_unknown() -> wasmtime::Result<()> {
        use banto::driver::types::Value;
        let (mut store, driver) = load()?;
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        let type_step = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("0101")
            .expect("type request");
        assert_eq!(&type_step.send[2..4], &[1, 0]);
        assert_eq!(&type_step.send[15..19], &[1, 1, 0, 0]);
        let sd = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("Q25PRHCPU", 0x004c)))?
            .expect("Q wire and SD302 map")
            .expect("SD302 request");
        assert_eq!(&sd.send[2..4], &[2, 0]);
        assert_eq!(&sd.send[17..25], &[0, 0, 0x2e, 1, 0, 0xa9, 1, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &3014u16.to_le_bytes())))?
            .expect("ready after SD")
            .is_none());
        let specs = d_specs(&[5, 6]);
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("explicit D reads with unknown range");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[1].covers, [1]);
        assert_eq!(&plans[0].step.send[2..4], &[3, 0]);
        assert_eq!(&plans[1].step.send[2..4], &[4, 0]);
        assert_eq!(&plans[0].step.send[17..25], &[0, 0, 5, 0, 0, 0xa8, 1, 0]);
        let values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x34, 0x12]),
            )?
            .expect("one explicit D word");
        assert!(matches!(values.as_slice(), [Ok(Value::Uint(0x1234))]));
        let later = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("successful read did not verify range");
        assert_eq!(later.len(), 2);
        assert_eq!(later[0].covers, [0]);
        assert_eq!(later[1].covers, [1]);
        conn.resource_drop(&mut store)?;

        let fresh = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("fresh connection");
        let fresh_type = driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, None)?
            .expect("fresh 0101")
            .expect("new resource repeats type request");
        assert_eq!(&fresh_type.send[2..4], &[1, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                fresh,
                Some(&model_response("Q25PRHCPU", 0x004b))
            )?
            .expect("mismatched model code remains unknown")
            .is_none());
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, fresh, &specs)?
            .is_err());
        fresh.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn verified_contiguous_d_batches_preserve_covers_values_and_unknown_split(
    ) -> wasmtime::Result<()> {
        use banto::driver::types::{ItemError, Value};
        let (mut store, driver) = load()?;
        let conn = ready_q(&mut store, &driver, 100)?;
        let specs = d_specs(&[98, 99, 100, 101, 5, 6, 6, 7, 9]);
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(plans.len(), 6);
        assert_eq!(
            plans
                .iter()
                .map(|p| p.covers.as_slice())
                .collect::<Vec<_>>(),
            vec![
                &[0, 1][..],
                &[2][..],
                &[3][..],
                &[4, 5][..],
                &[6, 7][..],
                &[8][..]
            ]
        );
        assert_eq!(&plans[0].step.send[19..25], &[98, 0, 0, 0xa8, 2, 0]);
        assert_eq!(plans[0].step.max_len, 64);
        let first = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x11, 0, 0x22, 0]),
            )?
            .expect("batch");
        assert!(matches!(
            first.as_slice(),
            [Ok(Value::Uint(0x11)), Ok(Value::Uint(0x22))]
        ));
        let unknown = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[1],
                &specs,
                &response(4, &[0x33, 0]),
            )?
            .expect("unknown explicit read");
        assert!(matches!(unknown.as_slice(), [Ok(Value::Uint(0x33))]));
        let mut refusal = response(6, &[]);
        refusal[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        let refused = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[3], &specs, &refusal)?
            .expect("batch refusal is per-item");
        assert!(
            matches!(refused.as_slice(), [Err(ItemError::Device(a)), Err(ItemError::Device(b))] if a.code == 0xc05b && b.code == 0xc05b)
        );
        let duplicate = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[4],
                &specs,
                &response(7, &[0x44, 0, 0x55, 0]),
            )?
            .expect("new run after duplicate");
        assert!(matches!(
            duplicate.as_slice(),
            [Ok(Value::Uint(0x44)), Ok(Value::Uint(0x55))]
        ));
        let next = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &d_specs(&[99, 100]))?
            .expect("unknown was not promoted");
        assert_eq!(
            next.iter().map(|p| p.covers.as_slice()).collect::<Vec<_>>(),
            vec![&[0][..], &[1][..]]
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn batch_splits_at_official_960_word_limit_and_rejects_short_response() -> wasmtime::Result<()>
    {
        use banto::driver::types::Value;
        let (mut store, driver) = load()?;
        let conn = ready_q(&mut store, &driver, 1000)?;
        let specs = d_specs(&(0..=960).collect::<Vec<_>>());
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers.len(), 960);
        assert_eq!(plans[0].covers[0], 0);
        assert_eq!(plans[0].covers[959], 959);
        assert_eq!(plans[0].step.send[23..25], 960u16.to_le_bytes());
        assert_eq!(plans[0].step.max_len, 1935);
        assert_eq!(plans[1].covers, [960]);
        assert_eq!(plans[1].step.send[23..25], [1, 0]);
        let mut body = vec![0; 1920];
        body[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        body[1918..1920].copy_from_slice(&0x5678u16.to_le_bytes());
        let values = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[0], &specs, &response(3, &body))?
            .expect("full 960-word response");
        assert_eq!(values.len(), 960);
        assert!(matches!(values[0], Ok(Value::Uint(0x1234))));
        assert!(matches!(values[959], Ok(Value::Uint(0x5678))));
        let retry = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &d_specs(&[0, 1]))?
            .expect("new batch");
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    conn,
                    &retry[0],
                    &d_specs(&[0, 1]),
                    &response(5, &[1, 0])
                )?
                .is_err(),
            "short batch response is fatal; no partial tag values"
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn r_w_unknown_range_keeps_explicit_tags_separate() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
        let (mut store, driver) = load()?;
        let conn = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("settings");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, None)?
            .expect("start");
        driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("model");
        let mut refusal = response(2, &[]);
        refusal[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&refusal))?
            .expect("SD refusal")
            .is_none());
        let specs = ["W1E", "W1F"].map(|address| TagSpec {
            address: address.into(),
            data_type: "u16".into(),
            string_words: None,
        });
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("explicit W plans");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[1].covers, [1]);
        assert_eq!(plans[0].step.send[23], 0xb4);
        let value = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x34, 0x12]),
            )?
            .expect("unknown-range W read");
        assert!(matches!(value.as_slice(), [Ok(Value::Uint(0x1234))]));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn r_exact_count_batches_prefix_and_leaves_outside_uncovered() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
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
            .expect("0101")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("SD280 pair")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[32, 0, 0, 0, 32, 0, 0, 0]))
            )?
            .expect("ready")
            .is_none());
        let specs = d_specs(&[30, 31, 32]);
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("plans");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].covers, [0, 1]);
        assert_eq!(&plans[0].step.send[19..27], &[30, 0, 0, 0, 0xa8, 0, 2, 0]);
        let values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x11, 0, 0x22, 0]),
            )?
            .expect("values");
        assert!(matches!(
            values.as_slice(),
            [Ok(Value::Uint(0x11)), Ok(Value::Uint(0x22))]
        ));
        let w_specs = ["W1E", "W1F", "W20"].map(|address| TagSpec {
            address: address.into(),
            data_type: "u16".into(),
            string_words: None,
        });
        let w_plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &w_specs)?
            .expect("W plans");
        assert_eq!(w_plans.len(), 1);
        assert_eq!(w_plans[0].covers, [0, 1]);
        assert_eq!(&w_plans[0].step.send[19..27], &[30, 0, 0, 0, 0xb4, 0, 2, 0]);
        let w_values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &w_plans[0],
                &w_specs,
                &response(4, &[0x33, 0, 0x44, 0]),
            )?
            .expect("W values");
        assert!(matches!(
            w_values.as_slice(),
            [Ok(Value::Uint(0x33)), Ok(Value::Uint(0x44))]
        ));
        let w_pair = [TagSpec {
            address: "W1E".into(),
            data_type: "u32".into(),
            string_words: None,
        }];
        let pair_plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &w_pair)?
            .expect("complete W/u32 plan");
        assert_eq!(
            &pair_plan[0].step.send[19..27],
            &[30, 0, 0, 0, 0xb4, 0, 2, 0]
        );
        let pair_value = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &pair_plan[0],
                &w_pair,
                &response(5, &[0x34, 0x12, 0xcd, 0xab]),
            )?
            .expect("complete W/u32 value");
        assert!(matches!(
            pair_value.as_slice(),
            [Ok(Value::Uint(0xabcd_1234))]
        ));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn q_0101_sd302_then_verified_and_unknown_d_reads_run_in_real_component() -> wasmtime::Result<()>
    {
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
        let outside_plan = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &outside)?
            .expect("Q SD302 is internal-only, so D100 remains unknown");
        let outside_value = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &outside_plan[0],
                &outside,
                &response(4, &[0x78, 0x56]),
            )?
            .expect("explicit unknown-range read");
        assert!(matches!(
            outside_value.as_slice(),
            [Ok(Value::Uint(0x5678))]
        ));
        assert!(
            driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &outside)?
                .is_ok(),
            "read success must not promote internal-only count to a full D range"
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn multiple_q_tags_keep_order_covers_values_and_device_error_separate() -> wasmtime::Result<()>
    {
        use banto::driver::types::{ItemError, TagSpec, Value};
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
            .expect("model")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &[100, 0])))?
            .expect("SD302")
            .is_none());

        let specs = ["D5", "D100", "D7", "D5"].map(|address| TagSpec {
            address: address.into(),
            data_type: "u16".into(),
            string_words: None,
        });
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("one request per valid tag, including unknown-range D100");
        assert_eq!(plans.len(), specs.len());
        for (index, plan) in plans.iter().enumerate() {
            assert_eq!(plan.covers, [index as u32]);
            assert_eq!(&plan.step.send[2..4], &((index as u16) + 3).to_le_bytes());
            assert_eq!(&plan.step.send[17..19], &[0, 0]); // 0401 word read
            assert_eq!(plan.step.send[19], [5, 100, 7, 5][index]);
            assert_eq!(plan.step.send[23], 1); // one D word, no combined batch
        }
        let third = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[2],
                &specs,
                &response(5, &[0x33, 0x33]),
            )?
            .expect("third tag");
        assert!(matches!(third.as_slice(), [Ok(Value::Uint(0x3333))]));
        let first = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x11, 0x11]),
            )?
            .expect("first tag");
        assert!(matches!(first.as_slice(), [Ok(Value::Uint(0x1111))]));
        let mut refusal = response(4, &[]);
        refusal[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        let second = driver
            .banto_driver_read()
            .call_decode(&mut store, conn, &plans[1], &specs, &refusal)?
            .expect("one device refusal is an item error");
        assert!(matches!(second.as_slice(), [Err(ItemError::Device(e))] if e.code == 0xc05b));
        let fourth = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[3],
                &specs,
                &response(6, &[0x44, 0x44]),
            )?
            .expect("last tag still decodes after the individual refusal");
        assert!(matches!(fourth.as_slice(), [Ok(Value::Uint(0x4444))]));
        let beyond_wire = [TagSpec {
            address: "D16777216".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &beyond_wire)?
            .expect("Q wire-number overflow is item-level unsupported")
            .is_empty());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn mixed_valid_and_unsupported_r_tags_leave_only_valid_covers() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
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
            .call_handshake_next(&mut store, conn, Some(&model_response("R04CPU", 0x4800)))?
            .expect("model")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[32, 0, 0, 0, 32, 0, 0, 0]))
            )?
            .expect("SD280 pair")
            .is_none());
        let specs = [
            TagSpec {
                address: "D31".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            TagSpec {
                address: "D32".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            TagSpec {
                address: "Dfoo".into(),
                data_type: "u16".into(),
                string_words: None,
            },
            TagSpec {
                address: "D0".into(),
                data_type: "string".into(),
                string_words: None,
            },
            TagSpec {
                address: "D0".into(),
                data_type: "u16".into(),
                string_words: None,
            },
        ];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("invalid tags are omitted, not a connection fatal");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].covers, [0]);
        assert_eq!(plans[1].covers, [4]);
        assert_eq!(&plans[0].step.send[2..4], &[3, 0]);
        assert_eq!(&plans[1].step.send[2..4], &[4, 0]);
        let last = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[1],
                &specs,
                &response(4, &[0x22, 0x22]),
            )?
            .expect("last valid tag");
        assert!(matches!(last.as_slice(), [Ok(Value::Uint(0x2222))]));
        let first = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &specs,
                &response(3, &[0x11, 0x11]),
            )?
            .expect("first valid tag");
        assert!(matches!(first.as_slice(), [Ok(Value::Uint(0x1111))]));
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn swapped_cover_or_response_serial_is_fatal() -> wasmtime::Result<()> {
        use banto::driver::types::TagSpec;
        let (mut store, driver) = load()?;
        let specs = ["D5", "D5"].map(|address| TagSpec {
            address: address.into(),
            data_type: "u16".into(),
            string_words: None,
        });
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
            .expect("model")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&response(2, &[100, 0])))?
            .expect("SD302")
            .is_none());
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .expect("duplicate-address tags have distinct covers and serials");
        let mut swapped = plans[1].clone();
        swapped.covers = vec![0];
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    conn,
                    &swapped,
                    &specs,
                    &response(4, &[0x11, 0x11]),
                )?
                .is_err(),
            "same address cannot mask a swapped cover"
        );
        conn.resource_drop(&mut store)?; // the host closes on a fatal error

        let fresh = driver
            .banto_driver_session()
            .connection()
            .call_create(&mut store, &settings())?
            .expect("fresh settings");
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, None)?
            .expect("start")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, Some(&model_response("Q02UCPU", 0x0263)))?
            .expect("model")
            .is_some());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, fresh, Some(&response(2, &[100, 0])))?
            .expect("SD302")
            .is_none());
        let one = driver
            .banto_driver_read()
            .call_plan(&mut store, fresh, &specs[..1])?
            .expect("one pending read");
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    fresh,
                    &one[0],
                    &specs[..1],
                    &response(9, &[0x22, 0x22]),
                )?
                .is_err(),
            "wrong response serial is fatal"
        );
        assert!(
            driver
                .banto_driver_read()
                .call_decode(
                    &mut store,
                    fresh,
                    &one[0],
                    &specs[..1],
                    &response(3, &[0x22, 0x22]),
                )?
                .is_err(),
            "late response must not replay a consumed plan"
        );
        fresh.resource_drop(&mut store)?;
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
        assert_eq!(&sd.send[17..27], &[2, 0, 0x18, 1, 0, 0, 0xa9, 0, 4, 0]);
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(
                &mut store,
                conn,
                Some(&response(2, &[0x20, 0, 0, 0, 32, 0, 0, 0]))
            )?
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
        let beyond = [TagSpec {
            address: "D32".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(
            driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &beyond)?
                .expect("out-of-range D has an item-level unsupported result")
                .is_empty(),
            "R SD280 pair is the complete D count"
        );
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
            .call_handshake_next(
                &mut store,
                fresh,
                Some(&response(2, &[0x20, 0, 0, 0, 32, 0, 0, 0]))
            )?
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
    fn unknown_0101_pair_does_not_guess_wire_format_or_plan_sd() -> wasmtime::Result<()> {
        use banto::driver::types::TagSpec;
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
        let specs = [TagSpec {
            address: "D0".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &specs)?
            .is_err());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn refused_0101_has_no_model_or_read_plan() -> wasmtime::Result<()> {
        use banto::driver::types::TagSpec;
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
        let mut refusal = response(1, &[]);
        refusal[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
        assert!(driver
            .banto_driver_session()
            .connection()
            .call_handshake_next(&mut store, conn, Some(&refusal))?
            .expect("valid PLC refusal, no model")
            .is_none());
        let spec = [TagSpec {
            address: "D0".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &spec)?
            .is_err());
        conn.resource_drop(&mut store)?;
        Ok(())
    }

    #[test]
    fn rejected_sd_leaves_unknown_range_but_allows_one_explicit_read() -> wasmtime::Result<()> {
        use banto::driver::types::{TagSpec, Value};
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
            .expect("SD refusal leaves range unknown")
            .is_none());
        let spec = [TagSpec {
            address: "D0".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        let plans = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &spec)?
            .expect("explicit D0 read with unknown range");
        let values = driver
            .banto_driver_read()
            .call_decode(
                &mut store,
                conn,
                &plans[0],
                &spec,
                &response(3, &[0x34, 0x12]),
            )?
            .expect("D0 value");
        assert!(matches!(values.as_slice(), [Ok(Value::Uint(0x1234))]));
        assert!(
            driver
                .banto_driver_read()
                .call_plan(&mut store, conn, &spec)?
                .is_ok(),
            "a successful read leaves range unknown"
        );
        let invalid = [TagSpec {
            address: "Dfoo".into(),
            data_type: "u16".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &invalid)?
            .expect("invalid tag is left uncovered")
            .is_empty());
        let unsupported = [TagSpec {
            address: "D0".into(),
            data_type: "string".into(),
            string_words: None,
        }];
        assert!(driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &unsupported)?
            .expect("unsupported type is left uncovered")
            .is_empty());
        let two = driver
            .banto_driver_read()
            .call_plan(&mut store, conn, &[spec[0].clone(), spec[0].clone()])?
            .expect("two explicit reads with unknown range");
        assert_eq!(
            two.iter().map(|p| p.covers.as_slice()).collect::<Vec<_>>(),
            vec![&[0][..], &[1][..]]
        );
        conn.resource_drop(&mut store)?;
        Ok(())
    }
}
