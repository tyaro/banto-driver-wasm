//! Boundary matrix for diagnostics-v1 range verdicts. Each case places the
//! last inside point and the first point past a controlled SD count, so a
//! wrong radix, bit/word unit, word span or low/high decode moves a verdict.
//! The fixtures use controlled responses only; no PLC is contacted.
use wasmtime::component::{Component, Linker, ResourceAny};
use wasmtime::{Engine, Store};

mod host {
    wasmtime::component::bindgen!({ path: "../wit", world: "driver-with-diagnostics-v1" });
}
use host::banto::driver::types::{Setting, TagSpec};
use host::exports::banto::driver::diagnostics_v1::{Progress, Report, TagVerdict};

const INSIDE: &str = "same-connection-sd-guaranteed-span";
const OUTSIDE: &str = "complete-configured-count";
const UNPROVEN: &str = "configured-range-unproven";
const NO_PATH: &str = "guest-read-path-unavailable; range-not-proven";
const Q_W_PREFIX: &str = "internal-w-prefix-unproven; guest-read-path-unavailable";
const Q25PRH: &str = "q25prh-general-range-unconfirmed";

/// (address, data type, verdict kind, reason)
type Case = (&'static str, &'static str, &'static str, &'static str);

fn response(serial: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![0xd4, 0];
    frame.extend_from_slice(&serial.to_le_bytes());
    frame.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 3, 0]);
    frame.extend_from_slice(&(u16::try_from(body.len() + 2).unwrap()).to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
}

fn words_response(serial: u16, words: Option<&[u16]>) -> Vec<u8> {
    match words {
        Some(words) => {
            let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            response(serial, &bytes)
        }
        None => {
            let mut refused = response(serial, &[]);
            refused[13..15].copy_from_slice(&0xc05bu16.to_le_bytes());
            refused
        }
    }
}

fn model_response(name: &str, code: u16) -> Vec<u8> {
    let mut data = [b' '; 18];
    data[..name.len()].copy_from_slice(name.as_bytes());
    data[16..].copy_from_slice(&code.to_le_bytes());
    response(1, &data)
}

fn low_high(count: u32) -> [u16; 2] {
    [count as u16, (count >> 16) as u16]
}

struct Fixture {
    store: Store<()>,
    guest: host::DriverWithDiagnosticsV1,
}

impl Fixture {
    fn new() -> wasmtime::Result<Self> {
        let path = std::env::var("BANTO_SLMP_COMPONENT_WASM")?;
        let engine = Engine::default();
        let component = Component::from_file(&engine, path)?;
        let mut store = Store::new(&engine, ());
        let guest = host::DriverWithDiagnosticsV1::instantiate(
            &mut store,
            &component,
            &Linker::new(&engine),
        )?;
        Ok(Self { store, guest })
    }

    /// Plan on a fresh connection that has D/W evidence only, then run one
    /// diagnostic attempt and plan again on a second fresh connection.
    /// `bit_sd` answers the bounded bit-count request when the guest sends
    /// one; `None` inside answers it with a PLC refusal.
    fn diagnose(
        &mut self,
        model: (&str, u16),
        dw_sd: Option<&[u16]>,
        bit_sd: Option<Option<&[u16]>>,
        specs: &[TagSpec],
    ) -> wasmtime::Result<(Report, Vec<u32>, Vec<u32>)> {
        let conn = self.ready(model, dw_sd)?;
        let before = self.covers(conn, specs)?;
        conn.resource_drop(&mut self.store)?;
        let conn = self.ready(model, dw_sd)?;
        let diagnostics = self.guest.banto_driver_diagnostics_v1();
        let report = match diagnostics
            .call_diagnose_next(&mut self.store, conn, specs, None)?
            .expect("diagnose")
        {
            Progress::Done(report) => {
                assert!(bit_sd.is_none(), "{}: bit SD was not requested", model.0);
                report
            }
            Progress::Send(_) => {
                let reply = bit_sd.expect("unexpected bit SD request");
                let Progress::Done(report) = diagnostics
                    .call_diagnose_next(
                        &mut self.store,
                        conn,
                        specs,
                        Some(&words_response(3, reply)),
                    )?
                    .expect("bit SD result")
                else {
                    panic!("{}: one bit SD request is enough", model.0);
                };
                report
            }
        };
        let after = self.covers(conn, specs)?;
        conn.resource_drop(&mut self.store)?;
        Ok((report, before, after))
    }

