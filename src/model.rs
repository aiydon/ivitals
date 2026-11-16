//! Data model: stable core schema plus flat, extensible extras.
//!
//! Design rules (see `docs/metrics-research.md`):
//! - `cpu` / `memory` / `gpu` form the stable schema: fields are never
//!   renamed or removed, so downstream analysis stays valid long-term.
//! - Every optional metric is an `Option` field skipped when `None`.
//!   Absence means "unavailable" — we never write pseudo-values like
//!   `"unavailable"` or `null`.
//! - Extras are flattened to the top level of the JSON so downstream tools
//!   (jq, DuckDB) read new metrics without changing query paths.

use std::collections::BTreeMap;

use serde::Serialize;

/// One metrics sample, one line per tick per server.
#[derive(Debug, Clone, Serialize)]
pub struct MetricSample {
    pub schema_version: u32,
    pub ts_ms: i64,
    pub server: String,
    pub mode: &'static str,
    /// Wall time spent collecting this sample (core + extras), ms.
    pub collect_ms: u32,
    /// Stable core field.
    pub cpu: CpuInfo,
    /// Stable core field.
    pub memory: MemoryInfo,
    /// Stable core field, three-state: absent / ok / error.
    pub gpu: GpuStatus,
    /// Extended metrics, flattened to the top level of the record.
    #[serde(flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CpuInfo {
    /// Global CPU busy percent over the sampling window (0-100).
    pub global_percent: f32,
    pub core_count: usize,
    pub load_avg_1m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    /// total - available (the same "used" semantics as `free`).
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// Three-state GPU status. `Absent` covers: no NVIDIA hardware, no driver,
/// remote host without nvidia-smi. `Error` covers runtime probe failures.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GpuStatus {
    Absent,
    Ok { gpus: Vec<GpuSummary> },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GpuSummary {
    pub index: u32,
    pub name: String,
    pub util_percent: u32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub temp_c: f32,
}

/// Flattened extended metrics. Each field is either present with real data
/// or absent entirely; nothing is ever written as null or a pseudo-value.
///
/// The struct is Serialize-only on purpose: the remote side deserializes
/// into an explicit payload struct (see `collector::remote`) instead of
/// relying on nested flattened deserialization.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Extras {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_detail: Option<CpuDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_detail: Option<MemoryDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_io: Option<Vec<DiskIo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<NetStat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_detail: Option<Vec<GpuDetail>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slurm: Option<SlurmStat>,
    /// Extras collection cost in ms; a flat convenience metric so the
    /// sampling cadence of each category can be tuned from data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras_ms: Option<u32>,
    /// Free-form flat metrics found during research (e.g. `psi_cpu_some`).
    /// Keys must be flat snake_case names.
    #[serde(flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// CPU state breakdown from `/proc/stat` deltas (local) or the embedded
/// script (remote).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CpuDetail {
    pub iowait_percent: f32,
    pub steal_percent: f32,
    pub softirq_percent: f32,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    /// Busy percent per logical core, aligned with `/proc/stat` order.
    pub per_core_percent: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct MemoryDetail {
    /// Page cache (`Cached` in /proc/meminfo).
    pub cache_bytes: u64,
    pub swap_used_bytes: u64,
    /// From /proc/vmstat pswpin/pswpout deltas.
    pub swap_in_bytes_per_s: u64,
    pub swap_out_bytes_per_s: u64,
    /// /proc/vmstat oom_kill counter (cumulative since boot).
    pub oom_kill_count: u64,
}

/// One device row from `/proc/diskstats` deltas. Only devices with I/O in
/// the window (or non-zero busy time) are emitted, keeping lines small on
/// hosts with many loop devices.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DiskIo {
    pub device: String,
    pub util_percent: f32,
    /// Average I/O wait (iostat "await") in ms.
    pub await_ms: f32,
    pub read_bytes_per_s: u64,
    pub write_bytes_per_s: u64,
    pub iops: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct NetStat {
    /// All interfaces except loopback, from /proc/net/dev deltas.
    pub rx_bytes_per_s: u64,
    pub tx_bytes_per_s: u64,
    /// TCP retransmission percent from /proc/net/snmp `Tcp:` line deltas
    /// (RetransSegs / (OutSegs - RetransSegs)).
    pub retrans_percent: f32,
}

/// Per-GPU efficiency fields. NVML-only fields (power) fill in without any
/// extra deployment; DCGM fields stay `None` unless DCGM is probed and
/// available (see docs/metrics-research.md).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GpuDetail {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sm_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sm_occupancy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dram_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tensor_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_w: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_reasons: Option<String>,
}

