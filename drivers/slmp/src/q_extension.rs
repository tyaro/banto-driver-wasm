//! Conditional Q SD282-289 reads. M/B pairs give complete configured counts;
//! fast D/W pairs give internal counts only. No totals are synthesized.
use crate::{Device, ModelFamily, PointCount, ReadSpan, TypeName};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QExtensionArea {
    MTotal,
    BTotal,
    FastDInternal,
    FastWInternal,
}

/// Evidence for the same CPU, route, and acquisition generation as the SD read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QExtensionEvidence {
    pub route_confirmed: bool,
    /// CPU version applicability checked independently of 0101. For M/B,
    /// this includes QnU bit-device-extension function version B support.
    pub version_compatible: Option<bool>,
    /// First five digits of the CPU serial, needed for QnU SD286-289.
    pub serial_first_five: Option<u32>,
}

/// Plan one two-word SD read only for the documented model and version.
/// QnUDPV models remain unsupported by the current 0101 classifier.
pub fn plan_q_extension(
    model: &TypeName,
    area: QExtensionArea,
    evidence: QExtensionEvidence,
) -> Option<ReadSpan> {
    if model.family != ModelFamily::StandardQ
        || !evidence.route_confirmed
        || evidence.version_compatible != Some(true)
    {
        return None;
    }
    let start = match area {
        QExtensionArea::MTotal | QExtensionArea::BTotal => {
            if !is_qnu(model.code) || evidence.serial_first_five? < 10042 {
                return None;
            }
            if area == QExtensionArea::MTotal {
                286
            } else {
                288
            }
        }
        QExtensionArea::FastDInternal | QExtensionArea::FastWInternal => {
            if !is_recognized_qnudv(model) {
                return None;
            }
            if area == QExtensionArea::FastDInternal {
                282
            } else {
                284
            }
        }
    };
    Some(ReadSpan {
        device: Device::SD,
        start,
        count: 2,
    })
}

/// Decode the named field only. SD286/288 are complete M/B counts, while
/// SD282/284 are internal D/W counts. Never add an old saturated word here.
/// A missing or partial response stays unknown; no blanket 32K ceiling is used.
pub fn decode_q_extension_count(
    model: &TypeName,
    area: QExtensionArea,
    evidence: QExtensionEvidence,
    words: Option<&[u16]>,
) -> PointCount {
    if plan_q_extension(model, area, evidence).is_none() {
        return PointCount::Unknown;
    }
    match words {
        Some([low, high]) => PointCount::Exact(u32::from(*low) | (u32::from(*high) << 16)),
        _ => PointCount::Unknown,
    }
}

fn is_qnu(code: u16) -> bool {
    matches!(code, 0x0260..=0x0263 | 0x0266..=0x026e | 0x0366..=0x0368 | 0x036a | 0x036c)
}

fn is_recognized_qnudv(model: &TypeName) -> bool {
    matches!(
        (model.name.as_str(), model.code),
        ("Q03UDVCPU", 0x0366)
            | ("Q04UDVCPU", 0x0367)
            | ("Q06UDVCPU", 0x0368)
            | ("Q13UDVCPU", 0x036a)
            | ("Q26UDVCPU", 0x036c)
    )
}
