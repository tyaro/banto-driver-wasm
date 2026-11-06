//! G3 global labels: `@` followed by a GX Works3 global label name, read with
//! 041C and written with 141B (SLMP reference manual SH-080956ENG N 5.3).
//!
//! Address grammar (2026-11-06 owner decision, docs/PLAN.md "G3"):
//! - The name is sent as written (case preserved) in UTF-16. The guest does not
//!   check character classes; an empty name, whitespace, control characters,
//!   a `.` followed by a digit (a bit specification such as `@X.3`, which SLMP
//!   refuses) and malformed subscripts are parse errors.
//! - `.` separates structure members; each member may carry one subscript
//!   group `[a]`, `[a,b]` or `[a,b,c]` of decimal numbers. The canonical text
//!   drops leading zeros. A name too long to fit one 141B request of its type
//!   within the 1920-byte send data limit is a parse error.
//! - Types and data type IDs (printed p.84): bit 1, u16 2, u32 3, i16 4, i32 5,
//!   f32 6, f64 7, string 9 (`2 x string_words` bytes). There is no 64-bit
//!   integer label type, so i64/u64 are parse errors.
//! - `offset` advances only the last subscript, wherever its group is.
//!
//! Label data uses the PLC's own little-endian layout: `word_order` (a setting
//! for device pairs) does not apply. A bit label holds one word whose b0 is
//! the value and whose other bits are 0 (printed pp.109, 116). A string label
//! write must contain a NUL end byte (printed p.115) or the request is rejected.
//!
//! Writes need `d_u16_write_mode=enabled` and an entry of
//! `label_write_allowlist`: `;`-separated canonical labels (`@Tank1.Level`) or
//! ranges of the last subscript (`@Arr[0-9]`, `@M[1,0-15]`); no whitespace, at
//! most 32 entries and 2048 characters, case-sensitive.

use super::*;
use banto_slmp_read_core::{label_write_data_len, LABEL_SEND_DATA_LIMIT};

pub(super) const LABEL_ALLOWLIST_KEY: &str = "label_write_allowlist";
const MAX_ALLOWLIST_LEN: usize = 2048;
const MAX_ALLOWLIST_ENTRIES: usize = 32;
const MAX_DIMENSIONS: usize = 3;

/// Response allowance per label beyond its 4-byte head: the GX Works3 string
/// and Unicode string types hold at most 255 characters (at most 512 bytes),
/// so a label of another type than the tag still fits and is reported per item.
const MAX_LABEL_DATA: u32 = 512;

