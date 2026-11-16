//! Local collector: sysinfo for core metrics, /proc for the cheap extras
//! (load average, per-process state and cumulative CPU time).

use std::time::Instant;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::collector::{now_ms, CollectError, Collector};
use crate::model::{
    CpuInfo, Extras, GpuStatus, MemoryInfo, MetricSample, ProcRow, ProcessSnapshot,
};
use crate::SCHEMA_VERSION;

/// Collects metrics from the machine ivitals runs on.
pub struct LocalCollector {
    alias: String,
    top_n: usize,
    sys: System,
}

impl LocalCollector {
    pub fn new(alias: &str, top_n: usize) -> Self {
        let mut sys = System::new();
        // Prime the CPU delta baseline and memory counters so the first
        // tick already carries meaningful values.
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            alias: alias.to_string(),
            top_n,
            sys,
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

        Ok(MetricSample {
            schema_version: SCHEMA_VERSION,
            ts_ms: now_ms(),
            server: self.alias.clone(),
            mode: "local",
            collect_ms: t0.elapsed().as_millis() as u32,
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
            gpu: GpuStatus::Absent,
            extras: Extras::default(),
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
        let mut ranked: Vec<(f32, &sysinfo::Process)> = self
            .sys
            .processes()
            .values()
            .map(|p| (p.cpu_usage(), p))
            .collect();
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.1.memory().cmp(&a.1.memory()).reverse())
        });
        ranked.truncate(self.top_n);
        let processes = ranked
            .into_iter()
            .map(|(_, p)| {
                let pid = p.pid();
                let (cum_cpu_ms, state) = read_proc_stat(pid);
                ProcRow {
                    pid: pid.as_u32(),
                    name: p.name().to_string_lossy().into_owned(),
                    cpu_percent: p.cpu_usage(),
                    mem_percent: if total_mem > 0 {
                        p.memory() as f32 / total_mem as f32 * 100.0
                    } else {
                        0.0
                    },
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
            processes,
        })
    }
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
