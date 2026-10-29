//! Guest-side copy of the bounded SLMP area policy grammar. The Host independently
//! validates its saved policy before handing a request to this component.
//!
//! Word intervals (`D100-D105`) authorize word and bit-device writes. Word-bit
//! intervals (`D100.3-D100.5`, `W1A.0-W1B.F`) are a separate list that alone
//! authorizes read-modify-write of single bits inside D/W words; a word
//! interval never authorizes a bit, and a bit interval never a word.

use banto_slmp_read_core::Device;

pub(super) const MODE_KEY: &str = "d_u16_write_mode";
pub(super) const ALLOWLIST_KEY: &str = "d_u16_write_allowlist";
const MAX_TEXT_LEN: usize = 256;
const MAX_INTERVALS: usize = 16;
const MAX_WORDS: usize = 960;
const MAX_BITS: usize = 7168;
const MAX_DEVICE: u32 = 0x00ff_ffff;

#[derive(Clone, Debug)]
pub(super) struct WritePolicy {
    enabled: bool,
    ranges: Vec<(Device, u32, u32)>,
    /// D/W word-bit intervals as inclusive `word * 16 + bit` positions.
    bit_ranges: Vec<(Device, u64, u64)>,
}

impl WritePolicy {
    pub(super) fn parse(
        mode: Option<&str>,
        allowlist: Option<&str>,
    ) -> Result<Self, (&'static str, &'static str)> {
        let enabled = match mode.unwrap_or("disabled") {
            "disabled" => false,
            "enabled" => true,
            _ => return Err((MODE_KEY, "write mode must be disabled or enabled")),
        };
        let raw = allowlist.unwrap_or("");
        if raw.len() > MAX_TEXT_LEN {
            return Err((ALLOWLIST_KEY, "write allowlist is too long"));
        }
        let mut ranges = Vec::new();
        let mut bit_ranges: Vec<(Device, u64, u64)> = Vec::new();
        if !raw.is_empty() {
            for interval in raw.split(',') {
                if ranges.len() + bit_ranges.len() == MAX_INTERVALS {
                    return Err((ALLOWLIST_KEY, "too many write ranges"));
                }
                let Some((start, end)) = interval.split_once('-') else {
                    return Err((ALLOWLIST_KEY, "expected same-area start-end ranges"));
                };
                if start.contains('.') || end.contains('.') {
                    let (device, start) = parse_word_bit(start)?;
                    let (end_device, end) = parse_word_bit(end)?;
                    if device != end_device {
                        return Err((ALLOWLIST_KEY, "write range must remain in one device area"));
                    }
                    if end < start || end - start >= MAX_BITS as u64 {
                        return Err((
                            ALLOWLIST_KEY,
                            "write range is reversed or exceeds device request limit",
                        ));
                    }
                    if device == Device::D && start / 16 <= 3001 && end / 16 >= 3000 {
                        return Err((ALLOWLIST_KEY, "D3000 and D3001 are protected"));
                    }
                    if bit_ranges
                        .iter()
                        .rev()
                        .find(|&&(area, _, _)| area == device)
                        .is_some_and(|&(_, _, last)| start <= last)
                    {
                        return Err((ALLOWLIST_KEY, "write ranges must be ordered and disjoint"));
                    }
                    bit_ranges.push((device, start, end));
                    continue;
                }
                let (device, start) = parse_device(start)?;
                let (end_device, end) = parse_device(end)?;
                if device != end_device {
                    return Err((ALLOWLIST_KEY, "write range must remain in one device area"));
                }
                let max_points = if device.is_bit() { MAX_BITS } else { MAX_WORDS };
                if end < start || end - start >= max_points as u32 {
                    return Err((
                        ALLOWLIST_KEY,
                        "write range is reversed or exceeds device request limit",
                    ));
                }
                if device == Device::D && start <= 3001 && end >= 3000 {
                    return Err((ALLOWLIST_KEY, "D3000 and D3001 are protected"));
                }
                if ranges
                    .iter()
                    .rev()
                    .find(|&&(area, _, _)| area == device)
                    .is_some_and(|&(_, _, last)| start <= last)
                {
                    return Err((ALLOWLIST_KEY, "write ranges must be ordered and disjoint"));
                }
                ranges.push((device, start, end));
            }
        }
        if enabled && ranges.is_empty() && bit_ranges.is_empty() {
            return Err((
                ALLOWLIST_KEY,
                "enabled write requires an explicit allowlist",
            ));
        }
        Ok(Self {
            enabled,
            ranges,
            bit_ranges,
        })
    }

