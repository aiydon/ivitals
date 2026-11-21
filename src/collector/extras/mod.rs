//! Extension-metric pipeline: the v1 extras chosen in
//! `docs/metrics-research.md`.
//!
//! Architecture (shared by local and remote collection):
//! - each `extras/*` module owns one metric family and exposes
//!   `Raw::parse(text)` / `Raw::read_local()` plus a pure
//!   `compute(prev, cur, window)` over two raw snapshots;
//! - this module holds the cross-tick state (previous raw snapshots,
//!   sampling cadences) and assembles the per-tick [`ExtrasData`];
//! - the local collector feeds `collect_local` (reads /proc directly,
//!   microseconds), the remote collector feeds `collect_remote` (parses
//!   the section block emitted by `scripts/collect.sh` after the JSON
//!   lines);
//! - availability is *structural*: a missing section, a missing /proc
//!   file, or a first tick without baseline simply yields `None` for that
//!   family (field absent from the JSON line — never a pseudo-value,
//!   never a warn storm).

pub mod cpu_detail;
pub mod disk_io;
pub mod fs;
pub mod gpu_detail;
pub mod memory_detail;
pub mod net;
pub mod slurm;

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::ExtrasConfig;
use crate::model::{
    CpuDetail, DiskIo, Extras, Filesystem, GpuDetail, MemoryDetail, NetStat, SlurmStat,
};

pub use cpu_detail::CpuRaw;
pub use disk_io::DiskRaw;
pub use memory_detail::MemRaw;
pub use net::NetRaw;

/// Slurm sampling period. `sinfo` takes ~100-300 ms, which blows the
/// per-metric local budget if run every 5 s; 60 s matches the process
/// snapshot cadence and the queue state does not change faster than that
/// in any meaningful way.
const SLURM_PERIOD: Duration = Duration::from_secs(60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_COMMAND_OUTPUT: u64 = 4 * 1024 * 1024;

/// Which extras are enabled, derived from `[extras]` in config.toml.
/// `off` = the family is not collected at all (no field, no cost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub cpu_detail: bool,
    pub memory_detail: bool,
    pub disk_io: bool,
    pub net: bool,
    pub gpu_detail: bool,
    pub slurm: bool,
    pub psi: bool,
    pub fs_space: bool,
}

impl Flags {
    pub fn from_config(cfg: &ExtrasConfig) -> Self {
        Self {
            cpu_detail: cfg.cpu_detail,
            memory_detail: cfg.memory_detail,
            disk_io: cfg.disk_io,
            net: cfg.net,
            gpu_detail: matches!(
                cfg.gpu_detail,
                crate::config::GpuDetailMode::Auto | crate::config::GpuDetailMode::Dcgm
            ),
            slurm: matches!(cfg.slurm, crate::config::SlurmMode::Auto),
            psi: cfg.psi,
            fs_space: cfg.fs_space,
        }
    }

    pub fn any(&self) -> bool {
        self.cpu_detail
            || self.memory_detail
            || self.disk_io
            || self.net
            || self.gpu_detail
            || self.slurm
            || self.psi
            || self.fs_space
    }
}

/// One tick's extras output, ready to merge into a sample's `Extras`.
#[derive(Debug, Default)]
pub struct ExtrasData {
    pub cpu_detail: Option<CpuDetail>,
    pub memory_detail: Option<MemoryDetail>,
    pub disk_io: Option<Vec<DiskIo>>,
    pub net: Option<NetStat>,
    pub gpu_detail: Option<Vec<GpuDetail>>,
    pub slurm: Option<SlurmStat>,
    pub filesystems: Option<Vec<Filesystem>>,
    /// Flat PSI keys (`psi_cpu_some` etc.), merged into `Extras.extra`.
    pub psi: BTreeMap<String, Value>,
    /// Wall time spent producing this tick's extras (ms); the scheduler
    /// warns (rate-limited) when it exceeds the 20 ms budget.
    pub extras_ms: u32,
    /// Enabled families whose non-empty input failed to parse.
    pub parse_errors: u32,
}