pub(super) fn is_label(text: &str) -> bool {
    text.starts_with('@')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LabelType {
    Bit,
    U16,
    U32,
    I16,
    I32,
    F32,
    F64,
    /// Character string label read and written as this many words.
    String(u16),
}

impl LabelType {
    fn parse(data_type: &str, string_words: Option<u32>) -> Result<Self, String> {
        if data_type == "string" {
            let words = string_words.ok_or("string tags require string_words")?;
            if !(1..=MAX_STRING_WORDS).contains(&words) {
                return Err(format!("string_words must be 1..={MAX_STRING_WORDS}"));
            }
            return Ok(Self::String(words as u16));
        }
        if string_words.is_some() {
            return Err("only string labels accept string_words".into());
        }
        Ok(match data_type {
            "bit" => Self::Bit,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "i64" | "u64" => return Err("labels have no 64-bit integer type".into()),
            _ => return Err("unsupported label type".into()),
        })
    }

    /// Data type ID of a label of this type (printed p.84).
    pub(super) const fn type_id(self) -> u8 {
        match self {
            Self::Bit => 1,
            Self::U16 => 2,
            Self::U32 => 3,
            Self::I16 => 4,
            Self::I32 => 5,
            Self::F32 => 6,
            Self::F64 => 7,
            Self::String(_) => 9,
        }
    }

    /// Read and write data length in bytes (printed p.115).
    pub(super) const fn data_len(self) -> usize {
        match self {
            Self::Bit | Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::F64 => 8,
            Self::String(words) => 2 * words as usize,
        }
    }

    pub(super) const fn value_words(self) -> u32 {
        (self.data_len() / 2) as u32
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bit => "bit",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String(_) => "string",
        }
    }

    /// `None` when the label's ID and length match this tag; otherwise why not.
    pub(super) fn mismatch(self, type_id: u8, len: usize) -> Option<String> {
        if type_id != self.type_id() {
            Some(format!(
                "label data type ID is {type_id}, tag type {} needs ID {}",
                self.name(),
                self.type_id()
            ))
        } else if len != self.data_len() {
            Some(format!(
                "label data length is {len} bytes, tag type {} needs {} bytes",
                self.name(),
                self.data_len()
            ))
        } else {
            None
        }
    }

    /// Decode data whose ID and length already match. A bit word other than
    /// 0 or 1 is a malformed response.
    pub(super) fn decode(self, data: &[u8]) -> Result<Value, FatalError> {
        let malformed = || FatalError::Framing("invalid SLMP label data".into());
        if data.len() != self.data_len() {
            return Err(malformed());
        }
        let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
        let u32_at = || u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        Ok(match self {
            Self::Bit => match u16_at(0) {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(malformed()),
            },
            Self::U16 => Value::Uint(u64::from(u16_at(0))),
            Self::I16 => Value::Int(i64::from(u16_at(0) as i16)),
            Self::U32 => Value::Uint(u64::from(u32_at())),
            Self::I32 => Value::Int(i64::from(u32_at() as i32)),
            Self::F32 => Value::Float(f64::from(f32::from_bits(u32_at()))),
            Self::F64 => Value::Float(f64::from_le_bytes(
                data.try_into().expect("checked eight bytes"),
            )),
            // As with D/W strings: the wire bytes unchanged, no NUL cut.
            Self::String(_) => Value::Bytes(data.to_vec()),
        })
    }

    /// Write data of exactly `data_len` bytes, with the same value checks as
    /// a device tag of this type.
    pub(super) fn encode(self, value: &Value) -> Result<Vec<u8>, String> {
        let words = match self {
            Self::Bit => {
                let Value::Bool(bit) = value else {
                    return Err("bit write requires a bool value".into());
                };
                vec![u16::from(*bit)]
            }
            Self::U16 => DKind::U16.encode(value, WordOrder::LowHigh)?,
            Self::U32 => DKind::U32.encode(value, WordOrder::LowHigh)?,
            Self::I16 => DKind::I16.encode(value, WordOrder::LowHigh)?,
            Self::I32 => DKind::I32.encode(value, WordOrder::LowHigh)?,
            Self::F32 => DKind::F32.encode(value, WordOrder::LowHigh)?,
            Self::F64 => DKind::F64.encode(value, WordOrder::LowHigh)?,
            Self::String(words) => {
                let words = encode_string(value, words)?;
                if !words.iter().any(|w| w.to_le_bytes().contains(&0)) {
                    return Err(
                        "string label write requires a NUL terminator within 2 x string_words bytes"
                            .into(),
                    );
                }
                words
            }
        };
        Ok(words.iter().flat_map(|word| word.to_le_bytes()).collect())
    }
}

/// A parsed label tag. The canonical text is `head`, then the last subscript
/// and `tail` when the name has a subscript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Label {
    head: String,
    last: Option<(u32, String)>,
    pub(super) ty: LabelType,
}

impl Label {
    /// Canonical tag text, including the leading `@`.
    pub(super) fn canonical(&self) -> String {
        match &self.last {
            Some((number, tail)) => format!("{}{number}{tail}", self.head),
            None => self.head.clone(),
        }
    }

    /// The label name as sent: canonical text without `@`, in UTF-16.
    pub(super) fn utf16(&self) -> Vec<u16> {
        self.canonical()[1..].encode_utf16().collect()
    }
}

