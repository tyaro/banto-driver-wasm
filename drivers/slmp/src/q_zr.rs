//! Conditional QnU ZR point-count read. No file-capacity or D/W-total
//! inference is performed here.
use crate::{Device, ModelFamily, PointCount, ReadSpan, TypeName};
use banto_register_codec::{decode_u32, WordOrder};

/// Evidence provided by a later connection workflow for this exact CPU,
/// firmware, route, and acquisition generation. Unknown is the default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QZrEvidence {
    pub route_confirmed: bool,
    pub firmware_compatible: Option<bool>,
    /// True only when extended D or extended W has at least 1K points set.
    pub extended_d_or_w_at_least_1k: Option<bool>,
}

/// Plan SD306–307 only when all applicable QnU conditions have been verified.
/// Q00UJCPU and other series have no plan. This does not execute I/O.
pub fn plan_q_zr(model: &TypeName, evidence: QZrEvidence) -> Option<ReadSpan> {
    if model.family != ModelFamily::StandardQ
        || !supports_qnu_zr_field(model.code)
        || !evidence.route_confirmed
        || evidence.firmware_compatible != Some(true)
        || evidence.extended_d_or_w_at_least_1k != Some(true)
    {
        return None;
    }
    Some(ReadSpan {
        device: Device::SD,
        start: 306,
        count: 2,
    })
}

/// Decode ZR points from an exact two-word response for a proven plan.
/// SD647 is a 1K-point truncated file capacity shared with extended D/W and
/// must never substitute for these exact ZR words.
pub fn decode_q_zr_count(
    model: &TypeName,
    evidence: QZrEvidence,
    words: Option<&[u16]>,
) -> PointCount {
    if plan_q_zr(model, evidence).is_none() {
        return PointCount::Unknown;
    }
    match words {
        Some([low, high]) => PointCount::Exact(decode_u32([*low, *high], WordOrder::LowHigh)),
        _ => PointCount::Unknown,
    }
}

fn supports_qnu_zr_field(code: u16) -> bool {
    // Q CPU Appendix 3 identifies this map for QnU, excluding Q00UJCPU.
    // The model-code table is already checked by decode_type_name.
    matches!(
        code,
        0x0261..=0x026e | 0x0366 | 0x0367 | 0x0368 | 0x036a | 0x036c
    )
}
