//! `gpu_detail` extra: per-GPU efficiency/power fields.
//!
//! Remote side: `nvidia-smi --query-gpu=index,power.draw,clocks_throttle_reasons_active
//! --format=csv,noheader,nounits` — one line per GPU. Power parses as a
//! fractional watt value; the throttle string is `"[Not Active]"` when
//! nothing is throttling (-> `None`, absence not a pseudo-value).
//!
//! The four DCGM-only fields (sm_active / sm_occupancy / dram_active /
//! tensor_active) stay `None` in v1 on every host: DCGM is absent on all
//! four hosts (see docs/metrics-research.md §6), and their DCGM metric
//! IDs are pinned in that report for the v2 dcgmi probe.
//!
//! Local side: the NVML probe supplies power directly (no subprocess);
//! throttle reason bits are not exposed by NVML cleanly, so local
//! `throttle_reasons` is `None`.

use crate::model::GpuDetail;

/// Parses nvidia-smi CSV lines `index,power_w,throttle_reasons`. The
/// throttle string itself contains commas (`"SW Power Cap, SW Thermal"`),
/// so only the first two commas split fields; the rest is throttle text.
pub fn parse(text: &str) -> Option<Vec<GpuDetail>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, ',');
        let Some(index_s) = parts.next() else {
            continue;
        };
        let Some(power_s) = parts.next() else {
            continue;
        };
        let index: u32 = index_s.trim().parse().ok()?;
        let power_w: Option<u32> = power_s.trim().parse::<f64>().ok().map(|w| w.round() as u32);
        let throttle = parts.next().and_then(|s| {
            let s = s.trim();
            if s.is_empty() || s == "[Not Active]" || s == "Not Active" {
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
        let text = "0, 45.2, [Not Active]\n1, 12.0, [SW Power Cap, SW Thermal]\n";
        let v = parse(text).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 0);
        assert_eq!(v[0].power_w, Some(45));
        assert!(v[0].throttle_reasons.is_none());
        assert_eq!(v[0].sm_active, None);
        assert_eq!(v[1].power_w, Some(12));
        assert_eq!(
            v[1].throttle_reasons.as_deref(),
            Some("SW Power Cap, SW Thermal")
        );
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