    #[cfg(test)]
    pub(super) fn allows_span(&self, start: u32, len: usize) -> bool {
        self.allows_device_span(Device::D, start, len)
    }

    pub(super) fn allows_device_span(&self, device: Device, start: u32, len: usize) -> bool {
        if !matches!(
            device,
            Device::D | Device::W | Device::M | Device::Y | Device::L | Device::B
        ) {
            return false;
        }
        if !self.enabled || len == 0 || len > if device.is_bit() { MAX_BITS } else { MAX_WORDS } {
            return false;
        }
        let Some(end) = start.checked_add((len - 1) as u32) else {
            return false;
        };
        end <= MAX_DEVICE
            && (device != Device::D || start > 3001 || end < 3000)
            && self
                .ranges
                .iter()
                .any(|&(area, allowed_start, allowed_end)| {
                    area == device && start >= allowed_start && end <= allowed_end
                })
    }

    /// Every written bit of one D/W word must lie inside a word-bit interval.
    /// Word intervals do not count. D3000/D3001 stay protected.
    pub(super) fn allows_word_bits(&self, device: Device, number: u32, bits: &[u8]) -> bool {
        self.enabled
            && matches!(device, Device::D | Device::W)
            && !bits.is_empty()
            && number <= MAX_DEVICE
            && (device != Device::D || !(3000..=3001).contains(&number))
            && bits.iter().all(|&bit| {
                let position = u64::from(number) * 16 + u64::from(bit);
                bit < 16
                    && self.bit_ranges.iter().any(|&(area, start, end)| {
                        area == device && (start..=end).contains(&position)
                    })
            })
    }

    /// M/Y/L/B intervals only; they request the bit-count SD read in handshake.
    /// Word-bit intervals use the D/W counts and do not change the handshake.
    pub(super) fn has_bit_ranges(&self) -> bool {
        self.enabled && self.ranges.iter().any(|(device, _, _)| device.is_bit())
    }
}

fn parse_device(value: &str) -> Result<(Device, u32), (&'static str, &'static str)> {
    let (device, digits, radix) = match value.as_bytes().first() {
        Some(b'D') => (Device::D, &value[1..], 10),
        Some(b'W') => (Device::W, &value[1..], 16),
        Some(b'M') => (Device::M, &value[1..], 10),
        Some(b'Y') => (Device::Y, &value[1..], 16),
        Some(b'L') => (Device::L, &value[1..], 10),
        Some(b'B') => (Device::B, &value[1..], 16),
        _ => return Err((ALLOWLIST_KEY, "expected D/W/M/Y/L/B address")),
    };
    let digits = Some(digits)
        .filter(|digits| {
            !digits.is_empty()
                && digits.bytes().all(|ch| {
                    if radix == 10 {
                        ch.is_ascii_digit()
                    } else {
                        ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase()
                    }
                })
        })
        .ok_or((ALLOWLIST_KEY, "invalid write address"))?;
    let number = u32::from_str_radix(digits, radix)
        .map_err(|_| (ALLOWLIST_KEY, "write address is out of range"))?;
    if number > MAX_DEVICE {
        return Err((ALLOWLIST_KEY, "write address is out of range"));
    }
    Ok((device, number))
}

