//! Independent FC3 comparison against the pinned Hub `FrameDriver`.

pub mod fixtures {
    use banto_driver_api::{
        DataType, DriverBase, FatalError, FrameDriver, ItemError, RawValue, Settings, TagKind,
        TagSpec,
    };
    use banto_driver_modbus::ModbusDriver;
    use banto_modbus_read_core::{ReadError, ReadHoldingU16, MAX_RESPONSE_LEN};
    #[cfg(test)]
    use banto_modbus_read_core::{ReadHoldingU32, RequestError, WordOrder};

    #[derive(Debug, PartialEq, Eq)]
    pub enum Outcome<T = u16> {
        Value(T),
        Device(u32),
        Framing,
        Protocol,
    }

    pub struct Fixture {
        pub name: &'static str,
        pub response: &'static [u8],
        pub expected: Outcome,
        /// The native PDU parser accepts extra bytes after the declared data.
        pub native_accepts_extra: bool,
    }

    pub const FIXTURES: &[Fixture] = &[
        Fixture {
            name: "value",
            response: &[0, 0, 0, 0, 0, 5, 1, 3, 2, 0, 42],
            expected: Outcome::Value(42),
            native_accepts_extra: false,
        },
        Fixture {
            name: "max_u16",
            response: &[0, 0, 0, 0, 0, 5, 1, 3, 2, 255, 255],
            expected: Outcome::Value(65535),
            native_accepts_extra: false,
        },
        Fixture {
            name: "exception",
            response: &[0, 0, 0, 0, 0, 3, 1, 0x83, 2],
            expected: Outcome::Device(2),
            native_accepts_extra: false,
        },
        Fixture {
            name: "wrong_tid",
            response: &[0, 1, 0, 0, 0, 5, 1, 3, 2, 0, 42],
            expected: Outcome::Protocol,
            native_accepts_extra: false,
        },
        Fixture {
            name: "wrong_unit",
            response: &[0, 0, 0, 0, 0, 5, 2, 3, 2, 0, 42],
            expected: Outcome::Protocol,
            native_accepts_extra: false,
        },
        Fixture {
            name: "bad_protocol_id",
            response: &[0, 0, 0, 1, 0, 5, 1, 3, 2, 0, 42],
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        Fixture {
            name: "short_header",
            response: &[0, 0, 0, 0, 0, 5],
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        Fixture {
            name: "bad_mbap_length",
            response: &[0, 0, 0, 0, 0, 6, 1, 3, 2, 0, 42],
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        Fixture {
            name: "wrong_byte_count",
            response: &[0, 0, 0, 0, 0, 5, 1, 3, 1, 0, 42],
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        Fixture {
            name: "truncated_data",
            response: &[0, 0, 0, 0, 0, 4, 1, 3, 2, 0],
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        Fixture {
            name: "extra_normal_pdu",
            response: &[0, 0, 0, 0, 0, 6, 1, 3, 2, 0, 42, 255],
            expected: Outcome::Value(42),
            native_accepts_extra: true,
        },
        Fixture {
            name: "extra_exception_pdu",
            response: &[0, 0, 0, 0, 0, 4, 1, 0x83, 2, 255],
            expected: Outcome::Device(2),
            native_accepts_extra: true,
        },
    ];

    #[cfg(test)]
    pub struct U32Fixture {
        pub name: &'static str,
        pub response: &'static [u8],
        /// None exercises native `create_connection` with no word_order key.
        pub word_order: Option<&'static str>,
        pub expected: Outcome<u32>,
        pub native_accepts_extra: bool,
    }

    #[cfg(test)]
    pub const U32_FIXTURES: &[U32Fixture] = &[
        U32Fixture {
            name: "zero_default",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0, 0, 0, 0],
            word_order: None,
            expected: Outcome::Value(0),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "max_u32",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 4, 255, 255, 255, 255],
            word_order: Some("high_low"),
            expected: Outcome::Value(u32::MAX),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "asymmetric_high_low",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd],
            word_order: Some("high_low"),
            expected: Outcome::Value(0x1234_abcd),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "asymmetric_low_high",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd],
            word_order: Some("low_high"),
            expected: Outcome::Value(0xabcd_1234),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "asymmetric_default",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd],
            word_order: None,
            expected: Outcome::Value(0xabcd_1234),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "exception",
            response: &[0, 0, 0, 0, 0, 3, 1, 0x83, 2],
            word_order: None,
            expected: Outcome::Device(2),
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "wrong_tid",
            response: &[0, 1, 0, 0, 0, 7, 1, 3, 4, 0, 1, 0, 2],
            word_order: None,
            expected: Outcome::Protocol,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "wrong_unit",
            response: &[0, 0, 0, 0, 0, 7, 2, 3, 4, 0, 1, 0, 2],
            word_order: None,
            expected: Outcome::Protocol,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "bad_protocol_id",
            response: &[0, 0, 0, 1, 0, 7, 1, 3, 4, 0, 1, 0, 2],
            word_order: None,
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "short_header",
            response: &[0, 0, 0, 0, 0, 7],
            word_order: None,
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "bad_mbap_length",
            response: &[0, 0, 0, 0, 0, 8, 1, 3, 4, 0, 1, 0, 2],
            word_order: None,
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "wrong_byte_count",
            response: &[0, 0, 0, 0, 0, 7, 1, 3, 2, 0, 1, 0, 2],
            word_order: None,
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "truncated_data",
            response: &[0, 0, 0, 0, 0, 6, 1, 3, 4, 0, 1, 0],
            word_order: None,
            expected: Outcome::Framing,
            native_accepts_extra: false,
        },
        U32Fixture {
            name: "extra_normal_pdu",
            response: &[0, 0, 0, 0, 0, 8, 1, 3, 4, 0, 1, 0, 2, 255],
            word_order: None,
            expected: Outcome::Value(0x0002_0001),
            native_accepts_extra: true,
        },
        U32Fixture {
            name: "extra_exception_pdu",
            response: &[0, 0, 0, 0, 0, 4, 1, 0x83, 2, 255],
            word_order: None,
            expected: Outcome::Device(2),
            native_accepts_extra: true,
        },
    ];

