//! 0101/0000 Read Type Name (binary 4E). The lookup is deliberately limited
//! to ordinary Q and iQ-R/EN CPU pairs in the official model-code table.
use alloc::{string::String, vec::Vec};

use crate::{request_head, response_data, ResponseError, Route};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFamily {
    StandardQ,
    StandardR,
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

/// Build one 4E request for Read Type Name (0101/0000).
pub fn encode_type_name(route: Route) -> Vec<u8> {
    let mut out = request_head(route, 6);
    out.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
    out
}

/// Decode a complete 4E response. A recognized model still needs configured
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

// SLMP Reference Manual pp. 139-141 (printed), ordinary Q and iQ-R/EN
// entries only. Shared codes are accepted only with an exact listed name.
// Process, safety, remote, L, MX and the RCPU/0360H route fallback stay unknown.
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
        ("Q13UDHCPU", 0x026b),
        ("Q13UDEHCPU", 0x026b),
        ("Q13UDVCPU", 0x036a),
        ("Q20UDHCPU", 0x0267),
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
    ];
    if Q.contains(&(name, code)) {
        ModelFamily::StandardQ
    } else if R.contains(&(name, code)) {
        ModelFamily::StandardR
    } else {
        ModelFamily::Unknown
    }
}