/// Split a name (without `@`) into canonical text before the last subscript
/// and, when there is one, that subscript and the canonical text after it.
fn parse_name(name: &str) -> Result<(String, Option<(u32, String)>), String> {
    if name.is_empty() {
        return Err("label name is empty".into());
    }
    if name.chars().any(char::is_whitespace) {
        return Err("label name must not contain whitespace".into());
    }
    if name.chars().any(char::is_control) {
        return Err("label name must not contain control characters".into());
    }
    let mut out = String::with_capacity(name.len());
    // Byte range of the most recent subscript number in `out`.
    let mut last: Option<(usize, usize, u32)> = None;
    for (index, member) in name.split('.').enumerate() {
        if index > 0 {
            if member.starts_with(|c: char| c.is_ascii_digit()) {
                return Err("bit specification (.n) of a label is not supported".into());
            }
            out.push('.');
        }
        let (base, subscripts) = match member.split_once('[') {
            Some((base, rest)) => {
                let inner = rest
                    .strip_suffix(']')
                    .ok_or("label subscript must close with ] at the end of a member")?;
                (base, Some(inner))
            }
            None => (member, None),
        };
        if base.is_empty() {
            return Err("label name has an empty member".into());
        }
        if base.contains([']', ',']) {
            return Err("malformed label subscript".into());
        }
        out.push_str(base);
        let Some(inner) = subscripts else {
            continue;
        };
        let parts = inner.split(',').collect::<Vec<_>>();
        if parts.len() > MAX_DIMENSIONS {
            return Err("label arrays have at most three dimensions".into());
        }
        out.push('[');
        for (position, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err("label subscript must be a decimal number".into());
            }
            let number = part
                .parse::<u32>()
                .map_err(|_| "label subscript exceeds u32")?;
            if position > 0 {
                out.push(',');
            }
            let start = out.len();
            out.push_str(&number.to_string());
            last = Some((start, out.len(), number));
        }
        out.push(']');
    }
    Ok(match last {
        Some((start, end, number)) => (
            format!("@{}", &out[..start]),
            Some((number, out[end..].to_owned())),
        ),
        None => (format!("@{out}"), None),
    })
}

pub(super) fn parse_label(
    text: &str,
    data_type: &str,
    string_words: Option<u32>,
) -> Result<Label, String> {
    let name = text
        .strip_prefix('@')
        .ok_or("label address must start with @")?;
    let (head, last) = parse_name(name)?;
    let ty = LabelType::parse(data_type, string_words)?;
    let label = Label { head, last, ty };
    // One label of this type must fit one 141B (and so one 041C) request.
    if label_write_data_len([(label.utf16().len(), ty.data_len())]) > LABEL_SEND_DATA_LIMIT {
        return Err("label name is too long for one SLMP label request".into());
    }
    Ok(label)
}

/// `address.offset` for a label: only the last subscript moves.
pub(super) fn offset_label(
    text: &str,
    data_type: &str,
    string_words: Option<u32>,
    n: i64,
) -> Result<String, String> {
    let label = parse_label(text, data_type, string_words)?;
    let Some((number, tail)) = &label.last else {
        return Err("a label without a subscript has no offset".into());
    };
    let next = i64::from(*number).checked_add(n).ok_or("offset overflow")?;
    let next = u32::try_from(next).map_err(|_| "label subscript out of range")?;
    let moved = Label {
        last: Some((next, tail.clone())),
        ..label
    }
    .canonical();
    parse_label(&moved, data_type, string_words)?;
    Ok(moved)
}

pub(super) fn parsed_label(label: &Label) -> address::Parsed {
    address::Parsed {
        canonical: label.canonical(),
        // One area for every label; without an index the Hub does not compare
        // overlap (a structure and its member can name the same storage).
        area: "@".into(),
        // Static capability; label_write_allowlist and the Host gate decide.
        writable_area: true,
        bit: None,
        word_count: label.ty.value_words(),
        index: None,
    }
}

