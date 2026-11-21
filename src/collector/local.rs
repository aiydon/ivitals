//! Local collector: sysinfo for core metrics, /proc for the cheap extras
//! (load average, per-process state and cumulative CPU time).

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::collector::extras::{self, ExtrasCollector};
use crate::collector::nvml::NvmlProbe;
use crate::collector::{now_ms, CollectError, Collector};
use crate::model::{
    CpuInfo, Extras, GpuStatus, HostInfo, MemoryInfo, MetricSample, ProcRow, ProcessSnapshot,
};
use crate::SCHEMA_VERSION;

/// Collects metrics from the machine ivitals runs on.
pub struct LocalCollector {
    alias: String,
    top_n: usize,
    sys: System,
    nvml: NvmlProbe,
    extras: ExtrasCollector,
    users: HashMap<u32, String>,
}

impl LocalCollector {
    pub fn new(alias: &str, top_n: usize, extras_flags: extras::Flags) -> Self {
        let mut sys = System::new();
        // Prime the CPU delta baseline and memory counters so the first
        // tick already carries meaningful values.
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            alias: alias.to_string(),
            top_n,
            sys,
            nvml: NvmlProbe::new(),
            extras: ExtrasCollector::new(extras_flags),
            users: load_users(),
        }
    }
}

impl Collector for LocalCollector {
    fn metrics(&mut self) -> Result<MetricSample, CollectError> {
        let t0 = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();

        let cpus = self.sys.cpus();
        let core_count = cpus.len();
        let global_percent = if core_count > 0 {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / core_count as f32
        } else {
            0.0
        };
        let load_avg_1m = read_loadavg()?.0;
        let total_bytes = self.sys.total_memory();
        let available_bytes = self.sys.available_memory();
        let gpu_snapshot = self.nvml.sample();
        let gpu = gpu_snapshot.status.unwrap_or(GpuStatus::Absent);
        let core_ms = t0.elapsed().as_millis() as u32;

        // Extras run after the core fields so `collect_ms` stays a
        // core-only number; `extras_ms` is reported separately inside
        // the sample.
        let extras_data = self.extras.collect_local(gpu_snapshot.details);
        let mut sample_extras = Extras::default();
        extras_data.apply(&mut sample_extras);

        Ok(MetricSample {
            schema_version: SCHEMA_VERSION,
            ts_ms: now_ms(),
            server: self.alias.clone(),
            mode: "local",
            collect_ms: core_ms,
            cpu: CpuInfo {
                global_percent,
                core_count,
                load_avg_1m,
            },
            memory: MemoryInfo {
                total_bytes,
                used_bytes: total_bytes.saturating_sub(available_bytes),
                available_bytes,
            },
            gpu,
            extras: sample_extras,
        })
    }

