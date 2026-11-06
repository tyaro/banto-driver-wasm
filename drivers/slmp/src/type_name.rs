//! 0101/0000 Read Type Name (binary 4E or 3E). The lookup is deliberately limited
//! to Q (ordinary, process, universal process), MELSEC-L, iQ-R/EN, iQ-R process,
//! SIL2 process and safety CPU, and MX-R pairs plus the two redundant-CPU
//! wire-only pairs in the official model-code table.
use alloc::{string::String, vec::Vec};

use crate::{request_head, response_data, ResponseError, Route};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFamily {
    StandardQ,
    StandardR,
    /// Exact Q25PRHCPU/004CH and Q12PRHCPU/004BH redundant-CPU pairs: Q wire
    /// and SD302/SD303 internal D/W only. Both pairs share the same read and
    /// write rules (2026-11-06 owner decision).
    Q25PrhWireOnly,
    /// Valid 0101 data whose name/code pair is not in the supported table.
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeName {
    /// All 16 bytes, including the original space padding.
    pub raw_name: [u8; 16],
    /// Printable ASCII name with only trailing space padding removed.
    pub name: String,
    pub code: u16,
    pub family: ModelFamily,
}

/// Build one 4E or 3E request for Read Type Name (0101/0000).
pub fn encode_type_name(route: Route) -> Vec<u8> {
    let mut out = request_head(route, 6);
    out.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
    out
}

/// Decode a complete 4E or 3E response. A recognized model still needs configured
/// SD counts and route/firmware checks before tag ranges can be asserted.
pub fn decode_type_name(route: Route, frame: &[u8]) -> Result<TypeName, ResponseError> {
    let data = response_data(route, frame)?;
    if data.len() != 18 {
        return Err(ResponseError::Framing);
    }
    let mut raw_name = [0; 16];
    raw_name.copy_from_slice(&data[..16]);
    if !raw_name.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        return Err(ResponseError::MalformedName);
    }
    let name = core::str::from_utf8(&raw_name)
        .map_err(|_| ResponseError::MalformedName)?
        .trim_end_matches(' ');
    if name.is_empty() {
        return Err(ResponseError::MalformedName);
    }
    let code = u16::from_le_bytes([data[16], data[17]]);
    Ok(TypeName {
        raw_name,
        name: String::from(name),
        code,
        family: classify(name, code),
    })
}

// SLMP Reference Manual pp. 139-141 (printed), Q, process, universal process,
// MELSEC-L, iQ-R/EN, iQ-R process/SIL2 process/safety and MX-R entries plus
// Q25PRH/Q12PRH wire-only. Codes require an exact listed name. Remote, iQ-L and
// the RCPU/0360H and MX-R/037EH route fallbacks stay unknown.
fn classify(name: &str, code: u16) -> ModelFamily {
    const Q: &[(&str, u16)] = &[
        ("Q00JCPU", 0x0250),
        ("Q00CPU", 0x0251),
        ("Q01CPU", 0x0252),
        ("Q02CPU", 0x0041),
        ("Q02HCPU", 0x0041),
        ("Q06HCPU", 0x0042),
        ("Q12HCPU", 0x0043),
        ("Q25HCPU", 0x0044),
        // Process CPU: standard non-U Q (G2a). Codes are shared with QnH.
        ("Q02PHCPU", 0x0041),
        ("Q06PHCPU", 0x0042),
        ("Q12PHCPU", 0x0043),
        ("Q25PHCPU", 0x0044),
        // Universal process CPU: QnU (G2a). Codes are shared with QnUDV.
        ("Q04UDPVCPU", 0x0367),
        ("Q06UDPVCPU", 0x0368),
        ("Q13UDPVCPU", 0x036a),
        ("Q26UDPVCPU", 0x036c),
        // MELSEC-L: Q wire and the basic SD290-304 table only (G2a).
        ("L02SCPU", 0x0543),
        ("L02SCPU-P", 0x0543),
        ("L02CPU", 0x0541),
        ("L02CPU-P", 0x0541),
        ("L06CPU", 0x0544),
        ("L06CPU-P", 0x0544),
        ("L26CPU", 0x0545),
        ("L26CPU-P", 0x0545),
        ("L26CPU-BT", 0x0542),
        ("L26CPU-PBT", 0x0542),
        ("Q00UJCPU", 0x0260),
        ("Q00UCPU", 0x0261),
        ("Q01UCPU", 0x0262),
        ("Q02UCPU", 0x0263),
        ("Q03UDCPU", 0x0268),
        ("Q03UDECPU", 0x0268),
        ("Q03UDVCPU", 0x0366),
        ("Q04UDHCPU", 0x0269),
        ("Q04UDEHCPU", 0x0269),
        ("Q04UDVCPU", 0x0367),
        ("Q06UDHCPU", 0x026a),
        ("Q06UDEHCPU", 0x026a),
        ("Q06UDVCPU", 0x0368),
        ("Q10UDHCPU", 0x0266),
        ("Q10UDEHCPU", 0x0266),
        ("Q13UDHCPU", 0x026b),
        ("Q13UDEHCPU", 0x026b),
        ("Q13UDVCPU", 0x036a),
        ("Q20UDHCPU", 0x0267),
        ("Q20UDEHCPU", 0x0267),
        ("Q26UDHCPU", 0x026c),
        ("Q26UDEHCPU", 0x026c),
        ("Q26UDVCPU", 0x036c),
        ("Q50UDEHCPU", 0x026d),
        ("Q100UDEHCPU", 0x026e),
    ];
    const R: &[(&str, u16)] = &[
        ("R00CPU", 0x48a0),
        ("R01CPU", 0x48a1),
        ("R02CPU", 0x48a2),
        ("R04CPU", 0x4800),
        ("R04ENCPU", 0x4805),
        ("R08CPU", 0x4801),
        ("R08ENCPU", 0x4806),
        ("R16CPU", 0x4802),
        ("R16ENCPU", 0x4807),
        ("R32CPU", 0x4803),
        ("R32ENCPU", 0x4808),
        ("R120CPU", 0x4804),
        ("R120ENCPU", 0x4809),
        // Process CPU (G2b): same wire and SD table as standard R.
        ("R08PCPU", 0x4841),
        ("R16PCPU", 0x4842),
        ("R32PCPU", 0x4843),
        ("R120PCPU", 0x4844),
        // SIL2 process CPU (G2b): standard R rules, but no S device.
        ("R08PSFCPU", 0x4851),
        ("R16PSFCPU", 0x4852),
        ("R32PSFCPU", 0x4853),
        ("R120PSFCPU", 0x4854),
        // Safety CPU (G2b): standard R rules, but no S device.
        ("R08SFCPU", 0x4891),
        ("R16SFCPU", 0x4892),
        ("R32SFCPU", 0x4893),
        ("R120SFCPU", 0x4894),
        // MX-R (G2b): standard R rules, but no S device.
        ("MXR300-16", 0x48e9),
        ("MXR300-32", 0x48ea),
        ("MXR300-64", 0x48eb),
        ("MXR500-128", 0x48ee),
        ("MXR500-256", 0x48ef),
    ];
    if matches!((name, code), ("Q25PRHCPU", 0x004c) | ("Q12PRHCPU", 0x004b)) {
        // The 0101 table proves these pairs' Q identity, but not SD302's
        // applicability. Never classify it as StandardQ for SD planning.
        ModelFamily::Q25PrhWireOnly
    } else if Q.contains(&(name, code)) {
        ModelFamily::StandardQ
    } else if R.contains(&(name, code)) {
        ModelFamily::StandardR
    } else {
        ModelFamily::Unknown
    }
}
