//! Guest-side copy of the bounded D/u16 policy grammar. The Host independently
//! validates its saved policy before handing a request to this component.

pub(super) const MODE_KEY: &str = "d_u16_write_mode";
pub(super) const ALLOWLIST_KEY: &str = "d_u16_write_allowlist";
const MAX_TEXT_LEN: usize = 256;
const MAX_INTERVALS: usize = 16;
const MAX_WORDS: usize = 960;
const MAX_DEVICE: u32 = 0x00ff_ffff;

#[derive(Clone, Debug)]
pub(super) struct WritePolicy {
    enabled: bool,
    ranges: Vec<(u32, u32)>,
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
                    return Err((ALLOWLIST_KEY, "expected Dstart-Dend ranges"));
                };
                let start = parse_device(start)?;
                let end = parse_device(end)?;
                if end < start || end - start >= MAX_WORDS as u32 {
                    return Err((
                        ALLOWLIST_KEY,
                        "write range is reversed or exceeds 960 words",
                    ));
                }
                if start <= 3001 && end >= 3000 {
                    return Err((ALLOWLIST_KEY, "D3000 and D3001 are protected"));
                }
                if ranges.last().is_some_and(|&(_, last)| start <= last) {
                    return Err((ALLOWLIST_KEY, "write ranges must be ordered and disjoint"));
                }
                ranges.push((start, end));
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

    pub(super) fn allows_span(&self, start: u32, len: usize) -> bool {
        if !self.enabled || len == 0 || len > MAX_WORDS {
            return false;
        }
        let Some(end) = start.checked_add((len - 1) as u32) else {
            return false;
        };
        end <= MAX_DEVICE
            && (start > 3001 || end < 3000)
            && self
                .ranges
                .iter()
                .any(|&(allowed_start, allowed_end)| start >= allowed_start && end <= allowed_end)
    }
}

fn parse_device(value: &str) -> Result<u32, (&'static str, &'static str)> {
    let digits = value
        .strip_prefix('D')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|ch| ch.is_ascii_digit()))
        .ok_or((ALLOWLIST_KEY, "expected decimal D addresses"))?;
    let number = digits
        .parse::<u32>()
        .map_err(|_| (ALLOWLIST_KEY, "write address is out of range"))?;
    if number > MAX_DEVICE {
        return Err((ALLOWLIST_KEY, "write address is out of range"));
    }
    Ok(number)
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