    /// Run 0101 and the D/W SD handshake to Ready on a fresh connection.
    fn ready(
        &mut self,
        model: (&str, u16),
        dw_sd: Option<&[u16]>,
    ) -> wasmtime::Result<ResourceAny> {
        let settings =
            [("host", "fixture.invalid"), ("port", "5000")].map(|(key, value)| Setting {
                key: key.into(),
                value: value.into(),
            });
        let session = self.guest.banto_driver_session().connection();
        let conn: ResourceAny = session
            .call_create(&mut self.store, &settings)?
            .expect("create");
        session
            .call_handshake_next(&mut self.store, conn, None)?
            .expect("0101")
            .expect("0101 step");
        session
            .call_handshake_next(
                &mut self.store,
                conn,
                Some(&model_response(model.0, model.1)),
            )?
            .expect("D/W SD request")
            .expect("D/W SD step");
        assert!(session
            .call_handshake_next(&mut self.store, conn, Some(&words_response(2, dw_sd)))?
            .expect("ready")
            .is_none());
        Ok(conn)
    }

    fn covers(&mut self, conn: ResourceAny, specs: &[TagSpec]) -> wasmtime::Result<Vec<u32>> {
        Ok(self
            .guest
            .banto_driver_read()
            .call_plan(&mut self.store, conn, specs)?
            .expect("plan")
            .into_iter()
            .flat_map(|plan| plan.covers)
            .collect())
    }
}

fn verdict(verdict: &TagVerdict) -> (&'static str, &str) {
    match verdict {
        TagVerdict::Inside(reason) => ("inside", reason),
        TagVerdict::Outside(reason) => ("outside", reason),
        TagVerdict::Unknown(reason) => ("unknown", reason),
        TagVerdict::Invalid(reason) => ("invalid", reason),
        TagVerdict::Unsupported(reason) => ("unsupported", reason),
    }
}

fn run(
    fixture: &mut Fixture,
    model: (&str, u16),
    dw_sd: Option<&[u16]>,
    bit_sd: Option<Option<&[u16]>>,
    cases: &[Case],
) -> wasmtime::Result<()> {
    let specs: Vec<TagSpec> = cases
        .iter()
        .map(|(address, data_type, _, _)| TagSpec {
            address: (*address).into(),
            data_type: (*data_type).into(),
            string_words: None,
        })
        .collect();
    let (report, before, after) = fixture.diagnose(model, dw_sd, bit_sd, &specs)?;
    assert_eq!(report.tags.len(), cases.len());
    for (index, (tag, (address, data_type, kind, reason))) in
        report.tags.iter().zip(cases).enumerate()
    {
        assert_eq!(tag.address, *address);
        assert_eq!(
            verdict(&tag.verdict),
            (*kind, *reason),
            "{} {address}/{data_type}",
            model.0
        );
        // An inside verdict needs a guest read path; an unsupported or
        // outside tag is never planned. Handshake-only D/W evidence already
        // excludes D/W outside tags; a bit tag is excluded once the same
        // connection holds its exact SD count, and is readable before that.
        let index = index as u32;
        let planned = after.contains(&index);
        match *kind {
            "inside" => assert!(planned, "{} {address} inside but unplanned", model.0),
            "unsupported" | "outside" => {
                assert!(!planned, "{} {address} {kind} but planned", model.0)
            }
            // Missing, refused or prefix-only evidence keeps the explicit
            // one-tag read; only Q W beyond its SD303 prefix has no path.
            "unknown" => assert_eq!(
                planned,
                *reason != Q_W_PREFIX,
                "{} {address} unknown ({reason}) plan",
                model.0
            ),
            _ => {}
        }
        let dw = address.starts_with('D') || address.starts_with('W');
        if *kind == "outside" {
            assert_eq!(
                before.contains(&index),
                !dw,
                "{} {address} plan before bit SD evidence",
                model.0
            );
        }
    }
    Ok(())
}

