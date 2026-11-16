//! `disk_io` extra: per-device I/O from /proc/diskstats deltas.
//!
//! Structural filtering keeps the line small on large hosts (384-core
//! nodes also expose 300+ nvme partition rows): a device is a partition
//! when some *other listed device* is a strict prefix of its name and the
//! remainder is either all digits (`sda` → `sda1`) or `p` + digits
//! (`nvme0n1` → `nvme0n1p1`); `loop*`, `dm-*`, `sr*` are dropped. `sr0`
//! is skipped even though `/proc/diskstats` prints it (0 I/O, no whole
//! disk to attribute). Whatever survives is emitted whenever it had I/O in
//! the window or non-zero busy time.

use crate::model::DiskIo;

/// One snapshot of /proc/diskstats rows: device name + the 20 standard
/// fields (sector units for read/write; all times in 1 ms ticks).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskRaw {
    pub rows: Vec<(String, Vec<u64>)>,
}

impl DiskRaw {
    /// Parses /proc/diskstats text (20-field modern layout; short rows are
    /// padded with zeros, malformed rows skipped).
    pub fn parse(text: &str) -> Option<Self> {
        let mut rows = Vec::new();
        for line in text.lines() {
            let t: Vec<&str> = line.split_whitespace().collect();
            // `major minor name` + at least the busy-time field (1-based
            // field 11 of the 20-field modern layout).
            if t.len() < 12 {
                continue;
            }
            let name = t[2].to_string();
            let mut vals = Vec::with_capacity(20);
            for tok in &t[3..] {
                match tok.parse::<u64>() {
                    Ok(v) => vals.push(v),
                    Err(_) => break,
                }
            }
            while vals.len() < 20 {
                vals.push(0);
            }
            rows.push((name, vals));
        }
        Some(Self { rows })
    }

    /// Local read.
    pub fn read_local() -> Option<Self> {
        std::fs::read_to_string("/proc/diskstats")
            .ok()
            .map(|t| Self::parse(&t).unwrap())
    }

    pub fn names(&self) -> Vec<String> {
        self.rows.iter().map(|(n, _)| n.clone()).collect()
    }
}

