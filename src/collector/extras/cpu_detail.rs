//! `cpu_detail` extra: CPU state breakdown from `/proc/stat` deltas.
//!
//! The raw side is a snapshot of the per-CPU tick counters (global line +
//! one line per logical core) plus load averages. `compute` takes two
//! snapshots and the real elapsed window between them; it is a pure
//! function shared by the local collector (reads /proc directly) and the
//! remote collector (parses the section emitted by collect.sh).

use crate::model::CpuDetail;

/// One snapshot of /proc/stat CPU counters + /proc/loadavg.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuRaw {
    /// Global line ticks: user, nice, system, idle, iowait, irq, softirq,
    /// steal, guest, guest_nice.
    pub global: [u64; 10],
    /// Per-core ticks, same layout, in /proc/stat order.
    pub cores: Vec<[u64; 10]>,
    /// (load1, load5, load15).
    pub load: [f64; 3],
}

fn parse_ticks(tokens: &[&str]) -> Option<[u64; 10]> {
    let mut out = [0u64; 10];
    for (slot, tok) in tokens.iter().take(10).enumerate() {
        out[slot] = tok.parse().ok()?;
    }
    Some(out)
}

impl CpuRaw {
    /// Parses the combined section text: `/proc/stat` CPU lines, then a
    /// `#loadavg` marker line, then the `/proc/loadavg` line.
    pub fn parse(text: &str) -> Option<Self> {
        let mut global = None;
        let mut cores: Vec<[u64; 10]> = Vec::new();
        let mut load = [0.0f64; 3];
        let mut after_load_marker = false;
        for line in text.lines() {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.is_empty() {
                continue;
            }
            if t[0] == "#loadavg" {
                after_load_marker = true;
                continue;
            }
            if after_load_marker {
                load = [
                    t.first().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    t.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0),
                    t.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0),
                ];
                break;
            }
            if t[0] == "cpu" {
                global = parse_ticks(&t[1..]);
            } else if let Some(rest) = t[0].strip_prefix("cpu") {
                if rest.chars().all(|c| c.is_ascii_digit()) {
                    if let Some(g) = parse_ticks(&t[1..]) {
                        cores.push(g);
                    }
                }
            }
        }
        let global = global?;
        Some(Self {
            global,
            cores,
            load,
        })
    }

    /// Local read.
    pub fn read_local() -> Option<Self> {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let loadavg = std::fs::read_to_string("/proc/loadavg").ok()?;
        Self::parse(&format!("{stat}\n#loadavg\n{loadavg}"))
    }
}

fn delta(cur: u64, prev: u64) -> f64 {
    // Counters only move forward; a wrap/reboot between samples yields a
    // negative delta which we clamp to 0 rather than emit a bogus percent.
    (cur as i128 - prev as i128).max(0) as f64
}

fn percentile(sorted: &[f32], pct: u32) -> Option<f32> {
    if sorted.is_empty() {
        return None;
    }
    let idx = (sorted.len() * pct as usize).div_ceil(100) - 1;
    Some(sorted[idx.min(sorted.len() - 1)])
}