/// Response size bound for a 041C of these labels (see `MAX_LABEL_DATA`).
pub(super) fn read_max_len(frame: Frame, labels: &[Label]) -> u32 {
    let per_label = labels
        .iter()
        .map(|label| 4 + (label.ty.data_len() as u32).max(MAX_LABEL_DATA))
        .sum::<u32>();
    frame.response_header_len() as u32 + 2 + per_label
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AllowEntry {
    Exact(String),
    /// Same canonical head and tail, last subscript within `start..=end`.
    Range {
        head: String,
        start: u32,
        end: u32,
        tail: String,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct LabelAllowlist {
    entries: Vec<AllowEntry>,
}

fn canonical_decimal(text: &str) -> Option<u32> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

fn parse_entry(entry: &str) -> Result<AllowEntry, &'static str> {
    const NOT_CANONICAL: &str = "label allowlist entries must be canonical @labels";
    let name = entry.strip_prefix('@').ok_or(NOT_CANONICAL)?;
    // A range sits in the last element of the last subscript group.
    let range = name.rfind('[').and_then(|open| {
        let close = open + name[open..].find(']')?;
        let element_start = name[open + 1..close]
            .rfind(',')
            .map_or(open + 1, |comma| open + 2 + comma);
        let (start, end) = name[element_start..close].split_once('-')?;
        Some((element_start, close, start, end))
    });
    if let Some((element_start, close, start, end)) = range {
        let (Some(start), Some(end)) = (canonical_decimal(start), canonical_decimal(end)) else {
            return Err("label allowlist range must be decimal start-end without leading zeros");
        };
        if start > end {
            return Err("label allowlist range is reversed");
        }
        let probe = format!("{}{start}{}", &name[..element_start], &name[close..]);
        let (head, last) = parse_name(&probe).map_err(|_| NOT_CANONICAL)?;
        let Some((number, tail)) = last else {
            return Err(NOT_CANONICAL);
        };
        if format!("{head}{number}{tail}") != format!("@{probe}") {
            return Err(NOT_CANONICAL);
        }
        return Ok(AllowEntry::Range {
            head,
            start,
            end,
            tail,
        });
    }
    let (head, last) = parse_name(name).map_err(|_| NOT_CANONICAL)?;
    let canonical = match last {
        Some((number, tail)) => format!("{head}{number}{tail}"),
        None => head,
    };
    if canonical != entry {
        return Err(NOT_CANONICAL);
    }
    Ok(AllowEntry::Exact(canonical))
}

impl LabelAllowlist {
    pub(super) fn parse(raw: Option<&str>) -> Result<Self, (&'static str, &'static str)> {
        let fail = |message| (LABEL_ALLOWLIST_KEY, message);
        let raw = raw.unwrap_or("");
        if raw.chars().count() > MAX_ALLOWLIST_LEN {
            return Err(fail("label write allowlist is too long"));
        }
        if raw.is_empty() {
            return Ok(Self::default());
        }
        if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(fail("label write allowlist must not contain whitespace"));
        }
        if raw.contains('*') {
            return Err(fail("label write allowlist has no wildcards"));
        }
        let mut entries = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for entry in raw.split(';') {
            if entry.is_empty() {
                return Err(fail("label write allowlist has an empty entry"));
            }
            if entries.len() == MAX_ALLOWLIST_ENTRIES {
                return Err(fail("too many label write allowlist entries"));
            }
            if seen.contains(&entry) {
                return Err(fail("duplicate label write allowlist entry"));
            }
            seen.push(entry);
            entries.push(parse_entry(entry).map_err(fail)?);
        }
        Ok(Self { entries })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Exact canonical match, or the same head and tail with the last
    /// subscript inside a range. Case-sensitive.
    pub(super) fn allows(&self, label: &Label) -> bool {
        let canonical = label.canonical();
        self.entries.iter().any(|entry| match entry {
            AllowEntry::Exact(text) => *text == canonical,
            AllowEntry::Range {
                head,
                start,
                end,
                tail,
            } => label.last.as_ref().is_some_and(|(number, label_tail)| {
                label.head == *head && label_tail == tail && (start..=end).contains(&number)
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str, data_type: &str) -> Result<Label, String> {
        parse_label(text, data_type, None)
    }

    #[test]
    fn names_parse_canonicalize_and_keep_case_and_non_ascii() {
        for (text, canonical) in [
            ("@Tank1.Level", "@Tank1.Level"),
            ("@tank1.level", "@tank1.level"),
            ("@Arr[03]", "@Arr[3]"),
            ("@M[01,002]", "@M[1,2]"),
            ("@C[1,2,3]", "@C[1,2,3]"),
            ("@St.Arr[2]", "@St.Arr[2]"),
            ("@Arr[2].x", "@Arr[2].x"),
            ("@XXX.YYY[20,10,30]", "@XXX.YYY[20,10,30]"),
            ("@タンク1.水位", "@タンク1.水位"),
            ("@ラベル[0]", "@ラベル[0]"),
            ("@X.Y3", "@X.Y3"),
            ("@a-b", "@a-b"),
        ] {
            let parsed = label(text, "u16").unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(parsed.canonical(), canonical);
        }
        assert_eq!(
            label("@タ", "u16").unwrap().utf16(),
            vec![0x30bf],
            "@ is not sent"
        );
    }

    #[test]
    fn malformed_names_and_types_are_parse_errors() {
        for text in [
            "@",
            "@ A",
            "@A B",
            "@A\u{3000}",
            "@A\tB",
            "@A\u{7}",
            "@X.3",
            "@X.0A",
            "@St.Arr.1",
            "@.A",
            "@A.",
            "@A..B",
            "@Arr[]",
            "@Arr[1",
            "@Arr]",
            "@Arr[1]]",
            "@Arr[1]x",
            "@Arr[1][2]",
            "@Arr[1,2,3,4]",
            "@Arr[a]",
            "@Arr[-1]",
            "@Arr[+1]",
            "@Arr[1,]",
            "@Arr[,1]",
            "@Arr[4294967296]",
            "@[1]",
            "@A,B",
            "A",
        ] {
            assert!(label(text, "u16").is_err(), "{text}");
        }
        for data_type in ["i64", "u64", "string", "f16", "word"] {
            assert!(label("@A", data_type).is_err(), "{data_type}");
        }
        assert!(parse_label("@A", "u16", Some(1)).is_err());
        assert!(parse_label("@A", "string", Some(0)).is_err());
        assert!(parse_label("@A", "string", Some(129)).is_err());
        assert!(parse_label("@A", "string", Some(128)).is_ok());
    }

    #[test]
    fn name_length_is_bounded_by_one_request_of_the_type() {
        // 14 + 2n + data <= 1920: f64 allows 949 units, a 128-word string 825.
        let name = |units: usize| format!("@{}", "A".repeat(units));
        assert!(label(&name(949), "f64").is_ok());
        assert!(label(&name(950), "f64").is_err());
        assert!(label(&name(952), "u16").is_ok());
        assert!(label(&name(953), "u16").is_err());
        assert!(parse_label(&name(825), "string", Some(128)).is_ok());
        assert!(parse_label(&name(826), "string", Some(128)).is_err());
        // Counted in UTF-16 code units: U+1F600 is two.
        let fits = format!("@{}", "😀".repeat(476));
        assert_eq!(label(&fits, "u16").map(|l| l.utf16().len()), Ok(952));
        let over = format!("@{}A", "😀".repeat(476));
        assert!(label(&over, "u16").is_err());
    }

    #[test]
    fn offset_moves_only_the_last_subscript() {
        let off = |text: &str, n| offset_label(text, "u16", None, n);
        assert_eq!(off("@Arr[3]", 2).as_deref(), Ok("@Arr[5]"));
        assert_eq!(off("@M[1,2]", 1).as_deref(), Ok("@M[1,3]"));
        assert_eq!(off("@C[1,2,03]", -3).as_deref(), Ok("@C[1,2,0]"));
        assert_eq!(off("@St.Arr[2].x", 1).as_deref(), Ok("@St.Arr[3].x"));
        assert_eq!(off("@A[1].B[7]", 0).as_deref(), Ok("@A[1].B[7]"));
        assert!(off("@Arr[3]", -4).is_err());
        assert!(off("@Tank1.Level", 1).is_err());
        assert!(off("@Arr[4294967295]", 1).is_err());
    }

    #[test]
    fn type_ids_lengths_and_values_follow_the_manual() {
        for (data_type, id, len) in [
            ("bit", 1, 2),
            ("u16", 2, 2),
            ("u32", 3, 4),
            ("i16", 4, 2),
            ("i32", 5, 4),
            ("f32", 6, 4),
            ("f64", 7, 8),
        ] {
            let ty = LabelType::parse(data_type, None).unwrap();
            assert_eq!((ty.type_id(), ty.data_len()), (id, len), "{data_type}");
            assert_eq!(ty.mismatch(id, len), None);
            assert!(ty.mismatch(id + 1, len).is_some());
            assert!(ty.mismatch(id, len + 2).is_some());
        }
        let s = LabelType::String(4);
        assert_eq!((s.type_id(), s.data_len()), (9, 8));
        assert!(s.mismatch(10, 8).is_some(), "Unicode strings are not read");
        assert_eq!(
            LabelType::I16.mismatch(5, 4).as_deref(),
            Some("label data type ID is 5, tag type i16 needs ID 4")
        );
        let decoded = |ty: LabelType, data: &[u8]| format!("{:?}", ty.decode(data).ok());
        assert_eq!(decoded(LabelType::Bit, &[1, 0]), "Some(Value::Bool(true))");
        assert_eq!(decoded(LabelType::Bit, &[0, 0]), "Some(Value::Bool(false))");
        assert!(LabelType::Bit.decode(&[2, 0]).is_err());
        assert!(LabelType::Bit.decode(&[1, 1]).is_err());
        assert_eq!(
            decoded(LabelType::I16, &[0xff, 0xff]),
            "Some(Value::Int(-1))"
        );
        assert_eq!(
            decoded(LabelType::U16, &[0xff, 0xff]),
            "Some(Value::Uint(65535))"
        );
        assert_eq!(
            decoded(LabelType::U32, &[0x78, 0x56, 0x34, 0x12]),
            "Some(Value::Uint(305419896))"
        );
        assert_eq!(
            decoded(LabelType::I32, &[0xfe, 0xff, 0xff, 0xff]),
            "Some(Value::Int(-2))"
        );
        assert_eq!(
            decoded(LabelType::F32, &1.5f32.to_le_bytes()),
            "Some(Value::Float(1.5))"
        );
        assert_eq!(
            decoded(LabelType::F64, &(-2.25f64).to_le_bytes()),
            "Some(Value::Float(-2.25))"
        );
        assert_eq!(
            decoded(s, b"AB\0\0xy\0\0"),
            format!("{:?}", Some(Value::Bytes(b"AB\0\0xy\0\0".to_vec())))
        );
        assert_eq!(LabelType::Bit.encode(&Value::Bool(true)), Ok(vec![1, 0]));
        assert_eq!(LabelType::Bit.encode(&Value::Bool(false)), Ok(vec![0, 0]));
        assert!(LabelType::Bit.encode(&Value::Uint(1)).is_err());
        assert_eq!(
            LabelType::U32.encode(&Value::Uint(0x1234_5678)),
            Ok(vec![0x78, 0x56, 0x34, 0x12])
        );
        assert_eq!(LabelType::I16.encode(&Value::Int(-1)), Ok(vec![0xff, 0xff]));
        assert!(LabelType::I16.encode(&Value::Int(40000)).is_err());
        assert!(LabelType::U16.encode(&Value::Uint(65536)).is_err());
        assert_eq!(
            LabelType::F64.encode(&Value::Float(-2.25)),
            Ok((-2.25f64).to_le_bytes().to_vec())
        );
        assert_eq!(
            s.encode(&Value::Bytes(b"ABCDEFG\0".to_vec())),
            Ok(b"ABCDEFG\0".to_vec())
        );
        assert!(s.encode(&Value::Bytes(b"ABC".to_vec())).is_err());
        // No NUL terminator: refused (printed p.115).
        let err = s.encode(&Value::Bytes(b"ABCDEFGH".to_vec())).unwrap_err();
        assert!(err.contains("NUL terminator"), "{err}");
        assert!(s.encode(&Value::Bytes(b"ABCDEFG\0".to_vec())).is_ok());
    }

    fn allows(list: &LabelAllowlist, text: &str) -> bool {
        list.allows(&label(text, "u16").unwrap())
    }

    #[test]
    fn allowlist_matches_exact_labels_and_last_subscript_ranges() {
        let list = LabelAllowlist::parse(Some(
            "@Tank1.Level;@Arr[0-9];@M[1,0-15];@St.A[2-4].x;@Single[7];@タンク.水位",
        ))
        .unwrap();
        for text in [
            "@Tank1.Level",
            "@Arr[0]",
            "@Arr[009]",
            "@Arr[9]",
            "@M[1,0]",
            "@M[1,15]",
            "@St.A[3].x",
            "@Single[7]",
            "@タンク.水位",
        ] {
            assert!(allows(&list, text), "{text}");
        }
        for text in [
            "@tank1.level",
            "@Tank1",
            "@Tank1.Level2",
            "@Arr[10]",
            "@Arr",
            "@Arr[1,1]",
            "@M[2,0]",
            "@M[1,16]",
            "@M[0,1,5]",
            "@St.A[3].y",
            "@St.A[3]",
            "@Single[6]",
            "@XArr[1]",
        ] {
            assert!(!allows(&list, text), "{text}");
        }
        assert!(LabelAllowlist::parse(None).unwrap().is_empty());
        assert!(LabelAllowlist::parse(Some("")).unwrap().is_empty());
    }

    #[test]
    fn malformed_allowlists_fail_closed() {
        for raw in [
            "@A;",
            ";@A",
            "@A;;@B",
            "@A;@A",
            "@A @B",
            "@A; @B",
            "@A,@B",
            "A",
            "@",
            "@Arr[03]",
            "@Arr[3-1]",
            "@Arr[01-3]",
            "@Arr[1-03]",
            "@Arr[1-]",
            "@Arr[-1]",
            "@Arr[*]",
            "@Arr*",
            "@X.3",
            "@Arr[1-2,3]",
            "@Arr[0-4294967296]",
            "@A\u{7}",
        ] {
            assert!(LabelAllowlist::parse(Some(raw)).is_err(), "{raw}");
        }
        let entries = |n: usize| {
            (0..n)
                .map(|i| format!("@L{i}"))
                .collect::<Vec<_>>()
                .join(";")
        };
        assert!(LabelAllowlist::parse(Some(&entries(32))).is_ok());
        assert!(LabelAllowlist::parse(Some(&entries(33))).is_err());
        let long = format!("@{}", "A".repeat(2047));
        assert!(LabelAllowlist::parse(Some(&long)).is_ok());
        let long = format!("@{}", "A".repeat(2048));
        assert!(LabelAllowlist::parse(Some(&long)).is_err());
        assert_eq!(
            LabelAllowlist::parse(Some("@A;@A")).unwrap_err().0,
            LABEL_ALLOWLIST_KEY
        );
    }
}
