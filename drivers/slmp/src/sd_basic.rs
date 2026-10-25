//! Read-only SD point-count planning for the basic standard Q/R maps.
//! A decoded count is not a device address, a wire read limit, or a tag verdict.
use crate::{Device, ModelFamily, ReadSpan, TypeName};

/// Logical point-count field. Timer/counter fields mean configured points;
/// they do not select a bit/word contact, coil, or current-value wire code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicArea {
    X,
    Y,
    M,
    L,
    B,
    F,
    SB,
    V,
    S,
    T,
    ST,
    C,
    /// Complete D/W points on R; Q's full internal + extended result is later.
    D,
    W,
    /// Q SD302/303 only, never a full extended D/W limit.
    DInternal,
    WInternal,
    SW,
    LT,
    LST,
    LC,
    Z,
    LZ,
    ZR,
    RD,
}

/// Evidence that a later connection workflow must obtain independently.
/// `route_confirmed` never follows from the 0101 model name alone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BasicEvidence {
    pub route_confirmed: bool,
    /// R CPU firmware version, when independently verified.
    pub r_firmware: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointCount {
    Exact(u32),
    /// The old Q M/B/D/W word saturated at 32768; only a lower bound is known.
    AtLeast(u32),
    Unknown,
}

/// Return one 0401 word-unit SD read for a recognized standard model and
/// basic field. No I/O is performed. Unsupported/conditional Q extension
/// fields (including Q ZR and SD647) are deliberately absent from this stage.
pub fn plan_basic_sd(model: &TypeName, area: BasicArea) -> Option<ReadSpan> {
    let (start, count) = match model.family {
        ModelFamily::StandardQ => (q_sd(area)?, 1),
        ModelFamily::StandardR => r_sd(area)?,
        ModelFamily::Unknown => return None,
    };
    Some(ReadSpan {
        device: Device::SD,
        start: u32::from(start),
        count,
    })
}

/// Decode the words returned by the plan. Missing/mismatched data and
/// unconfirmed applicability remain unknown. A saturated Q legacy field
/// yields a lower bound, never an invented exact upper limit.
pub fn basic_sd_count(
    model: &TypeName,
    area: BasicArea,
    evidence: BasicEvidence,
    words: Option<&[u16]>,
) -> PointCount {
    let Some(plan) = plan_basic_sd(model, area) else {
        return PointCount::Unknown;
    };
    if !evidence.route_confirmed {
        return PointCount::Unknown;
    }
    if model.family == ModelFamily::StandardR
        && area == BasicArea::S
        && !sfc_supported(model, evidence.r_firmware)
    {
        return PointCount::Unknown;
    }
    let Some(words) = words.filter(|words| words.len() == usize::from(plan.count)) else {
        return PointCount::Unknown;
    };
    let count = if plan.count == 1 {
        u32::from(words[0])
    } else {
        u32::from(words[0]) | (u32::from(words[1]) << 16)
    };
    if model.family == ModelFamily::StandardQ
        && matches!(
            area,
            BasicArea::M | BasicArea::B | BasicArea::DInternal | BasicArea::WInternal
        )
        && count == 32768
    {
        PointCount::AtLeast(32768)
    } else {
        PointCount::Exact(count)
    }
}

fn sfc_supported(model: &TypeName, firmware: Option<u16>) -> bool {
    if matches!(model.name.as_str(), "R00CPU" | "R01CPU" | "R02CPU") {
        true
    } else {
        firmware.is_some_and(|version| version >= 12)
    }
}

fn q_sd(area: BasicArea) -> Option<u16> {
    Some(match area {
        BasicArea::X => 290,
        BasicArea::Y => 291,
        BasicArea::M => 292,
        BasicArea::L => 293,
        BasicArea::B => 294,
        BasicArea::F => 295,
        BasicArea::SB => 296,
        BasicArea::V => 297,
        BasicArea::S => 298,
        BasicArea::T => 299,
        BasicArea::ST => 300,
        BasicArea::C => 301,
        BasicArea::DInternal => 302,
        BasicArea::WInternal => 303,
        BasicArea::SW => 304,
        BasicArea::D | BasicArea::W => return None,
        BasicArea::LT
        | BasicArea::LST
        | BasicArea::LC
        | BasicArea::Z
        | BasicArea::LZ
        | BasicArea::ZR
        | BasicArea::RD => return None,
    })
}

fn r_sd(area: BasicArea) -> Option<(u16, u16)> {
    Some(match area {
        BasicArea::X => (260, 2),
        BasicArea::Y => (262, 2),
        BasicArea::M => (264, 2),
        BasicArea::B => (266, 2),
        BasicArea::SB => (268, 2),
        BasicArea::F => (270, 2),
        BasicArea::V => (272, 2),
        BasicArea::L => (274, 2),
        BasicArea::S => (276, 2),
        BasicArea::D => (280, 2),
        BasicArea::W => (282, 2),
        BasicArea::SW => (284, 2),
        BasicArea::DInternal | BasicArea::WInternal => return None,
        BasicArea::T => (288, 2),
        BasicArea::ST => (290, 2),
        BasicArea::C => (292, 2),
        BasicArea::LT => (294, 2),
        BasicArea::LST => (296, 2),
        BasicArea::LC => (298, 2),
        BasicArea::Z => (300, 1),
        BasicArea::LZ => (302, 1),
        BasicArea::ZR => (306, 2),
        BasicArea::RD => (308, 2),
    })
}