#[test]
fn standard_r_exact_counts_bound_every_unit_radix_and_span() -> wasmtime::Result<()> {
    let mut fixture = Fixture::new()?;
    // SD280-283: D = 100 points, W = 0x50 points.
    let dw = [low_high(100), low_high(0x50)].concat();
    // SD262-275 low/high pairs: Y, M, B, SB, F, V (unused), L.
    // M = 70000 needs a nonzero high word; L = 0 means no configured points.
    let bit = [
        low_high(0x40),
        low_high(70_000),
        low_high(0x100),
        low_high(0x20),
        low_high(10),
        low_high(0),
        low_high(0),
    ]
    .concat();
    run(
        &mut fixture,
        ("R04CPU", 0x4800),
        Some(&dw),
        Some(Some(&bit)),
        &[
            // D is decimal; the whole 1-, 2- or 4-word span must fit.
            ("D99", "u16", "inside", INSIDE),
            ("D100", "u16", "outside", OUTSIDE),
            ("D98", "u32", "inside", INSIDE),
            ("D99", "i32", "outside", OUTSIDE),
            ("D98", "f32", "inside", INSIDE),
            ("D99", "f32", "outside", OUTSIDE),
            ("D96", "u64", "inside", INSIDE),
            ("D97", "i64", "outside", OUTSIDE),
            ("D96", "f64", "inside", INSIDE),
            ("D97", "f64", "outside", OUTSIDE),
            // A word bit occupies its whole D word.
            ("D99.F", "bit", "inside", INSIDE),
            ("D100.0", "bit", "outside", OUTSIDE),
            // W is hexadecimal: W4F is point 79 and W50 is point 80.
            ("W4F", "u16", "inside", INSIDE),
            ("W50", "u16", "outside", OUTSIDE),
            ("W4E", "u32", "inside", INSIDE),
            ("W4F", "u32", "outside", OUTSIDE),
            ("W4C", "u64", "inside", INSIDE),
            ("W4D", "f64", "outside", OUTSIDE),
            // A W word bit occupies its whole hexadecimal W word.
            ("W4F.F", "bit", "inside", INSIDE),
            ("W50.0", "bit", "outside", OUTSIDE),
            // Bit areas count bit points: M/F/L decimal, Y/B/SB hexadecimal.
            ("M69999", "bit", "inside", INSIDE),
            ("M70000", "bit", "outside", OUTSIDE),
            ("Y3F", "bit", "inside", INSIDE),
            ("Y40", "bit", "outside", OUTSIDE),
            ("BFF", "bit", "inside", INSIDE),
            ("B100", "bit", "outside", OUTSIDE),
            ("SB1F", "bit", "inside", INSIDE),
            ("SB20", "bit", "outside", OUTSIDE),
            ("F9", "bit", "inside", INSIDE),
            ("F10", "bit", "outside", OUTSIDE),
            ("L0", "bit", "outside", OUTSIDE),
            // No same-connection evidence is acquired for these R areas.
            ("X0", "bit", "unknown", UNPROVEN),
            ("SM0", "bit", "unknown", UNPROVEN),
            ("SD0", "u16", "unknown", UNPROVEN),
            ("SW0", "u16", "unknown", UNPROVEN),
            ("ZR0", "u32", "unknown", UNPROVEN),
            ("TN0", "u16", "unknown", UNPROVEN),
            ("STN0", "u16", "unknown", UNPROVEN),
            ("LTN0", "i32", "unknown", UNPROVEN),
            ("LTS0", "bit", "unknown", UNPROVEN),
            ("LCN0", "i32", "unknown", UNPROVEN),
            ("LCS0", "bit", "unknown", UNPROVEN),
            // Q-only retentive timer names have no R read path.
            ("SN0", "u16", "unsupported", NO_PATH),
        ],
    )
}

#[test]
fn standard_r_refused_bit_sd_keeps_bit_areas_unknown_and_dw_exact() -> wasmtime::Result<()> {
    let mut fixture = Fixture::new()?;
    let dw = [low_high(100), low_high(0x50)].concat();
    run(
        &mut fixture,
        ("R04CPU", 0x4800),
        Some(&dw),
        Some(None),
        &[
            ("D99", "u16", "inside", INSIDE),
            ("D100", "u16", "outside", OUTSIDE),
            ("W50", "u16", "outside", OUTSIDE),
            ("M0", "bit", "unknown", "sd-refused"),
            ("Y0", "bit", "unknown", "sd-refused"),
            ("L0", "bit", "unknown", "sd-refused"),
            ("B0", "bit", "unknown", "sd-refused"),
            ("SB0", "bit", "unknown", "sd-refused"),
            ("F0", "bit", "unknown", "sd-refused"),
            ("X0", "bit", "unknown", UNPROVEN),
        ],
    )
}