fn is_partition_suffix(rem: &str) -> bool {
    rem.chars().all(|c| c.is_ascii_digit())
        || (rem.len() >= 2 && rem.starts_with('p') && rem[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Structural filter: keep whole devices, drop partitions and
/// loop/dm/sr pseudo-devices.
pub fn filter_devices(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| {
            if n.starts_with("loop") || n.starts_with("dm-") || n.starts_with("sr") {
                return false;
            }
            !names.iter().any(|p| {
                p.as_str() != n.as_str()
                    && n.starts_with(p.as_str())
                    && is_partition_suffix(&n[p.len()..])
            })
        })
        .cloned()
        .collect()
}

fn delta(cur: u64, prev: u64) -> f64 {
    (cur as i128 - prev as i128).max(0) as f64
}

/// Computes per-device DiskIo rows from two snapshots `window` seconds
/// apart. Only devices that had I/O or non-zero busy time in the window
/// are emitted.
pub fn compute(prev: &DiskRaw, cur: &DiskRaw, window: f64) -> Option<Vec<DiskIo>> {
    if window <= 0.0 {
        return None;
    }
    let keep: Vec<String> = filter_devices(&cur.names());
    let prev_by_name: std::collections::HashMap<&str, &Vec<u64>> =
        prev.rows.iter().map(|(n, v)| (n.as_str(), v)).collect();
    let mut out: Vec<DiskIo> = Vec::new();
    for name in &keep {
        let cur_vals = match cur.rows.iter().find(|(n, _)| n == name).map(|(_, v)| v) {
            Some(v) => v,
            None => continue,
        };
        let prev_vals = match prev_by_name.get(name.as_str()) {
            Some(v) => v,
            // Device appeared mid-run: no baseline -> skip this tick.
            None => continue,
        };
        // Field layout (1-based in /proc/diskstats): 3 read sectors,
        // 7 write sectors, 10 io_ticks (busy 1 ms ticks), 11 avg wait
        // ticks (r_wait + w_wait, kernel >= 4.18).
        let f = |i: usize| cur_vals[i];
        let pf = |i: usize| prev_vals[i];
        let read_sectors = delta(f(2), pf(2));
        let write_sectors = delta(f(6), pf(6));
        let io_ticks = delta(f(9), pf(9));
        let wait_ticks = delta(f(10), pf(10));
        let io_count = read_sectors + write_sectors;
        if io_count <= 0.0 && io_ticks <= 0.0 {
            continue;
        }
        // Fields 10 and 11 are 1 ms ticks: util = busy_ms/window_ms,
        // await = wait_ms/io_count.
        let util = (io_ticks / (window * 1000.0) * 100.0).min(100.0);
        let await_ms = if io_count > 0.0 {
            wait_ticks / io_count
        } else {
            0.0
        };
        out.push(DiskIo {
            device: name.clone(),
            util_percent: util as f32,
            await_ms: await_ms as f32,
            read_bytes_per_s: (read_sectors * 512.0 / window) as u64,
            write_bytes_per_s: (write_sectors * 512.0 / window) as u64,
            iops: (io_count / window) as u64,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(
        name: &str,
        read_sectors: u64,
        write_sectors: u64,
        io_ticks: u64,
        wait_ticks: u64,
    ) -> DiskRaw {
        let mut vals = vec![0u64; 20];
        vals[2] = read_sectors;
        vals[6] = write_sectors;
        vals[9] = io_ticks;
        vals[10] = wait_ticks;
        DiskRaw {
            rows: vec![(name.to_string(), vals)],
        }
    }

    #[test]
    fn rates_and_util() {
        let prev = raw("sda", 0, 0, 0, 0);
        let cur = raw("sda", 2000, 1000, 250, 100);
        // 3000 sector IOs / 5s = 600 iops; util = 250ms/5000ms = 5%
        let v = compute(&prev, &cur, 5.0).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].device, "sda");
        assert_eq!(v[0].iops, 600);
        assert!((v[0].util_percent - 5.0).abs() < 0.01);
        // 100 wait-ticks (1 ms each) over 3000 IOs -> 0.0333 ms/IO
        assert!((v[0].await_ms - 100.0 / 3000.0).abs() < 0.001);
        assert_eq!(v[0].read_bytes_per_s, 2000 * 512 / 5);
    }

    #[test]
    fn idle_device_is_omitted() {
        let prev = raw("sda", 100, 100, 10, 10);
        let cur = raw("sda", 100, 100, 10, 10);
        let v = compute(&prev, &cur, 5.0).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn partitions_and_loops_are_filtered() {
        let text = concat!(
            "  8       0 sda 2000 0 0 0 1000 0 0 0 0 0 0\n",
            "  8       1 sda1 100 0 0 0 50 0 0 0 0 0 0\n",
            "  7       0 loop0 10 0 0 0 5 0 0 0 0 0 0\n",
            " 252     0 dm-0 10 0 0 0 5 0 0 0 0 0 0\n",
            " 259     0 nvme0n1 500 0 0 0 250 0 0 0 0 0 0\n",
            " 259     1 nvme0n1p1 50 0 0 0 25 0 0 0 0 0 0\n",
            " 259     2 nvme0n1p2 50 0 0 0 25 0 0 0 0 0 0\n",
            " 259    64 nvme0c0n1 500 0 0 0 250 0 0 0 0 0 0\n",
        );
        let r = DiskRaw::parse(text).unwrap();
        let keep = filter_devices(&r.names());
        assert!(keep.contains(&"sda".to_string()));
        assert!(keep.contains(&"nvme0n1".to_string()));
        assert!(keep.contains(&"nvme0c0n1".to_string()));
        assert!(!keep.contains(&"sda1".to_string()));
        assert!(!keep.contains(&"nvme0n1p1".to_string()));
        assert!(!keep.contains(&"nvme0n1p2".to_string()));
        assert!(!keep.contains(&"loop0".to_string()));
        assert!(!keep.contains(&"dm-0".to_string()));
    }

    #[test]
    fn new_device_without_baseline_is_skipped() {
        let prev = raw("sda", 10, 10, 1, 1);
        let cur = raw("sdb", 100, 100, 10, 10);
        let v = compute(&prev, &cur, 5.0).unwrap();
        assert!(v.is_empty());
    }
}
