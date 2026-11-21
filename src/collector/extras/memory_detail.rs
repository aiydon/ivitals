//! `memory_detail` extra: page cache, swap behavior, OOM kills.
//!
//! Sources: /proc/meminfo (instant values) and /proc/vmstat (`pswpin`,
//! `pswpout` page counters for rates, `oom_kill` cumulative count).
//! All three vmstat keys are required; on a kernel without `oom_kill` the
//! whole `memory_detail` is absent (no pseudo-zero).

use std::collections::HashMap;

use crate::model::MemoryDetail;

/// One snapshot of the relevant /proc/meminfo and /proc/vmstat counters.
/// meminfo values are in kB (as printed), vmstat are page counts except
/// `oom_kill` which is a kill count.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemRaw {
    /// meminfo key -> kB. Only keys we use are kept.
    pub meminfo: HashMap<String, u64>,
    /// vmstat key -> value.
    pub vmstat: HashMap<String, u64>,
}

const MEMINFO_KEYS: [&str; 8] = [
    "Buffers",
    "Cached",
    "SReclaimable",
    "Dirty",
    "Writeback",
    "SwapTotal",
    "SwapFree",
    "MemTotal",
];
const VMSTAT_KEYS: [&str; 3] = ["pswpin", "pswpout", "oom_kill"];

fn parse_meminfo(text: &str) -> Option<HashMap<String, u64>> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 2 {
            let key = t[0].trim_end_matches(':');
            if MEMINFO_KEYS.contains(&key) {
                if let Ok(v) = t[1].parse::<u64>() {
                    out.insert(key.to_string(), v);
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_vmstat(text: &str) -> Option<HashMap<String, u64>> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 2 && VMSTAT_KEYS.contains(&t[0]) {
            if let Ok(v) = t[1].parse::<u64>() {
                out.insert(t[0].to_string(), v);
            }
        }
    }
    if out.len() == VMSTAT_KEYS.len() {
        Some(out)
    } else {
        None
    }
}

impl MemRaw {
    /// Parses the combined section text: `/proc/meminfo` first, then a
    /// `#vmstat` marker line, then the filtered vmstat lines.
    pub fn parse(text: &str) -> Option<Self> {
        let (mi, vs) = text.split_once("#vmstat")?;
        let meminfo = parse_meminfo(mi)?;
        let vmstat = parse_vmstat(vs)?;
        Some(Self { meminfo, vmstat })
    }

    /// Local read.
    pub fn read_local() -> Option<Self> {
        let mi = std::fs::read_to_string("/proc/meminfo").ok()?;
        let vs = std::fs::read_to_string("/proc/vmstat").ok()?;
        let vs: Vec<String> = vs
            .lines()
            .filter(|l| {
                let k = l.split_whitespace().next().unwrap_or("");
                VMSTAT_KEYS.contains(&k)
            })
            .map(|l| l.to_string())
            .collect();
        Self::parse(&format!("{mi}\n#vmstat\n{}", vs.join("\n")))
    }
}

/// Page size for the pswpin/pswpout page counters.
const PAGE_BYTES: u64 = 4096;

fn get(m: &HashMap<String, u64>, k: &str) -> Option<u64> {
    m.get(k).copied()
}

fn delta(cur: u64, prev: u64) -> f64 {
    (cur as i128 - prev as i128).max(0) as f64
}

/// Computes MemoryDetail from two snapshots `window` seconds apart.
pub fn compute(prev: &MemRaw, cur: &MemRaw, window: f64) -> Option<MemoryDetail> {
    if window <= 0.0 {
        return None;
    }
    let cache_kb = get(&cur.meminfo, "Buffers").unwrap_or(0)
        + get(&cur.meminfo, "Cached").unwrap_or(0)
        + get(&cur.meminfo, "SReclaimable").unwrap_or(0);
    let swap_total = get(&cur.meminfo, "SwapTotal")?;
    let swap_free = get(&cur.meminfo, "SwapFree")?;
    let pswpin = get(&cur.vmstat, "pswpin")?;
    let pswpout = get(&cur.vmstat, "pswpout")?;
    let oom_kill = get(&cur.vmstat, "oom_kill")?;
    let p_pswpin = get(&prev.vmstat, "pswpin")?;
    let p_pswpout = get(&prev.vmstat, "pswpout")?;

    Some(MemoryDetail {
        cache_bytes: cache_kb * 1024,
        buffers_bytes: get(&cur.meminfo, "Buffers").unwrap_or(0) * 1024,
        slab_bytes: get(&cur.meminfo, "SReclaimable").unwrap_or(0) * 1024,
        dirty_bytes: get(&cur.meminfo, "Dirty").unwrap_or(0) * 1024,
        writeback_bytes: get(&cur.meminfo, "Writeback").unwrap_or(0) * 1024,
        swap_total_bytes: swap_total * 1024,
        swap_used_bytes: swap_total.saturating_sub(swap_free) * 1024,
        swap_in_bytes_per_s: (delta(pswpin, p_pswpin) / window * PAGE_BYTES as f64) as u64,
        swap_out_bytes_per_s: (delta(pswpout, p_pswpout) / window * PAGE_BYTES as f64) as u64,
        oom_kill_count: oom_kill,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pswpin: u64, pswpout: u64, oom: u64, swapfree: u64) -> MemRaw {
        let mut m = HashMap::new();
        m.insert("Buffers".into(), 100u64);
        m.insert("Cached".into(), 200u64);
        m.insert("SReclaimable".into(), 50u64);
        m.insert("SwapTotal".into(), 1000u64);
        m.insert("SwapFree".into(), swapfree);
        m.insert("MemTotal".into(), 8192u64);
        let mut v = HashMap::new();
        v.insert("pswpin".into(), pswpin);
        v.insert("pswpout".into(), pswpout);
        v.insert("oom_kill".into(), oom);
        MemRaw {
            meminfo: m,
            vmstat: v,
        }
    }

    #[test]
    fn rates_and_instants() {
        let prev = raw(100, 50, 0, 500);
        let cur = raw(200, 150, 2, 300);
        let d = compute(&prev, &cur, 5.0).unwrap();
        assert_eq!(d.cache_bytes, 350 * 1024);
        assert_eq!(d.swap_used_bytes, 700 * 1024);
        // 100 pages in / 5s = 20 pages/s = 81920 B/s
        assert_eq!(d.swap_in_bytes_per_s, 100 * 4096 / 5);
        assert_eq!(d.swap_out_bytes_per_s, 100 * 4096 / 5);
        assert_eq!(d.oom_kill_count, 2);
    }

    #[test]
    fn missing_oom_kill_field_is_none() {
        // Older kernel: vmstat has pswp* but no oom_kill line.
        let text = "MemTotal: 8192 kB\nCached: 100 kB\n#vmstat\npswpin 10\npswpout 5\n";
        assert!(MemRaw::parse(text).is_none());
    }

    #[test]
    fn parses_combined_section() {
        let text = "MemTotal: 8192 kB\nMemAvailable: 4096 kB\nCached: 100 kB\n#vmstat\npswpin 10\npswpout 5\noom_kill 1\n";
        let r = MemRaw::parse(text).unwrap();
        assert_eq!(r.meminfo.get("MemTotal"), Some(&8192));
        assert_eq!(r.vmstat.get("oom_kill"), Some(&1));
    }
}