#[test]
fn standard_q_exact_bit_counts_and_internal_dw_prefixes() -> wasmtime::Result<()> {
    let mut fixture = Fixture::new()?;
    // SD302-303: internal D = 100, internal W = 0x50.
    let dw = [100, 0x50];
    // SD290-294: X, Y, M (saturated legacy field), L = 0, B.
    let bit = [0x20, 0x40, 32768, 0, 0x100];
    run(
        &mut fixture,
        ("Q02UCPU", 0x0263),
        Some(&dw),
        Some(Some(&bit)),
        &[
            // Internal prefixes prove inside only; extended D may follow.
            ("D99", "u16", "inside", INSIDE),
            ("D100", "u16", "unknown", UNPROVEN),
            ("D98", "u32", "inside", INSIDE),
            ("D99", "u32", "unknown", UNPROVEN),
            ("D96", "f64", "inside", INSIDE),
            ("D97", "f64", "unknown", UNPROVEN),
            ("D99.F", "bit", "inside", INSIDE),
            ("D100.0", "bit", "unknown", UNPROVEN),
            // Q W is read only inside the SD303 prefix.
            ("W4F", "u16", "inside", INSIDE),
            ("W50", "u16", "unknown", Q_W_PREFIX),
            ("W4C", "u64", "inside", INSIDE),
            ("W4D", "u64", "unknown", Q_W_PREFIX),
            ("W4F.F", "bit", "inside", INSIDE),
            ("W50.0", "bit", "unknown", Q_W_PREFIX),
            // Exact Q bit counts: X/Y/B hexadecimal, L decimal.
            ("X1F", "bit", "inside", INSIDE),
            ("X20", "bit", "outside", OUTSIDE),
            ("Y3F", "bit", "inside", INSIDE),
            ("Y40", "bit", "outside", OUTSIDE),
            ("BFF", "bit", "inside", INSIDE),
            ("B100", "bit", "outside", OUTSIDE),
            ("L0", "bit", "outside", OUTSIDE),
            // SD292 = 32768 proves only the first 32768 M points.
            ("M32767", "bit", "inside", INSIDE),
            ("M32768", "bit", "unknown", UNPROVEN),
            // This guest acquires no Q SB/F/timer/counter counts.
            ("SB0", "bit", "unknown", UNPROVEN),
            ("F0", "bit", "unknown", UNPROVEN),
            ("SN0", "u16", "unknown", UNPROVEN),
            ("SS0", "bit", "unknown", UNPROVEN),
            // R-only devices and Q ZR have no Q read path.
            ("STN0", "u16", "unsupported", NO_PATH),
            ("LTN0", "i32", "unsupported", NO_PATH),
            ("LCS0", "bit", "unsupported", NO_PATH),
            ("ZR0", "u16", "unsupported", NO_PATH),
        ],
    )
}

#[test]
fn standard_q_saturated_b_and_exact_m_keep_distinct_rules() -> wasmtime::Result<()> {
    let mut fixture = Fixture::new()?;
    let dw = [32768, 0];
    // SD290-294: X = 0, Y, M = 100, L = 50, B saturated at 32768.
    let bit = [0, 0x10, 100, 50, 32768];
    run(
        &mut fixture,
        ("Q02UCPU", 0x0263),
        Some(&dw),
        Some(Some(&bit)),
        &[
            ("D32767", "u16", "inside", INSIDE),
            ("D32768", "u16", "unknown", UNPROVEN),
            ("W0", "u16", "unknown", Q_W_PREFIX),
            ("W0.0", "bit", "unknown", Q_W_PREFIX),
            ("X0", "bit", "outside", OUTSIDE),
            ("YF", "bit", "inside", INSIDE),
            ("Y10", "bit", "outside", OUTSIDE),
            ("M99", "bit", "inside", INSIDE),
            ("M100", "bit", "outside", OUTSIDE),
            ("L49", "bit", "inside", INSIDE),
            ("L50", "bit", "outside", OUTSIDE),
            // B is hexadecimal: B7FFF is point 32767, B8000 point 32768.
            ("B7FFF", "bit", "inside", INSIDE),
            ("B8000", "bit", "unknown", UNPROVEN),
        ],
    )
}

#[test]
fn q25prh_keeps_general_ranges_unknown_beside_its_internal_w_prefix() -> wasmtime::Result<()> {
    let mut fixture = Fixture::new()?;
    run(
        &mut fixture,
        ("Q25PRHCPU", 0x004c),
        Some(&[100, 0x50]),
        None,
        &[
            ("D0", "u16", "unknown", Q25PRH),
            ("D99", "u16", "unknown", Q25PRH),
            ("D100", "u16", "unknown", Q25PRH),
            ("W4F", "u16", "inside", INSIDE),
            ("W4E", "u32", "inside", INSIDE),
            ("W4F", "u32", "unknown", Q_W_PREFIX),
            ("W50", "u16", "unknown", Q_W_PREFIX),
            ("W4F.F", "bit", "inside", INSIDE),
            ("W50.0", "bit", "unknown", Q_W_PREFIX),
            ("M0", "bit", "unknown", Q25PRH),
            ("X0", "bit", "unknown", Q25PRH),
            ("TN0", "u16", "unsupported", NO_PATH),
            ("ZR0", "u16", "unsupported", NO_PATH),
        ],
    )
}