/// `D<decimal>.<hex>` or `W<HEX>.<hex>`; uppercase hexadecimal only. Returns
/// the inclusive bit position `word * 16 + bit`.
fn parse_word_bit(value: &str) -> Result<(Device, u64), (&'static str, &'static str)> {
    let (word, bit) = value
        .split_once('.')
        .ok_or((ALLOWLIST_KEY, "bit range ends must both be word bits"))?;
    if !matches!(word.as_bytes().first(), Some(b'D' | b'W')) {
        return Err((ALLOWLIST_KEY, "word-bit ranges require D or W"));
    }
    let (device, number) = parse_device(word)?;
    if bit.len() != 1
        || !bit
            .bytes()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase())
    {
        return Err((
            ALLOWLIST_KEY,
            "word bit must be one uppercase hexadecimal digit",
        ));
    }
    let bit = u64::from_str_radix(bit, 16).map_err(|_| (ALLOWLIST_KEY, "invalid word bit"))?;
    Ok((device, u64::from(number) * 16 + bit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_and_only_complete_intervals_are_allowed() {
        assert!(!WritePolicy::parse(None, None).unwrap().allows_span(3010, 1));
        assert!(!WritePolicy::parse(Some("disabled"), Some("D3010-D3013"))
            .unwrap()
            .allows_span(3010, 4));
        let policy = WritePolicy::parse(Some("enabled"), Some("D3010-D3013,D4000-D4009")).unwrap();
        assert!(policy.allows_span(3010, 4));
        assert!(policy.allows_span(4003, 2));
        for (start, len) in [
            (3000, 1),
            (3001, 1),
            (3002, 10),
            (3010, 5),
            (3013, 2),
            (3999, 2),
            (4000, 11),
            (3010, 0),
            (3010, 961),
        ] {
            assert!(!policy.allows_span(start, len), "{start} / {len}");
        }
    }

    #[test]
    fn one_common_setting_accepts_explicit_w_without_broadening_old_d_ranges() {
        let old = WritePolicy::parse(Some("enabled"), Some("D3010-D3013")).unwrap();
        assert!(old.allows_span(3010, 4));
        assert!(!old.allows_device_span(Device::W, 0x1a, 1));
        let mixed = WritePolicy::parse(Some("enabled"), Some("D3010-D3013,W1A-W1F")).unwrap();
        assert!(mixed.allows_span(3010, 4));
        assert!(mixed.allows_device_span(Device::W, 0x1a, 6));
        assert!(!mixed.allows_device_span(Device::W, 0x1f, 2));
        assert!(!mixed.allows_device_span(Device::M, 0, 1));
        for bad in ["W1A-D27", "W1A-W20,W1F-W21", "W1G-W1F", "w1a-w1f"] {
            assert!(
                WritePolicy::parse(Some("enabled"), Some(bad)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn bit_areas_share_enable_but_require_explicit_area_ranges() {
        let old = WritePolicy::parse(Some("enabled"), Some("D3010-D3013")).unwrap();
        assert!(!old.has_bit_ranges());
        assert!(!old.allows_device_span(Device::M, 10, 1));
        let policy = WritePolicy::parse(
            Some("enabled"),
            Some("D3010-D3013,M10-M17,Y1A-Y1F,L10-L10,B20-B21"),
        )
        .unwrap();
        assert!(policy.has_bit_ranges());
        assert!(policy.allows_device_span(Device::M, 10, 8));
        assert!(policy.allows_device_span(Device::Y, 0x1a, 6));
        assert!(policy.allows_device_span(Device::L, 10, 1));
        assert!(policy.allows_device_span(Device::B, 0x20, 2));
        assert!(!policy.allows_device_span(Device::X, 0, 1));
        assert!(!policy.allows_device_span(Device::M, 17, 2));
        let edge = WritePolicy::parse(Some("enabled"), Some("D2999-D2999")).unwrap();
        assert!(edge.allows_device_span(Device::D, 2999, 1));
        assert!(!edge.allows_device_span(Device::D, 2999, 2));
    }

    #[test]
    fn malformed_or_broad_settings_fail_closed() {
        for (mode, ranges) in [
            ("enabled", ""),
            ("enabled", "D2999-D3002"),
            ("enabled", "D3001-D3001"),
            ("enabled", "D0-D960"),
            ("enabled", "D3013-D3010"),
            ("enabled", "D3010-D3013,D3013-D3014"),
            ("enabled", "D4000-D4001,D3010-D3013"),
            ("enabled", "D3010-D3013,"),
            ("enabled", "D3010 - D3013"),
            ("enabled", "d3010-d3013"),
            ("enabled", "D16777215-D16777216"),
            ("yes", "D3010-D3013"),
        ] {
            assert!(
                WritePolicy::parse(Some(mode), Some(ranges)).is_err(),
                "{mode} / {ranges}"
            );
        }
        let seventeen = (0..17)
            .map(|n| format!("D{}-D{}", 4000 + n, 4000 + n))
            .collect::<Vec<_>>()
            .join(",");
        assert!(WritePolicy::parse(Some("enabled"), Some(&seventeen)).is_err());
        let wide = "D4000-D4000,".repeat(22);
        assert!(WritePolicy::parse(Some("enabled"), Some(&wide)).is_err());
    }

    #[test]
    fn word_bit_intervals_are_separate_from_word_intervals() {
        let policy = WritePolicy::parse(
            Some("enabled"),
            Some("D100-D105,D100.3-D100.5,W1A.0-W1B.F,D200.F-D201.1"),
        )
        .unwrap();
        assert!(policy.allows_word_bits(Device::D, 100, &[3, 4, 5]));
        assert!(!policy.allows_word_bits(Device::D, 100, &[2]));
        assert!(!policy.allows_word_bits(Device::D, 100, &[5, 6]));
        assert!(
            !policy.allows_word_bits(Device::D, 101, &[0]),
            "a word interval does not authorize bits"
        );
        assert!(policy.allows_word_bits(Device::W, 0x1a, &[0, 15]));
        assert!(policy.allows_word_bits(Device::W, 0x1b, &[15]));
        assert!(!policy.allows_word_bits(Device::W, 0x1c, &[0]));
        assert!(!policy.allows_word_bits(Device::D, 0x1a, &[0]));
        assert!(policy.allows_word_bits(Device::D, 200, &[15]));
        assert!(policy.allows_word_bits(Device::D, 201, &[0, 1]));
        assert!(!policy.allows_word_bits(Device::D, 200, &[14]));
        assert!(!policy.allows_word_bits(Device::D, 201, &[2]));
        assert!(!policy.allows_word_bits(Device::D, 100, &[]));
        assert!(!policy.allows_word_bits(Device::D, 100, &[16]));
        assert!(!policy.allows_word_bits(Device::M, 100, &[3]));
        // Bit intervals never authorize word writes, nor do they request bit SD.
        let bits_only = WritePolicy::parse(Some("enabled"), Some("D100.0-D100.F")).unwrap();
        assert!(!bits_only.allows_device_span(Device::D, 100, 1));
        assert!(!bits_only.has_bit_ranges());
        assert!(policy.allows_device_span(Device::D, 100, 6));
        let disabled = WritePolicy::parse(Some("disabled"), Some("D100.0-D100.F")).unwrap();
        assert!(!disabled.allows_word_bits(Device::D, 100, &[0]));
        // Word and bit intervals are ordered independently per device.
        assert!(WritePolicy::parse(Some("enabled"), Some("D100.0-D100.F,D50-D60")).is_ok());
        assert!(WritePolicy::parse(Some("enabled"), Some("D200-D205,D100.0-D100.1")).is_ok());
        assert!(WritePolicy::parse(Some("enabled"), Some("W1A.0-W1A.1,D5.0-D5.1")).is_ok());
        assert!(
            WritePolicy::parse(Some("enabled"), Some("D2999.0-D2999.F,D3002.0-D3002.F")).is_ok()
        );
        let max = WritePolicy::parse(Some("enabled"), Some("D0.0-D447.F")).unwrap();
        assert!(max.allows_word_bits(Device::D, 447, &[15]));
    }

    #[test]
    fn malformed_word_bit_intervals_fail_closed() {
        for ranges in [
            "D100.5-D100.3",
            "D100.3-D100.3,D100.3-D100.4",
            "D100.4-D100.5,D100.0-D100.1",
            "D100.3-D101",
            "D100-D101.3",
            "D100.3-W64.3",
            "D100.a-D100.b",
            "W1a.0-W1a.1",
            "W1A.f-W1A.f",
            "D100.10-D100.11",
            "D100.-D100.1",
            "D.1-D.2",
            "M10.0-M10.1",
            "Y1.0-Y1.1",
            "SD10.0-SD10.1",
            "D2999.0-D3000.0",
            "D3001.F-D3001.F",
            "D3000.0-D3000.0",
            "D0.0-D448.0",
            "D16777216.0-D16777216.1",
            "D100.3 - D100.5",
            "D100.3-D100.5,",
        ] {
            assert!(
                WritePolicy::parse(Some("enabled"), Some(ranges)).is_err(),
                "{ranges}"
            );
        }
        // The 16-interval cap is shared between word and bit intervals.
        let sixteen = (0..8)
            .map(|n| {
                format!(
                    "D{}-D{},D{}.0-D{}.0",
                    4000 + n,
                    4000 + n,
                    5000 + n,
                    5000 + n
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        assert!(WritePolicy::parse(Some("enabled"), Some(&sixteen)).is_ok());
        let seventeen = format!("{sixteen},D6000.0-D6000.0");
        assert!(WritePolicy::parse(Some("enabled"), Some(&seventeen)).is_err());
        let long = "D4000.0-D4000.0,".repeat(17);
        assert!(WritePolicy::parse(Some("enabled"), Some(&long)).is_err());
    }
}
