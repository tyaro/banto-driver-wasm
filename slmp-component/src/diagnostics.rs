//! Optional, bounded diagnostics over the same connection as read and write.
//! The host serializes calls and discards this resource after an abandoned send.
use super::*;

const MAX_DIAGNOSTIC_TAGS: usize = 256;

pub(super) struct PendingDiagnostic {
    specs: Vec<TagSpec>,
    route: Route,
    span: ReadSpan,
    model: TypeName,
}

fn same_specs(left: &[TagSpec], right: &[TagSpec]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.address == b.address && a.data_type == b.data_type && a.string_words == b.string_words
        })
}

fn requested_bit_count(specs: &[TagSpec]) -> bool {
    specs.iter().any(|spec| {
        parse_read_address(&spec.address, &spec.data_type, spec.string_words).is_ok_and(
            |(device, _, _)| matches!(device, Device::M | Device::Y | Device::L | Device::B),
        )
    })
}

fn bit_count_span(model: &TypeName) -> Option<ReadSpan> {
    match model.family {
        ModelFamily::StandardQ => Some(ReadSpan {
            device: Device::SD,
            start: 290,
            count: 5,
        }),
        ModelFamily::StandardR => Some(ReadSpan {
            device: Device::SD,
            start: 262,
            count: 14,
        }),
        _ => None,
    }
}

fn update_bit_counts(conn: &SlmpConnection, model: &TypeName, words: &[u16]) {
    let evidence = BasicEvidence {
        route_confirmed: true,
        r_firmware: None,
    };
    let counts = if model.family == ModelFamily::StandardQ {
        [
            basic_sd_count(model, BasicArea::M, evidence, words.get(2..3)),
            basic_sd_count(model, BasicArea::Y, evidence, words.get(1..2)),
            basic_sd_count(model, BasicArea::L, evidence, words.get(3..4)),
            basic_sd_count(model, BasicArea::B, evidence, words.get(4..5)),
        ]
    } else {
        [
            basic_sd_count(model, BasicArea::M, evidence, words.get(2..4)),
            basic_sd_count(model, BasicArea::Y, evidence, words.get(0..2)),
            basic_sd_count(model, BasicArea::L, evidence, words.get(12..14)),
            basic_sd_count(model, BasicArea::B, evidence, words.get(4..6)),
        ]
    };
    conn.bit_counts.set(counts);
}

fn family_name(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::StandardQ => "standard-q",
        ModelFamily::StandardR => "standard-r",
        ModelFamily::Q25PrhWireOnly => "q25prh-wire-only",
        ModelFamily::Unknown => "unknown",
    }
}

fn bound(count: PointCount, unknown_reason: &str) -> diagnostics_v1::CountBound {
    match count {
        PointCount::Exact(value) => diagnostics_v1::CountBound::Exact(value),
        PointCount::AtLeast(value) => diagnostics_v1::CountBound::AtLeast(value),
        PointCount::Unknown => diagnostics_v1::CountBound::Unknown(unknown_reason.into()),
    }
}

fn area_count(
    area: &str,
    unit: &str,
    scope: &str,
    source: &str,
    count: PointCount,
    reason: &str,
) -> diagnostics_v1::AreaCount {
    diagnostics_v1::AreaCount {
        area: area.into(),
        unit: unit.into(),
        scope: scope.into(),
        source: source.into(),
        bound: bound(count, reason),
    }
}

fn area_counts(
    conn: &SlmpConnection,
    model: &TypeName,
    d_count: PointCount,
) -> Vec<diagnostics_v1::AreaCount> {
    let family = model.family;
    let (d_scope, d_source, d_value) = match family {
        ModelFamily::StandardR => ("total", "SD280-281", d_count),
        ModelFamily::StandardQ => ("internal", "SD302", d_count),
        ModelFamily::Q25PrhWireOnly => (
            "internal-only; general D range unknown",
            "SD302",
            conn.q25prh_internal_d_count.get(),
        ),
        ModelFamily::Unknown => ("unknown", "", PointCount::Unknown),
    };
    let (w_scope, w_source) = match family {
        ModelFamily::StandardR => ("total", "SD282-283"),
        ModelFamily::StandardQ | ModelFamily::Q25PrhWireOnly => ("internal", "SD303"),
        ModelFamily::Unknown => ("unknown", ""),
    };
    let dw_reason = if conn.dw_sd_refused.get() {
        "sd-refused"
    } else {
        "sd-unavailable-or-inapplicable"
    };
    let mut result = vec![
        area_count("D", "word", d_scope, d_source, d_value, dw_reason),
        area_count(
            "W",
            "word",
            w_scope,
            w_source,
            conn.w_count.get(),
            dw_reason,
        ),
    ];
    let bit_reason = if conn.bit_sd_refused.get() {
        "sd-refused"
    } else if !conn.bit_sd_attempted.get() {
        "sd-not-acquired"
    } else {
        "sd-unavailable-or-inapplicable"
    };
    let bit = conn.bit_counts.get();
    let (sources, scope) = match family {
        ModelFamily::StandardQ => (["SD292", "SD291", "SD293", "SD294"], "configured-prefix"),
        ModelFamily::StandardR => (
            ["SD264-265", "SD262-263", "SD274-275", "SD266-267"],
            "total",
        ),
        _ => (["", "", "", ""], "unknown"),
    };
    for (index, area) in ["M", "Y", "L", "B"].iter().enumerate() {
        result.push(area_count(
            area,
            "bit",
            scope,
            sources[index],
            bit[index],
            bit_reason,
        ));
    }
    result
}

