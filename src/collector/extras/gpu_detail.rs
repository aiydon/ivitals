//! `gpu_detail` extra: per-GPU efficiency/power fields.
//!
//! Remote side: `nvidia-smi --query-gpu=index,power.draw,fan.speed,clocks.sm,
//! clocks.mem,pstate,clocks_throttle_reasons_active --format=csv,noheader,
//! nounits` — one line per GPU. Throttle text is last and itself contains
//! commas, so only the first six commas split fields. Malformed lines are
//! skipped instead of discarding the whole family.
//!
//! The four DCGM-only fields (sm_active / sm_occupancy / dram_active /
//! tensor_active) stay `None` until DCGM is probed; NVML only exposes
//! utilization, not SM occupancy.

use crate::model::GpuDetail;

fn optional_u32(field: &str) -> Option<u32> {
    field.trim().parse::<f64>().ok().map(|v| v.round() as u32)
}

fn optional_text(field: &str) -> Option<String> {
    let s = field.trim();
    if s.is_empty() || s == "[N/A]" || s == "N/A" {
        None
    } else {
        Some(s.to_string())
    }
}

/// Parses `index,power,fan,sm_clock,mem_clock,pstate,throttle_reasons`.
pub fn parse(text: &str) -> Option<Vec<GpuDetail>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(7, ',');
        let Some(index_s) = parts.next() else {
            continue;
        };
        let Ok(index) = index_s.trim().parse::<u32>() else {
            continue;
        };
        let power_w = parts.next().and_then(optional_u32);
        let fan_percent = parts.next().and_then(optional_u32);
        let sm_clock_mhz = parts.next().and_then(optional_u32);
        let mem_clock_mhz = parts.next().and_then(optional_u32);
        let pstate = parts.next().and_then(optional_text);
        let throttle = parts.next().and_then(|s| {
            let s = s.trim();
            let inactive = s.is_empty()
                || s == "[Not Active]"
                || s == "Not Active"
                || s == "N/A"
                || s.starts_with("0x0000000000000000");
            if inactive {
                None
            } else {
                Some(
                    s.strip_prefix('[')
                        .and_then(|s| s.strip_suffix(']'))
                        .unwrap_or(s)
                        .to_string(),
                )
            }
        });
        out.push(GpuDetail {
            index,
            sm_active: None,
            sm_occupancy: None,
            dram_active: None,
            tensor_active: None,
            power_w,
            fan_percent,
            sm_clock_mhz,
            mem_clock_mhz,
            pstate,
            throttle_reasons: throttle,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Builds GpuDetail rows from an NVML power read (index, watts).
pub fn from_nvml_power(powers: &[(u32, u32)]) -> Option<Vec<GpuDetail>> {
    let out: Vec<GpuDetail> = powers
        .iter()
        .map(|(index, w)| GpuDetail {
            index: *index,
            sm_active: None,
            sm_occupancy: None,
            dram_active: None,
            tensor_active: None,
            power_w: Some(*w),
            fan_percent: None,
            sm_clock_mhz: None,
            mem_clock_mhz: None,
            pstate: None,
            throttle_reasons: None,
        })
        .collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_lines() {
        let text =
            "0, 45.2, 30, 1800, 7000, P2, [Not Active]\n1, 12.0, [N/A], 900, 5000, P8, [SW Power Cap, SW Thermal]\n";
        let v = parse(text).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 0);
        assert_eq!(v[0].power_w, Some(45));
        assert_eq!(v[0].fan_percent, Some(30));
        assert_eq!(v[0].sm_clock_mhz, Some(1800));
        assert_eq!(v[0].mem_clock_mhz, Some(7000));
        assert_eq!(v[0].pstate.as_deref(), Some("P2"));
        assert!(v[0].throttle_reasons.is_none());
        assert_eq!(v[1].fan_percent, None);
        assert_eq!(v[1].pstate.as_deref(), Some("P8"));
        assert_eq!(
            v[1].throttle_reasons.as_deref(),
            Some("SW Power Cap, SW Thermal")
        );
    }

    #[test]
    fn skips_bad_line_without_dropping_good_rows() {
        let v = parse("garbage\n0, 45.2, 30, 1800, 7000, P2, [Not Active]\n").unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].index, 0);
    }

    #[test]
    fn empty_is_none() {
        assert!(parse("").is_none());
        assert!(parse("garbage").is_none());
    }

    #[test]
    fn nvml_power_only() {
        let v = from_nvml_power(&[(0, 100)]).unwrap();
        assert_eq!(v[0].power_w, Some(100));
        assert!(v[0].throttle_reasons.is_none());
        assert!(from_nvml_power(&[]).is_none());
    }
}
