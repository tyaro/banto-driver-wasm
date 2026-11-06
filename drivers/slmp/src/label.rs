//! Binary 4E/3E global label framing (G3): Label Read Random (041C) and
//! Label Write Random (141B), both subcommand 0000 and without the
//! abbreviation definition (SLMP reference manual SH-080956ENG N, 5.3,
//! printed pp.78-86, 106-118).
//!
//! Request after the monitoring timer (all binary fields little-endian):
//! command, subcommand 0000, number of labels (u16), number of abbreviation
//! points (u16, always 0), then per label the name length in UTF-16 code
//! units (u16) and the name as UTF-16LE. 141B adds, per label, the write data
//! length in bytes (u16) and the write data.
//!
//! A 041C response holds the number of labels (u16) and then per label: data
//! type ID (u8), one undefined spare byte, read data length in bytes (u16) and
//! the read data. The manual stores read and write data in two-byte units
//! regardless of the type, so an odd length is a malformed response. 141B
//! answers with no data.
//!
//! The manual caps the "send data" of one label request at 1920 bytes without
//! saying which fields count. This core takes the strict reading: the request
//! data length field of the header (monitoring timer through the last byte)
//! must not exceed 1920.
//!
//! The core does no type checking: which data type ID and length a tag needs
//! is the guest's decision.

use alloc::vec::Vec;

use crate::{request_head, response_data, RequestError, ResponseError, Route};

/// Largest request data length (monitoring timer onward) of one label request.
pub const LABEL_SEND_DATA_LIMIT: usize = 1920;

const LABEL_READ: u16 = 0x041c;
const LABEL_WRITE: u16 = 0x141b;
/// Monitoring timer, command, subcommand, label count and abbreviation count.
const FIXED_LEN: usize = 10;

/// Request data length of a 041C for labels of these UTF-16 lengths.
pub fn label_read_data_len(name_units: impl IntoIterator<Item = usize>) -> usize {
    name_units
        .into_iter()
        .fold(FIXED_LEN, |total, units| total + 2 + 2 * units)
}

/// Request data length of a 141B for `(name units, write data bytes)` items.
pub fn label_write_data_len(items: impl IntoIterator<Item = (usize, usize)>) -> usize {
    items.into_iter().fold(FIXED_LEN, |total, (units, data)| {
        total + 4 + 2 * units + data
    })
}

/// One label of a 141B request: the UTF-16 name and its write data bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LabelWrite<'a> {
    pub name: &'a [u16],
    pub data: &'a [u8],
}

/// One label of a 041C response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelData {
    pub type_id: u8,
    pub data: Vec<u8>,
}

fn check_name(name: &[u16]) -> Result<u16, RequestError> {
    match u16::try_from(name.len()) {
        Ok(units) if units > 0 => Ok(units),
        _ => Err(RequestError::LabelName),
    }
}

fn label_head(route: Route, command: u16, count: usize, data_len: usize) -> Vec<u8> {
    let mut out = request_head(route, data_len as u16);
    out.extend_from_slice(&command.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // no abbreviation definition
    out
}

fn push_name(out: &mut Vec<u8>, name: &[u16], units: u16) {
    out.extend_from_slice(&units.to_le_bytes());
    for unit in name {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

/// Build one 041C (subcommand 0000) for these labels, in order.
pub fn encode_label_read(route: Route, names: &[&[u16]]) -> Result<Vec<u8>, RequestError> {
    if names.is_empty() || names.len() > usize::from(u16::MAX) {
        return Err(RequestError::Count);
    }
    for name in names {
        check_name(name)?;
    }
    let data_len = label_read_data_len(names.iter().map(|name| name.len()));
    if data_len > LABEL_SEND_DATA_LIMIT {
        return Err(RequestError::TooLarge);
    }
    let mut out = label_head(route, LABEL_READ, names.len(), data_len);
    for name in names {
        push_name(&mut out, name, name.len() as u16);
    }
    Ok(out)
}

/// Build one 141B (subcommand 0000) writing every item in one request.
/// Write data must be a nonempty whole number of words.
pub fn encode_label_write(route: Route, items: &[LabelWrite<'_>]) -> Result<Vec<u8>, RequestError> {
    if items.is_empty() || items.len() > usize::from(u16::MAX) {
        return Err(RequestError::Count);
    }
    for item in items {
        check_name(item.name)?;
        if item.data.is_empty() || item.data.len() % 2 != 0 {
            return Err(RequestError::Count);
        }
    }
    let data_len =
        label_write_data_len(items.iter().map(|item| (item.name.len(), item.data.len())));
    if data_len > LABEL_SEND_DATA_LIMIT {
        return Err(RequestError::TooLarge);
    }
    let mut out = label_head(route, LABEL_WRITE, items.len(), data_len);
    for item in items {
        push_name(&mut out, item.name, item.name.len() as u16);
        out.extend_from_slice(&(item.data.len() as u16).to_le_bytes());
        out.extend_from_slice(item.data);
    }
    Ok(out)
}

/// Parse a complete 041C response to this request of `count` labels. The
/// echoed label count, every entry and the total length must match exactly.
pub fn decode_label_read(
    route: Route,
    count: usize,
    frame: &[u8],
) -> Result<Vec<LabelData>, ResponseError> {
    let data = response_data(route, frame)?;
    let (echo, mut rest) = data.split_at_checked(2).ok_or(ResponseError::Framing)?;
    if usize::from(u16::from_le_bytes([echo[0], echo[1]])) != count {
        return Err(ResponseError::Framing);
    }
    let mut labels = Vec::with_capacity(count);
    for _ in 0..count {
        let (head, tail) = rest.split_at_checked(4).ok_or(ResponseError::Framing)?;
        // head[1] is the spare byte; its value is undefined and ignored.
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        if len % 2 != 0 {
            return Err(ResponseError::Framing);
        }
        let (value, tail) = tail.split_at_checked(len).ok_or(ResponseError::Framing)?;
        labels.push(LabelData {
            type_id: head[0],
            data: value.to_vec(),
        });
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(ResponseError::Framing);
    }
    Ok(labels)
}