    fn host_info(&mut self) -> Option<HostInfo> {
        let hostname = read_trim("/etc/hostname");
        let os = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_string())
            });
        let cpu_model = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("model name").map(|_| line))
                    .and_then(|line| line.split_once(':').map(|(_, v)| v.trim().to_string()))
            });
        let uptime_secs = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
            .map(|v| v as u64);
        let ts_ms = now_ms();
        let gpu_models = match self.nvml.sample().status {
            Some(GpuStatus::Ok { gpus }) => {
                let names: Vec<String> = gpus.into_iter().map(|gpu| gpu.name).collect();
                if names.is_empty() {
                    None
                } else {
                    Some(names)
                }
            }
            _ => None,
        };
        Some(HostInfo {
            schema_version: SCHEMA_VERSION,
            ts_ms,
            server: self.alias.clone(),
            mode: "local",
            hostname,
            os,
            kernel: read_trim("/proc/sys/kernel/osrelease"),
            arch: Some(std::env::consts::ARCH.to_string()),
            cpu_model,
            core_count: self.sys.cpus().len(),
            mem_total_bytes: self.sys.total_memory(),
            boot_time_ms: uptime_secs.map(|up| ts_ms.saturating_sub((up as i64) * 1000)),
            uptime_secs,
            gpu_models,
        })
    }

    fn processes(&mut self) -> Result<ProcessSnapshot, CollectError> {
        let t0 = Instant::now();
        // Refresh only what ProcRow needs; everything else stays cached.
        let total = self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let total_mem = self.sys.total_memory();
        let mut total_threads = 0u64;
        let mut state_counts: BTreeMap<String, u32> = BTreeMap::new();
        for p in self.sys.processes().values() {
            total_threads += p.tasks().map(|t| t.len() as u64 + 1).unwrap_or(1);
            *state_counts.entry(state_char(p.status())).or_default() += 1;
        }
        let mut ranked: Vec<(f32, &sysinfo::Process)> = self
            .sys
            .processes()
            .values()
            .map(|p| (p.cpu_usage(), p))
            .collect();
        ranked.sort_by(|a, b| {
            // cpu desc, then memory desc, then pid asc (stable snapshots).
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.1.memory().cmp(&a.1.memory()))
                .then_with(|| a.1.pid().cmp(&b.1.pid()))
        });
        ranked.truncate(self.top_n);
        let processes = ranked
            .into_iter()
            .map(|(_, p)| {
                let pid = p.pid();
                let (cum_cpu_ms, state) = read_proc_stat(pid);
                let meta = read_proc_meta(pid);
                let user = meta.uid.map(|uid| {
                    self.users
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| uid.to_string())
                });
                let parent_name = meta.ppid.and_then(|ppid| {
                    self.sys
                        .process(Pid::from_u32(ppid))
                        .map(|parent| parent.name().to_string_lossy().into_owned())
                });
                ProcRow {
                    pid: pid.as_u32(),
                    name: p.name().to_string_lossy().into_owned(),
                    user,
                    uid: meta.uid,
                    gid: meta.gid,
                    ppid: meta.ppid,
                    parent_name,
                    cmdline: meta.cmdline,
                    cwd: meta.cwd,
                    exe: meta.exe,
                    cpu_percent: p.cpu_usage(),
                    mem_percent: if total_mem > 0 {
                        p.memory() as f32 / total_mem as f32 * 100.0
                    } else {
                        0.0
                    },
                    rss_bytes: Some(p.memory()),
                    threads: Some(p.tasks().map(|t| t.len() as u32 + 1).unwrap_or(1)),
                    start_time_ms: Some((p.start_time() as i64).saturating_mul(1000)),
                    cum_cpu_ms,
                    state,
                }
            })
            .collect();
        Ok(ProcessSnapshot {
            schema_version: SCHEMA_VERSION,
            ts_ms: now_ms(),
            server: self.alias.clone(),
            mode: "local",
            collect_ms: t0.elapsed().as_millis() as u32,
            top_n: self.top_n,
            total_procs: Some(total),
            total_threads: Some(total_threads),
            state_counts: Some(state_counts),
            gpu_processes: self.nvml.gpu_processes(),
            processes,
        })
    }
}

struct ProcMeta {
    uid: Option<u32>,
    gid: Option<u32>,
    ppid: Option<u32>,
    cmdline: Option<String>,
    cwd: Option<String>,
    exe: Option<String>,
}

fn truncate_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

fn load_users() -> HashMap<u32, String> {
    let mut users = HashMap::new();
    if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
        for line in passwd.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 {
                if let Ok(uid) = fields[2].parse::<u32>() {
                    users.entry(uid).or_insert_with(|| fields[0].to_string());
                }
            }
        }
    }
    users
}

fn read_proc_meta(pid: Pid) -> ProcMeta {
    let mut uid = None;
    let mut gid = None;
    let mut ppid = None;
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
        for line in status.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let first = value
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u32>().ok());
            match key {
                "Uid" => uid = first,
                "Gid" => gid = first,
                "PPid" => ppid = first,
                _ => {}
            }
        }
    }
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .and_then(|bytes| {
            let text = String::from_utf8_lossy(&bytes).replace('\0', " ");
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(truncate_text(text, 512))
            }
        });
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| truncate_text(&path.to_string_lossy(), 256));
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| truncate_text(&path.to_string_lossy(), 256));
    ProcMeta {
        uid,
        gid,
        ppid,
        cmdline,
        cwd,
        exe,
    }
}