/// Slurm cluster view, only present when a slurmctld endpoint is reachable.
/// GPU fields are `Option` because the slurm GRES/gpu plugin may be absent
/// (writing 0 would be a pseudo-value).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct SlurmStat {
    pub nodes_alloc: u32,
    pub nodes_idle: u32,
    pub cpus_alloc: u32,
    pub cpus_total: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpus_alloc: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpus_total: Option<u32>,
    pub jobs_running: u32,
    pub jobs_pending: u32,
}

/// One process snapshot, one line per process-interval tick per server.
/// Serialize-only: the remote side deserializes into an explicit payload
/// (`collector::remote`) and rebuilds this struct, because `mode` is
/// `&'static str` (not deserializable from borrowed JSON).
#[derive(Debug, Clone, Serialize)]
pub struct ProcessSnapshot {
    pub schema_version: u32,
    pub ts_ms: i64,
    pub server: String,
    pub mode: &'static str,
    pub collect_ms: u32,
    pub top_n: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_procs: Option<usize>,
    pub processes: Vec<ProcRow>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProcRow {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_percent: f32,
    /// Cumulative CPU time (user+system) since process start, from
    /// /proc/[pid]/stat (local) or `ps -o time` (remote).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cum_cpu_ms: Option<u64>,
    /// Process state char from /proc/[pid]/stat: R/S/D/Z/T/...
    /// D (uninterruptible sleep) and Z (zombie) are the interesting ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<char>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_with(extras: Extras) -> MetricSample {
        MetricSample {
            schema_version: crate::SCHEMA_VERSION,
            ts_ms: 1_700_000_000_000,
            server: "test".into(),
            mode: "local",
            collect_ms: 3,
            cpu: CpuInfo {
                global_percent: 12.5,
                core_count: 16,
                load_avg_1m: 1.5,
            },
            memory: MemoryInfo {
                total_bytes: 16 << 30,
                used_bytes: 8 << 30,
                available_bytes: 8 << 30,
            },
            gpu: GpuStatus::Absent,
            extras,
        }
    }

    #[test]
    fn empty_extras_serialize_to_no_keys() {
        let json = serde_json::to_string(&sample_with(Extras::default())).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for key in [
            "cpu_detail",
            "memory_detail",
            "disk_io",
            "net",
            "gpu_detail",
            "slurm",
            "extras_ms",
            "extras",
        ] {
            assert!(v.get(key).is_none(), "unexpected key {key}");
        }
        assert!(!json.contains("null"));
        // Stable core is always present.
        assert!(v.get("cpu").is_some());
        assert!(v.get("memory").is_some());
        assert_eq!(v["gpu"]["state"], "absent");
    }

    #[test]
    fn extras_are_flattened_to_top_level() {
        let mut extra = BTreeMap::new();
        extra.insert("psi_cpu_some".to_string(), serde_json::json!(0.42));
        let extras = Extras {
            cpu_detail: Some(CpuDetail {
                iowait_percent: 1.0,
                steal_percent: 0.0,
                softirq_percent: 0.5,
                load_avg_5m: 1.0,
                load_avg_15m: 1.0,
                per_core_percent: vec![1.0, 2.0],
            }),
            gpu_detail: Some(vec![GpuDetail {
                index: 0,
                sm_active: None,
                sm_occupancy: None,
                dram_active: None,
                tensor_active: None,
                power_w: Some(120),
                throttle_reasons: None,
            }]),
            extra,
            ..Default::default()
        };
        let json = serde_json::to_string(&sample_with(extras)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Flattened: top-level, not nested under "extras".
        assert_eq!(v["cpu_detail"]["iowait_percent"], 1.0);
        assert_eq!(v["gpu_detail"][0]["power_w"], 120);
        // Option None inside GpuDetail is skipped, not null.
        assert!(v["gpu_detail"][0].get("sm_active").is_none());
        assert!(v["gpu_detail"][0].get("throttle_reasons").is_none());
        // Flat extra map keys land at top level too.
        assert_eq!(v["psi_cpu_some"], 0.42);
        assert!(v.get("extras").is_none());
        assert!(!json.contains("null"));
    }

    #[test]
    fn gpu_status_three_state_shapes() {
        let absent: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&GpuStatus::Absent).unwrap()).unwrap();
        assert_eq!(absent, serde_json::json!({"state": "absent"}));

        let ok: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&GpuStatus::Ok {
                gpus: vec![GpuSummary {
                    index: 0,
                    name: "NVIDIA L20".into(),
                    util_percent: 19,
                    mem_used_mb: 469,
                    mem_total_mb: 46068,
                    temp_c: 43.0,
                }],
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(ok["state"], "ok");
        assert_eq!(ok["gpus"][0]["name"], "NVIDIA L20");

        // Remote side deserializes the tagged enum back.
        let back: GpuStatus = serde_json::from_value(ok).unwrap();
        assert!(matches!(back, GpuStatus::Ok { .. }));
    }
}