impl ExtrasData {
    /// Moves this tick's data into the sample's extras container.
    pub fn apply(mut self, e: &mut Extras) {
        e.cpu_detail = self.cpu_detail;
        e.memory_detail = self.memory_detail;
        e.disk_io = self.disk_io;
        e.net = self.net;
        e.gpu_detail = self.gpu_detail;
        e.slurm = self.slurm;
        e.filesystems = self.filesystems;
        for (k, v) in std::mem::take(&mut self.psi) {
            e.extra.insert(k, v);
        }
        if e.extras_ms.is_none() {
            e.extras_ms = Some(self.extras_ms);
        }
        if e.extras_errors.is_none() && self.parse_errors > 0 {
            e.extras_errors = Some(self.parse_errors);
        }
    }
}

/// Cross-tick state for delta metrics.
#[derive(Clone, Default)]
struct PrevState {
    ts: Option<Instant>,
    cpu: Option<CpuRaw>,
    mem: Option<MemRaw>,
    disk: Option<DiskRaw>,
    net: Option<NetRaw>,
}

/// Stateful extras pipeline, one instance per collector.
pub struct ExtrasCollector {
    flags: Flags,
    prev: PrevState,
    last_slurm: Option<Instant>,
    /// Local-only Slurm probe cache: None = not probed, Some(false) =
    /// no sinfo, Some(true) = sinfo present. Probing is a single
    /// `command -v`-equivalent (~5 ms), cached for the process lifetime.
    slurm_local_available: Option<bool>,
}

/// Parsed `## section` blocks from the remote script output.
pub type Sections = std::collections::HashMap<String, String>;

impl ExtrasCollector {
    pub fn new(flags: Flags) -> Self {
        Self {
            flags,
            prev: PrevState::default(),
            last_slurm: None,
            slurm_local_available: None,
        }
    }

    /// Local path: reads the enabled /proc sources directly. `gpu` is the
    /// NVML power read for this tick (`None` when NVML is absent/error or
    /// the family is disabled — the field is then simply absent).
    pub fn collect_local(&mut self, gpu: Option<Vec<GpuDetail>>) -> ExtrasData {
        let t0 = Instant::now();
        let mut cur = PrevState {
            ts: Some(t0),
            ..Default::default()
        };
        if self.flags.cpu_detail {
            cur.cpu = CpuRaw::read_local();
        }
        if self.flags.memory_detail {
            cur.mem = MemRaw::read_local();
        }
        if self.flags.disk_io {
            cur.disk = DiskRaw::read_local();
        }
        if self.flags.net {
            cur.net = NetRaw::read_local();
        }
        let mut data = ExtrasData::default();
        if self.flags.psi {
            data.psi = read_psi();
        }
        if self.flags.fs_space {
            data.filesystems = fs::read_local();
        }
        self.compute_from(t0, &cur, &mut data);
        data.gpu_detail = gpu;
        if self.flags.slurm && self.slurm_due(t0) {
            data.slurm = self.slurm_local();
        }
        data.extras_ms = t0.elapsed().as_millis() as u32;
        self.prev = cur;
        data
    }