// Q bit SD291/Y and SD293/L are exact bounds. Q M/B are also exact
// unless the legacy 32768 field saturated, which basic_sd_count marks AtLeast.
// D/W SD302/303 are internal prefixes and must keep their separate semantics.
fn assess_tag_span(
    model: &TypeName,
    device: Device,
    count: PointCount,
    number: u32,
    points: u16,
) -> RangeAssessment {
    if model.family == ModelFamily::StandardQ
        && matches!(device, Device::M | Device::Y | Device::L | Device::B)
    {
        let Some(last) = number.checked_add(u32::from(points - 1)) else {
            return RangeAssessment::Unknown;
        };
        return match count {
            PointCount::Exact(limit) if last < limit => RangeAssessment::VerifiedInside,
            PointCount::Exact(_) => RangeAssessment::VerifiedOutside,
            PointCount::AtLeast(prefix) if last < prefix => RangeAssessment::VerifiedInside,
            PointCount::AtLeast(_) | PointCount::Unknown => RangeAssessment::Unknown,
        };
    }
    assess_d_span(model, count, number, points)
}

fn tag_diagnostic(
    conn: &SlmpConnection,
    model: Option<&TypeName>,
    d_count: PointCount,
    spec: &TagSpec,
) -> diagnostics_v1::TagDiagnostic {
    use diagnostics_v1::TagVerdict;
    let verdict = match parse_read_address(&spec.address, &spec.data_type, spec.string_words) {
        Err(reason) => TagVerdict::Invalid(reason),
        Ok((_, number, kind)) if number.checked_add(u32::from(kind.points() - 1)).is_none() => {
            TagVerdict::Invalid("address-span-overflow".into())
        }
        Ok((device, number, kind)) => match model {
            None => TagVerdict::Unknown("0101-model-unavailable".into()),
            Some(model) if model.family == ModelFamily::Unknown => {
                TagVerdict::Unknown("unsupported-model-name-code-pair".into())
            }
            Some(model) => {
                let count = match device {
                    Device::M | Device::Y | Device::L | Device::B => write_bit_count(conn, device),
                    _ => read_count(device, d_count, conn.w_count.get()),
                };
                let range = if model.family == ModelFamily::Q25PrhWireOnly && device == Device::W {
                    if count_guarantees_span(count, number, kind.points()) {
                        RangeAssessment::VerifiedInside
                    } else {
                        RangeAssessment::Unknown
                    }
                } else {
                    assess_tag_span(model, device, count, number, kind.points())
                };
                match range {
                    RangeAssessment::VerifiedOutside => {
                        TagVerdict::Outside("complete-configured-count".into())
                    }
                    RangeAssessment::VerifiedInside => {
                        if checked_read_span(spec, model, d_count, conn.w_count.get()).is_ok() {
                            TagVerdict::Inside("same-connection-sd-guaranteed-span".into())
                        } else {
                            TagVerdict::Unsupported("guest-read-path-unavailable".into())
                        }
                    }
                    RangeAssessment::Unknown => {
                        if device == Device::W
                            && matches!(
                                model.family,
                                ModelFamily::StandardQ | ModelFamily::Q25PrhWireOnly
                            )
                        {
                            TagVerdict::Unknown(if conn.dw_sd_refused.get() {
                                "sd-refused; guest-read-path-unavailable".into()
                            } else {
                                "internal-w-prefix-unproven; guest-read-path-unavailable".into()
                            })
                        } else if checked_read_span(spec, model, d_count, conn.w_count.get())
                            .is_err()
                        {
                            TagVerdict::Unsupported(
                                "guest-read-path-unavailable; range-not-proven".into(),
                            )
                        } else if model.family == ModelFamily::Q25PrhWireOnly {
                            TagVerdict::Unknown("q25prh-general-range-unconfirmed".into())
                        } else if (conn.bit_sd_refused.get()
                            && matches!(device, Device::M | Device::Y | Device::L | Device::B))
                            || (conn.dw_sd_refused.get() && matches!(device, Device::D | Device::W))
                        {
                            TagVerdict::Unknown("sd-refused".into())
                        } else {
                            TagVerdict::Unknown("configured-range-unproven".into())
                        }
                    }
                }
            }
        },
    };
    diagnostics_v1::TagDiagnostic {
        address: spec.address.clone(),
        verdict,
    }
}

