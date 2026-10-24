//! Independent FC3 comparison against the pinned Hub `FrameDriver`.

#[cfg(test)]
mod tests {
    use banto_driver_api::{
        DataType, DriverBase, FatalError, FrameDriver, ItemError, RawValue, Settings, TagKind,
        TagSpec,
    };
    use banto_driver_modbus::ModbusDriver;
    use banto_modbus_read_core::{ReadError, ReadHoldingU16, MAX_RESPONSE_LEN};

    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        Value(u16),
        Device(u32),
        Framing,
        Protocol,
    }

    struct Fixture {
        name: &'static str,
        response: &'static [u8],
        expected: Outcome,
        /// The native PDU parser accepts extra bytes after the declared data.
        native_accepts_extra: bool,
    }

    const FIXTURES: &[Fixture] = &[
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

    fn native(response: &[u8]) -> Outcome {
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

    fn core(response: &[u8]) -> Outcome {
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