    /// Remote path: `sections` maps section name -> raw text as emitted by
    /// collect.sh after the JSON lines. The `## gpu_detail` /
    /// `## slurm_nodes` / `## slurm_jobs` sections are pre-parsed by the
    /// caller (see `gpu_detail::parse` / `slurm::parse` composition in
    /// `remote.rs`) and passed in as `gpu` / `slurm`.
    pub fn collect_remote(
        &mut self,
        sections: &Sections,
        gpu: Option<Vec<GpuDetail>>,
        slurm: Option<SlurmStat>,
    ) -> ExtrasData {
        let t0 = Instant::now();
        let mut cur = PrevState {
            ts: Some(t0),
            ..Default::default()
        };
        let mut parse_errors = 0u32;
        if self.flags.cpu_detail {
            if let Some(text) = sections.get("cpu_detail") {
                cur.cpu = CpuRaw::parse(text);
                if cur.cpu.is_none() && !text.trim().is_empty() {
                    parse_errors += 1;
                }
            }
        }
        if self.flags.memory_detail {
            if let Some(text) = sections.get("memory_detail") {
                cur.mem = MemRaw::parse(text);
                if cur.mem.is_none() && !text.trim().is_empty() {
                    parse_errors += 1;
                }
            }
        }
        if self.flags.disk_io {
            if let Some(text) = sections.get("diskstats") {
                cur.disk = DiskRaw::parse(text);
                if cur.disk.is_none() && !text.trim().is_empty() {
                    parse_errors += 1;
                }
            }
        }
        if self.flags.net {
            if let Some(text) = sections.get("net") {
                cur.net = NetRaw::parse(text);
                if cur.net.is_none() && !text.trim().is_empty() {
                    parse_errors += 1;
                }
            }
        }
        let mut data = ExtrasData::default();
        if self.flags.psi {
            if let Some(text) = sections.get("psi_cpu") {
                if let Some(v) = psi_value(text) {
                    data.psi.insert("psi_cpu_some".into(), Value::from(v));
                }
            }
            if let Some(text) = sections.get("psi_mem") {
                if let Some(v) = psi_value(text) {
                    data.psi.insert("psi_mem_some".into(), Value::from(v));
                }
            }
            if let Some(text) = sections.get("psi_io") {
                if let Some(v) = psi_value(text) {
                    data.psi.insert("psi_io_some".into(), Value::from(v));
                }
            }
        }
        if self.flags.fs_space {
            if let Some(text) = sections.get("filesystems") {
                data.filesystems = fs::parse_df(text);
                if data.filesystems.is_none() && !text.trim().is_empty() {
                    parse_errors += 1;
                }
            }
        }
        self.compute_from(t0, &cur, &mut data);
        data.gpu_detail = gpu;
        data.slurm = slurm;
        data.parse_errors = parse_errors;
        data.extras_ms = t0.elapsed().as_millis() as u32;
        self.prev = cur;
        data
    }

    /// Shared delta computation over the previous/cur raw snapshots.
    fn compute_from(&mut self, t0: Instant, cur: &PrevState, data: &mut ExtrasData) {
        let prev = self.prev.clone();
        let window = prev
            .ts
            .and_then(|p| t0.checked_duration_since(p))
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        if window <= 0.0 {
            return;
        }
        if self.flags.cpu_detail {
            if let (Some(pc), Some(cc)) = (&prev.cpu, &cur.cpu) {
                data.cpu_detail = cpu_detail::compute(pc, cc, window);
            }
        }
        if self.flags.memory_detail {
            if let (Some(pm), Some(cm)) = (&prev.mem, &cur.mem) {
                data.memory_detail = memory_detail::compute(pm, cm, window);
            }
        }
        if self.flags.disk_io {
            if let (Some(pd), Some(cd)) = (&prev.disk, &cur.disk) {
                data.disk_io = disk_io::compute(pd, cd, window);
            }
        }
        if self.flags.net {
            if let (Some(pn), Some(cn)) = (&prev.net, &cur.net) {
                data.net = net::compute(pn, cn, window);
            }
        }
    }

    fn slurm_due(&mut self, t0: Instant) -> bool {
        let due = match self.last_slurm {
            None => true,
            Some(last) => t0.duration_since(last) >= SLURM_PERIOD,
        };
        if due {
            self.last_slurm = Some(t0);
        }
        due
    }

    /// Local Slurm: probe once (cached), then run sinfo/squeue via the
    /// same command shapes the remote script uses. Every command has a
    /// hard deadline so a stuck slurmctld cannot wedge collection.
    fn slurm_local(&mut self) -> Option<SlurmStat> {
        let available = *self.slurm_local_available.get_or_insert_with(|| {
            let mut cmd = Command::new("sinfo");
            cmd.arg("--version");
            run_with_timeout(cmd, COMMAND_TIMEOUT)
                .map(|o| o.status.success())
                .unwrap_or(false)
        });
        if !available {
            return None;
        }
        let mut cmd = Command::new("sinfo");
        cmd.args(["-N", "-h", "-o", "%T %c %C %G"]);
        let nodes = run_with_timeout(cmd, COMMAND_TIMEOUT)?.stdout;
        let mut cmd = Command::new("squeue");
        cmd.args(["-h", "-o", "%t"]);
        let jobs = run_with_timeout(cmd, COMMAND_TIMEOUT)?.stdout;
        let nodes = String::from_utf8_lossy(&nodes).into_owned();
        let jobs = String::from_utf8_lossy(&jobs).into_owned();
        if nodes.trim().is_empty() {
            return None;
        }
        slurm::parse(&format!("{nodes}\n#jobs\n{jobs}"))
    }
}