    pub fn native(response: &[u8]) -> Outcome {
        let driver = ModbusDriver;
        let kind = TagKind::numeric(DataType::U16);
        let address = driver.parse_address("40001", &kind).unwrap();
        let spec = TagSpec::new(address, kind);
        let specs = [spec];
        let mut conn = driver.create_connection(&Settings::new()).unwrap();
        let planned = conn.plan_read(&specs).unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].covers, [0]);
        assert_eq!(planned[0].step.max_len as usize, MAX_RESPONSE_LEN);
        let core = ReadHoldingU16 {
            transaction_id: 0,
            unit_id: 1,
            offset: 0,
        };
        assert_eq!(planned[0].step.send, core.request());
        match conn.decode_read(&planned[0], &specs, response) {
            Ok(values) => match values.as_slice() {
                [Ok(RawValue::Uint(value))] => Outcome::Value((*value).try_into().unwrap()),
                [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                other => panic!("unexpected native result: {other:?}"),
            },
            Err(FatalError::Framing(_)) => Outcome::Framing,
            Err(FatalError::Protocol(_)) => Outcome::Protocol,
            other => panic!("unexpected native result: {other:?}"),
        }
    }

    pub fn core(response: &[u8]) -> Outcome {
        let read = ReadHoldingU16 {
            transaction_id: 0,
            unit_id: 1,
            offset: 0,
        };
        match read.decode(response) {
            Ok(value) => Outcome::Value(value),
            Err(ReadError::Device(code)) => Outcome::Device(code.into()),
            Err(ReadError::Framing) => Outcome::Framing,
            Err(ReadError::Protocol) => Outcome::Protocol,
        }
    }

    #[cfg(test)]
    fn read_u32(offset: u16, word_order: WordOrder) -> ReadHoldingU32 {
        ReadHoldingU32 {
            transaction_id: 0,
            unit_id: 1,
            offset,
            word_order,
        }
    }

    #[cfg(test)]
    fn native_u32(
        response: &[u8],
        order: Option<&str>,
        address: &str,
        offset: u16,
    ) -> Outcome<u32> {
        let driver = ModbusDriver;
        let kind = TagKind::numeric(DataType::U32);
        let spec = TagSpec::new(driver.parse_address(address, &kind).unwrap(), kind);
        let specs = [spec];
        let settings = match order {
            Some(order) => Settings::from_pairs([("word_order", order)]).unwrap(),
            None => Settings::new(),
        };
        let mut conn = driver.create_connection(&settings).unwrap();
        let planned = conn.plan_read(&specs).unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].covers, [0]);
        assert_eq!(planned[0].step.max_len as usize, MAX_RESPONSE_LEN);
        assert_eq!(
            planned[0].step.send,
            read_u32(offset, WordOrder::default()).request().unwrap()
        );
        match conn.decode_read(&planned[0], &specs, response) {
            Ok(values) => match values.as_slice() {
                [Ok(RawValue::Uint(value))] => Outcome::Value((*value).try_into().unwrap()),
                [Err(ItemError::Device(error))] => Outcome::Device(error.code),
                other => panic!("unexpected native U32 result: {other:?}"),
            },
            Err(FatalError::Framing(_)) => Outcome::Framing,
            Err(FatalError::Protocol(_)) => Outcome::Protocol,
            other => panic!("unexpected native U32 result: {other:?}"),
        }
    }

    #[cfg(test)]
    fn core_u32(response: &[u8], order: Option<&str>, offset: u16) -> Outcome<u32> {
        let word_order = order
            .map(|value| WordOrder::parse(value).expect("supported native order"))
            .unwrap_or_default();
        match read_u32(offset, word_order).decode(response) {
            Ok(value) => Outcome::Value(value),
            Err(ReadError::Device(code)) => Outcome::Device(code.into()),
            Err(ReadError::Framing) => Outcome::Framing,
            Err(ReadError::Protocol) => Outcome::Protocol,
        }
    }

    #[test]
    fn fc3_single_register_shared_fixtures() {
        for fixture in FIXTURES {
            let native = native(fixture.response);
            let core = core(fixture.response);
            assert_eq!(native, fixture.expected, "native: {}", fixture.name);
            if fixture.native_accepts_extra {
                assert_eq!(core, Outcome::Framing, "core: {}", fixture.name);
                assert_ne!(core, native, "intentional difference: {}", fixture.name);
            } else {
                assert_eq!(core, native, "parity: {}", fixture.name);
            }
        }
    }

    #[test]
    fn fc3_single_u32_native_core_fixtures() {
        for fixture in U32_FIXTURES {
            let native = native_u32(fixture.response, fixture.word_order, "40001", 0);
            let core = core_u32(fixture.response, fixture.word_order, 0);
            assert_eq!(native, fixture.expected, "native: {}", fixture.name);
            if fixture.native_accepts_extra {
                assert_eq!(core, Outcome::Framing, "core: {}", fixture.name);
                assert_ne!(core, native, "intentional difference: {}", fixture.name);
            } else {
                assert_eq!(core, native, "parity: {}", fixture.name);
            }
        }
    }

    #[test]
    fn u32_last_register_and_order_defaults_match_native() {
        let kind = TagKind::numeric(DataType::U32);
        let driver = ModbusDriver;
        assert_eq!(
            driver
                .settings_schema()
                .field("word_order")
                .and_then(|field| field.default.clone()),
            Some(banto_driver_api::SettingValue::Text("high_low".into()))
        );
        assert!(driver.parse_address("465535", &kind).is_ok());
        assert!(driver.parse_address("465536", &kind).is_err());
        assert_eq!(
            read_u32(65_535, WordOrder::default()).request(),
            Err(RequestError::AddressRange)
        );
        let response = &[0, 0, 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd];
        assert_eq!(
            native_u32(response, None, "465535", 65_534),
            core_u32(response, None, 65_534)
        );
        // The native connection fallback is low_high, even though its form
        // schema advertises high_low as the default choice.
        assert_eq!(core_u32(response, None, 0), Outcome::Value(0xabcd_1234));
        assert_eq!(
            native_u32(response, None, "40001", 0),
            native_u32(response, Some("low_high"), "40001", 0)
        );
        assert_eq!(
            native_u32(response, Some("high_low"), "40001", 0),
            Outcome::Value(0x1234_abcd)
        );
    }

    #[test]
    fn native_probe_and_tcp_gap() {
        let driver = ModbusDriver;
        assert_eq!(driver.descriptor().settings_version, 1);
        assert_eq!(
            driver.probe(),
            Some(("40001".into(), TagKind::numeric(DataType::U16)))
        );
        let conn = driver.create_connection(&Settings::new()).unwrap();
        assert!(conn.inter_frame_gap().is_zero());
    }
}
