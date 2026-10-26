//! Guest-side copy of the bounded SLMP area policy grammar. The Host independently
//! validates its saved policy before handing a request to this component.

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
        if !raw.is_empty() {
            for interval in raw.split(',') {
                if ranges.len() == MAX_INTERVALS {
                    return Err((ALLOWLIST_KEY, "too many write ranges"));
                }
                let Some((start, end)) = interval.split_once('-') else {
                    return Err((ALLOWLIST_KEY, "expected same-area start-end ranges"));
                };
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
        if enabled && ranges.is_empty() {
            return Err((
                ALLOWLIST_KEY,
                "enabled write requires an explicit allowlist",
            ));
        }
        Ok(Self { enabled, ranges })
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
}