/// Runs a command with a deadline and a bounded stdout/stderr capture.
/// `None` on spawn error, timeout, or a command that does not exit.
/// stderr is drained too, so a chatty command cannot fill the pipe and
/// deadlock the wait loop.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(out) = child.stdout.take() {
                    let _ = out.take(MAX_COMMAND_OUTPUT).read_to_end(&mut stdout);
                }
                if let Some(err) = child.stderr.take() {
                    let _ = err.take(1024 * 1024).read_to_end(&mut stderr);
                }
                return Some(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

/// Reads the three /proc/pressure files (kernel >= 4.20; absent on
/// 3.10/4.18 hosts -> empty map, fields simply missing).
fn read_psi() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (file, key) in [
        ("/proc/pressure/cpu", "psi_cpu_some"),
        ("/proc/pressure/memory", "psi_mem_some"),
        ("/proc/pressure/io", "psi_io_some"),
    ] {
        if let Some(v) = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| psi_value(&t))
        {
            out.insert(key.into(), Value::from(v));
        }
    }
    out
}

/// Extracts the `some avg10=` value from a PSI file (2 lines: some/full).
fn psi_value(text: &str) -> Option<f64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("some ") {
            for field in rest.split_whitespace() {
                if let Some(v) = field.strip_prefix("avg10=") {
                    return v.parse().ok();
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psi_value_parses_avg10() {
        assert_eq!(
            psi_value("some avg10=1.50 avg60=2.00 avg300=3.00 total=100\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"),
            Some(1.50)
        );
        assert_eq!(psi_value("full avg10=0.50\n"), None);
    }

    #[test]
    fn first_tick_yields_no_delta_families() {
        let mut ec = ExtrasCollector::new(Flags {
            cpu_detail: true,
            ..Default::default()
        });
        let data = ec.collect_local(None);
        assert!(data.cpu_detail.is_none());
        assert!(data.psi.is_empty() || !data.psi.is_empty()); // host-dependent
        assert_eq!(data.extras_ms, data.extras_ms);
    }

    #[test]
    fn flags_from_config() {
        let cfg = ExtrasConfig {
            cpu_detail: false,
            memory_detail: true,
            disk_io: false,
            net: true,
            gpu_detail: crate::config::GpuDetailMode::Off,
            slurm: crate::config::SlurmMode::Off,
            psi: false,
            fs_space: false,
        };
        let f = Flags::from_config(&cfg);
        assert!(!f.cpu_detail);
        assert!(f.memory_detail);
        assert!(!f.disk_io);
        assert!(f.net);
        assert!(!f.gpu_detail);
        assert!(!f.slurm);
        assert!(!f.psi);
        assert!(!Flags::default().any());
    }

    #[test]
    fn remote_sections_first_tick() {
        let mut ec = ExtrasCollector::new(Flags {
            cpu_detail: true,
            psi: true,
            ..Default::default()
        });
        let mut sections = Sections::new();
        sections.insert(
            "cpu_detail".into(),
            "cpu  100 0 10 500 20 0 30 40 0 0\n#loadavg\n1.0 2.0 3.0 1/1 1\n".into(),
        );
        sections.insert(
            "psi_cpu".into(),
            "some avg10=0.25 avg60=0 avg300=0 total=0\nfull avg10=0 avg60=0 avg300=0 total=0\n"
                .into(),
        );
        let data = ec.collect_remote(&sections, None, None);
        // first tick: cpu delta absent, but PSI is instant -> present
        assert!(data.cpu_detail.is_none());
        assert_eq!(
            data.psi.get("psi_cpu_some").and_then(|v| v.as_f64()),
            Some(0.25)
        );
    }
}