/// Computes CpuDetail from two snapshots `window` seconds apart. The full
/// per-core vector and its distribution are recorded every tick; values
/// are rounded to 2 decimals so a 384-core line stays manageable.
pub fn compute(prev: &CpuRaw, cur: &CpuRaw, window: f64) -> Option<CpuDetail> {
    if window <= 0.0 {
        return None;
    }
    let total_delta = delta(cur.global.iter().sum(), prev.global.iter().sum());
    let (iowait, steal, softirq) = if total_delta <= 0.0 {
        // Counter regression (e.g. reboot between samples): clamp to zero
        // rather than emit a negative percentage.
        (0.0, 0.0, 0.0)
    } else {
        (
            delta(cur.global[4], prev.global[4]) / total_delta * 100.0,
            delta(cur.global[7], prev.global[7]) / total_delta * 100.0,
            delta(cur.global[6], prev.global[6]) / total_delta * 100.0,
        )
    };

    let mut busy: Vec<f32> = Vec::with_capacity(cur.cores.len());
    for (ci, c) in cur.cores.iter().enumerate() {
        let Some(p) = prev.cores.get(ci) else {
            // Topology changed mid-run (hotplug): skip this core.
            continue;
        };
        let dt = delta(c.iter().sum(), p.iter().sum());
        if dt <= 0.0 {
            continue;
        }
        let idle = delta(c[3], p[3]) + delta(c[4], p[4]);
        let value = ((dt - idle) / dt * 100.0).clamp(0.0, 100.0);
        busy.push((value as f32 * 100.0).round() / 100.0);
    }

    if busy.is_empty() {
        return Some(CpuDetail {
            iowait_percent: iowait as f32,
            steal_percent: steal as f32,
            softirq_percent: softirq as f32,
            load_avg_5m: cur.load[1],
            load_avg_15m: cur.load[2],
            per_core_percent: None,
            per_core_min_percent: None,
            per_core_mean_percent: None,
            per_core_p50_percent: None,
            per_core_max_percent: None,
            per_core_p95_percent: None,
            cores_over_50: None,
            cores_over_90: None,
            busy_histogram: None,
        });
    }

    let n = busy.len();
    let sum: f32 = busy.iter().sum();
    let min = busy.iter().copied().fold(f32::MAX, f32::min);
    let max = busy.iter().copied().fold(0.0f32, f32::max);
    let mut sorted = busy.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&sorted, 50);
    let p95 = percentile(&sorted, 95);
    let over50 = busy.iter().filter(|&&v| v > 50.0).count() as u32;
    let over90 = busy.iter().filter(|&&v| v > 90.0).count() as u32;
    let mut histogram = vec![0u32; 10];
    for &value in &busy {
        let bin = ((value / 10.0).floor() as usize).min(9);
        histogram[bin] += 1;
    }

    Some(CpuDetail {
        iowait_percent: iowait as f32,
        steal_percent: steal as f32,
        softirq_percent: softirq as f32,
        load_avg_5m: cur.load[1],
        load_avg_15m: cur.load[2],
        per_core_percent: Some(busy),
        per_core_min_percent: Some(min),
        per_core_mean_percent: Some(sum / n as f32),
        per_core_p50_percent: p50,
        per_core_max_percent: Some(max),
        per_core_p95_percent: p95,
        cores_over_50: Some(over50),
        cores_over_90: Some(over90),
        busy_histogram: Some(histogram),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(total: u64, idle: u64, iowait: u64, steal: u64, softirq: u64) -> CpuRaw {
        // user = total - (idle + iowait + steal + softirq + 1 system)
        let user = total - idle - iowait - steal - softirq - 1;
        CpuRaw {
            global: [user, 0, 1, idle, iowait, 0, softirq, steal, 0, 0],
            cores: Vec::new(),
            load: [1.0, 2.0, 3.0],
        }
    }

    #[test]
    fn deltas_percentages() {
        let prev = raw(1000, 900, 0, 0, 0);
        let cur = raw(2000, 1800, 100, 20, 30);
        // total delta 1000; iowait 100 -> 10%, steal 20 -> 2%, softirq 30 -> 3%
        let d = compute(&prev, &cur, 5.0).unwrap();
        assert!((d.iowait_percent - 10.0).abs() < 0.01);
        assert!((d.steal_percent - 2.0).abs() < 0.01);
        assert!((d.softirq_percent - 3.0).abs() < 0.01);
        assert_eq!(d.load_avg_5m, 2.0);
        assert!(d.per_core_percent.is_none());
        assert!(d.per_core_max_percent.is_none());
    }

    #[test]
    fn counter_regression_is_clamped() {
        // Reboot between samples: counters jump down -> no negative percents.
        let prev = raw(9_000_000, 8_000_000, 0, 0, 0);
        let cur = raw(500, 450, 10, 5, 5);
        let d = compute(&prev, &cur, 5.0).unwrap();
        assert_eq!(d.iowait_percent, 0.0);
        assert_eq!(d.steal_percent, 0.0);
    }

    #[test]
    fn parses_stat_and_loadavg() {
        let text = "cpu  100 0 10 500 20 0 30 40 0 0\ncpu0 50 0 5 250 10 0 15 20 0 0\ncpu1 50 0 5 250 10 0 15 20 0 0\n#loadavg\n1.50 2.50 3.50 2/100 42\n";
        let r = CpuRaw::parse(text).unwrap();
        assert_eq!(r.global[0], 100);
        assert_eq!(r.global[7], 40);
        assert_eq!(r.cores.len(), 2);
        assert_eq!(r.load, [1.5, 2.5, 3.5]);
    }

    #[test]
    fn per_core_scalars() {
        let mut prev = CpuRaw {
            global: [0; 10],
            cores: vec![[100, 0, 0, 900, 0, 0, 0, 0, 0, 0]; 4],
            load: [0.0; 3],
        };
        let mut cur = CpuRaw {
            global: [0; 10],
            cores: Vec::new(),
            load: [0.0; 3],
        };
        // core 0: 100% busy, core 1: 50%, core 2: 0%, core 3: 60%
        // (cur totals must exceed the 1000-tick prev totals for a valid delta)
        let cores = [
            [2000, 0, 1000, 0, 0, 0, 0, 0, 0, 0],
            [1000, 0, 100, 1900, 0, 0, 0, 0, 0, 0],
            [100, 0, 0, 2000, 0, 0, 0, 0, 0, 0],
            [600, 0, 100, 1300, 0, 0, 0, 0, 0, 0],
        ];
        cur.cores = cores.to_vec();
        // keep globals consistent-ish (not asserted)
        prev.global = [0; 10];
        cur.global = [100, 0, 0, 100, 0, 0, 0, 0, 0, 0];
        let d = compute(&prev, &cur, 5.0).unwrap();
        let v = d.per_core_percent.as_ref().unwrap();
        assert_eq!(v.len(), 4);
        assert!((v[0] - 100.0).abs() < 0.1);
        assert!((v[1] - 50.0).abs() < 0.1);
        assert!((v[3] - 60.0).abs() < 0.1);
        assert!((d.per_core_max_percent.unwrap() - 100.0).abs() < 0.1);
        assert_eq!(d.cores_over_50.unwrap(), 2);
        assert_eq!(d.cores_over_90.unwrap(), 1);
        assert!(d.per_core_p95_percent.is_some());
        assert_eq!(d.per_core_min_percent, Some(0.0));
        assert!(d.per_core_mean_percent.unwrap() > 50.0);
        assert_eq!(d.per_core_p50_percent, Some(50.0));
        let histogram = d.busy_histogram.unwrap();
        assert_eq!(histogram.len(), 10);
        assert_eq!(histogram.iter().sum::<u32>(), 4);
    }
}