fn state_char(status: sysinfo::ProcessStatus) -> String {
    use sysinfo::ProcessStatus as S;
    match status {
        S::Idle => "I",
        S::Run => "R",
        S::Sleep => "S",
        S::Stop => "T",
        S::Zombie => "Z",
        S::Tracing => "t",
        S::Dead => "X",
        S::UninterruptibleDiskSleep => "D",
        S::Parked => "P",
        S::LockBlocked => "L",
        S::Wakekill => "K",
        S::Waking => "W",
        _ => "?",
    }
    .to_string()
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// (load1, load5, load15) from /proc/loadavg.
fn read_loadavg() -> Result<(f64, f64, f64), CollectError> {
    let text = std::fs::read_to_string("/proc/loadavg")?;
    let mut it = text.split_whitespace().take(3);
    let l1: f64 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CollectError::Parse("malformed /proc/loadavg (missing load1)".into()))?;
    let l5: f64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(l1);
    let l15: f64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(l5);
    Ok((l1, l5, l15))
}

/// Cumulative CPU time (ms) and state char from /proc/[pid]/stat.
/// The comm field (field 2) is parenthesized and may contain spaces, so
/// split after the last ')'. Field 3 = state, fields 14/15 = utime/stime
/// in USER_HZ (100) ticks.
fn read_proc_stat(pid: Pid) -> (Option<u64>, Option<char>) {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(t) => t,
        Err(_) => return (None, None),
    };
    let open = match text.find('(') {
        Some(i) => i,
        None => return (None, None),
    };
    let close = match text.rfind(')') {
        Some(i) if i > open => i,
        _ => return (None, None),
    };
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // fields[0] is file field 3 (state); fields[11]/[12] are fields 14/15.
    let state = fields.first().and_then(|s| s.chars().next());
    let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    // 100 USER_HZ ticks = 10 ms
    let cum_ms = Some(utime.saturating_add(stime) / 100 * 10);
    (cum_ms, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CPU deltas are primed in `new()`; two metrics() calls spaced apart
    /// must both yield valid samples without touching the process list.
    #[test]
    fn metrics_twice_returns_valid_samples() {
        let mut c = LocalCollector::new("test", 5, extras::Flags::default());
        let s1 = c.metrics().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s2 = c.metrics().unwrap();
        assert!(s1.cpu.core_count > 0);
        assert_eq!(s2.cpu.core_count, s1.cpu.core_count);
        assert!((0.0..=100.0).contains(&s2.cpu.global_percent));
        assert!(s1.memory.total_bytes > 0);
        assert!(s1.memory.available_bytes <= s1.memory.total_bytes);
    }

    /// Snapshot is bounded by top_n, consistent with total_procs, and
    /// sorted by cpu (then mem) descending. Called twice so the second
    /// round carries real per-process cpu deltas.
    #[test]
    fn processes_snapshot_bounded_sorted_and_consistent() {
        let mut c = LocalCollector::new("test", 5, extras::Flags::default());
        let _ = c.processes().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let snap = c.processes().unwrap();
        assert!(!snap.processes.is_empty());
        assert!(snap.processes.len() <= 5);
        let total = snap.total_procs.unwrap();
        assert!(total >= snap.processes.len());
        for w in snap.processes.windows(2) {
            assert!(
                w[0].cpu_percent > w[1].cpu_percent
                    || (w[0].cpu_percent == w[1].cpu_percent
                        && w[0].mem_percent >= w[1].mem_percent),
                "snapshot not sorted: {:?} before {:?}",
                w[0].name,
                w[1].name
            );
        }
    }
}