fn report(conn: &SlmpConnection, specs: &[TagSpec]) -> diagnostics_v1::Report {
    let (model, d_count) = match &*conn.phase.borrow() {
        Phase::Ready { model, count } => (Some(model.clone()), *count),
        Phase::NoRange(model) => (model.clone(), PointCount::Unknown),
        _ => (None, PointCount::Unknown),
    };
    let model_reason = match model.as_ref() {
        None => Some("0101-refused-or-unavailable".into()),
        Some(model) if model.family == ModelFamily::Unknown => {
            Some("unsupported-model-name-code-pair".into())
        }
        _ => None,
    };
    let counts = model
        .as_ref()
        .map_or_else(Vec::new, |model| area_counts(conn, model, d_count));
    let tags = specs
        .iter()
        .map(|spec| tag_diagnostic(conn, model.as_ref(), d_count, spec))
        .collect();
    diagnostics_v1::Report {
        model: model.map(|model| diagnostics_v1::ModelInfo {
            raw_name: model.raw_name.to_vec(),
            name: model.name,
            code: model.code,
            family: family_name(model.family).into(),
        }),
        model_reason,
        counts,
        tags,
    }
}

impl diagnostics_v1::Guest for Component {
    fn diagnose_next(
        conn: diagnostics_v1::ConnectionBorrow<'_>,
        specs: Vec<TagSpec>,
        prev_response: Option<Vec<u8>>,
    ) -> Result<diagnostics_v1::Progress, FatalError> {
        let conn = conn.get::<SlmpConnection>();
        if conn.diagnostic_poisoned.get() {
            return Err(FatalError::Protocol(
                "diagnostic outcome is uncertain; reconnect".into(),
            ));
        }
        if specs.len() > MAX_DIAGNOSTIC_TAGS {
            return Err(FatalError::InvalidConfig("too many diagnostic tags".into()));
        }
        if let Some(frame) = prev_response {
            // Consume before parsing. An invalid response poisons this connection.
            conn.diagnostic_poisoned.set(true);
            let pending = conn.pending_diagnostic.borrow_mut().take().ok_or_else(|| {
                FatalError::Protocol("diagnostic response without pending request".into())
            })?;
            if !same_specs(&specs, &pending.specs) {
                return Err(FatalError::Protocol(
                    "diagnostic tags changed after send".into(),
                ));
            }
            match decode_read(pending.route, pending.span, &frame) {
                Ok(Values::Words(words)) => update_bit_counts(conn, &pending.model, &words),
                Err(ResponseError::EndCode(_)) => conn.bit_sd_refused.set(true),
                Err(error) => return Err(response_error(error)),
                Ok(Values::Bits(_)) => return Err(FatalError::Protocol("SD returned bits".into())),
            }
            conn.diagnostic_poisoned.set(false);
            return Ok(diagnostics_v1::Progress::Done(report(conn, &specs)));
        }
        if conn.pending_diagnostic.borrow().is_some() {
            conn.diagnostic_poisoned.set(true);
            return Err(FatalError::Protocol(
                "diagnostic response is missing; reconnect".into(),
            ));
        }
        if conn.pending_write.borrow().is_some()
            || conn.write_poisoned.get()
            || !conn.pending.borrow().is_empty()
        {
            return Err(FatalError::Protocol(
                "another response is pending on this connection".into(),
            ));
        }
        let phase = conn.phase.borrow();
        let model = match &*phase {
            Phase::Ready { model, .. } => Some(model.clone()),
            Phase::NoRange(_) => None,
            _ => return Err(FatalError::Protocol("handshake is incomplete".into())),
        };
        drop(phase);
        if let Some(model) = model {
            if requested_bit_count(&specs) && !conn.bit_sd_attempted.get() {
                if let Some(span) = bit_count_span(&model) {
                    let route = conn.route();
                    let series = known_series(&model).expect("supported Q/R model");
                    let send = encode_read(route, series, span)
                        .map_err(|_| FatalError::Protocol("invalid diagnostic SD span".into()))?;
                    conn.bit_sd_attempted.set(true);
                    *conn.pending_diagnostic.borrow_mut() = Some(PendingDiagnostic {
                        specs,
                        route,
                        span,
                        model,
                    });
                    return Ok(diagnostics_v1::Progress::Send(frame_step(send)));
                }
            }
        }
        Ok(diagnostics_v1::Progress::Done(report(conn, &specs)))
    }
}
