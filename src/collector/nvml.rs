//! Local NVML GPU probe with graceful degradation.
//!
//! `nvml-wrapper`'s default `Nvml::init()` loads the *unversioned*
//! `libnvidia-ml.so`. Many driver installs only ship `libnvidia-ml.so.1`
//! (this host does), so the probe now tries the versioned soname first and
//! falls back to the default name and common absolute paths.
//!
//! Probe outcome handling:
//! - init failure / 0 devices -> `absent`, warn once per transition, retry
//!   every 5 minutes (a driver can be loaded later);
//! - runtime failure -> `error`, warn once, retry every 30 s instead of
//!   latching the error until process restart;
//! - success -> `ok`, full per-GPU util/mem/temp/power/clocks/fan/pstate.
//!
//! No per-tick warning spam: only state transitions produce a log line.

use std::ffi::OsStr;
use std::time::{Duration, Instant};

use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState, TemperatureSensor};
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::Nvml;

use crate::model::{GpuDetail, GpuProcess, GpuStatus, GpuSummary};

const ABSENT_RETRY: Duration = Duration::from_secs(300);
const ERROR_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum ProbeState {
    Unprobed,
    Ready,
    Absent { since: Instant },
    Error { since: Instant, message: String },
}

/// Long-lived NVML handle plus the cached probe outcome.
pub struct NvmlProbe {
    nvml: Option<Nvml>,
    state: ProbeState,
}

/// One tick's GPU data: the three-state core status plus the detailed
/// per-GPU extras (same probe, no duplicate NVML calls).
#[derive(Debug, Default)]
pub struct GpuSnapshot {
    pub status: Option<GpuStatus>,
    pub details: Option<Vec<GpuDetail>>,
}

impl Default for NvmlProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl NvmlProbe {
    pub fn new() -> Self {
        Self {
            nvml: None,
            state: ProbeState::Unprobed,
        }
    }

    /// Try the versioned soname before the unversioned one. This is the
    /// fix for hosts where only `libnvidia-ml.so.1` is installed.
    fn init() -> Result<Nvml, String> {
        let paths: [&str; 5] = [
            "libnvidia-ml.so.1",
            "libnvidia-ml.so",
            "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1",
            "/usr/lib64/libnvidia-ml.so.1",
            "/usr/local/cuda/compat/libnvidia-ml.so.1",
        ];
        let mut last = "no libnvidia-ml candidate".to_string();
        for path in paths {
            let mut builder = Nvml::builder();
            match builder.lib_path(OsStr::new(path)).init() {
                Ok(nvml) => return Ok(nvml),
                Err(error) => last = format!("{path}: {error}"),
            }
        }
        Err(last)
    }

    fn cached_status(&self) -> GpuStatus {
        match &self.state {
            ProbeState::Error { message, .. } => GpuStatus::Error {
                message: message.clone(),
            },
            _ => GpuStatus::Absent,
        }
    }

    /// Returns true when a cached failure should be retried now.
    fn retry_due(&self) -> bool {
        match &self.state {
            ProbeState::Absent { since, .. } => since.elapsed() >= ABSENT_RETRY,
            ProbeState::Error { since, .. } => since.elapsed() >= ERROR_RETRY,
            ProbeState::Unprobed => true,
            ProbeState::Ready => true,
        }
    }

    fn set_absent(&mut self, message: String) -> GpuStatus {
        let was_absent = matches!(self.state, ProbeState::Absent { .. });
        if !was_absent {
            log::warn!("gpu: NVML unavailable ({message}); gpu.state=absent (retrying)");
        }
        self.state = ProbeState::Absent {
            since: Instant::now(),
        };
        GpuStatus::Absent
    }

    fn set_error(&mut self, message: String) -> GpuStatus {
        let was_same =
            matches!(&self.state, ProbeState::Error { message: old, .. } if *old == message);
        if !was_same {
            log::warn!("gpu: NVML runtime failure: {message}; gpu.state=error (retrying)");
        }
        self.state = ProbeState::Error {
            since: Instant::now(),
            message: message.clone(),
        };
        GpuStatus::Error { message }
    }

    /// Three-state GPU status plus per-GPU details for this tick.
    pub fn sample(&mut self) -> GpuSnapshot {
        match &self.state {
            ProbeState::Absent { .. } | ProbeState::Error { .. } if !self.retry_due() => {
                return GpuSnapshot {
                    status: Some(self.cached_status()),
                    details: None,
                };
            }
            _ => {}
        }

        if self.nvml.is_none() {
            match Self::init() {
                Ok(nvml) => {
                    self.nvml = Some(nvml);
                    self.state = ProbeState::Ready;
                    log::info!("gpu: NVML initialized");
                }
                Err(error) => {
                    let status = self.set_absent(error.to_string());
                    return GpuSnapshot {
                        status: Some(status),
                        details: None,
                    };
                }
            }
        }

        let nvml = match self.nvml.as_ref() {
            Some(nvml) => nvml,
            None => {
                let status = self.set_absent("NVML handle missing after init".into());
                return GpuSnapshot {
                    status: Some(status),
                    details: None,
                };
            }
        };
        let count = match nvml.device_count() {
            Ok(count) => count,
            Err(error) => {
                let status = self.set_error(format!("nvmlDeviceGetCount: {error}"));
                return GpuSnapshot {
                    status: Some(status),
                    details: None,
                };
            }
        };
        if count == 0 {
            let status = self.set_absent("NVML reports 0 devices".into());
            return GpuSnapshot {
                status: Some(status),
                details: None,
            };
        }

        let mut gpus = Vec::with_capacity(count as usize);
        let mut details = Vec::with_capacity(count as usize);
        for index in 0..count {
            match Self::collect_device(nvml, index) {
                Ok((summary, detail)) => {
                    gpus.push(summary);
                    details.push(detail);
                }
                Err(error) => {
                    let status = self.set_error(format!("device {index}: {error}"));
                    return GpuSnapshot {
                        status: Some(status),
                        details: None,
                    };
                }
            }
        }
        if !matches!(self.state, ProbeState::Ready) {
            log::info!("gpu: NVML recovered ({} device(s))", gpus.len());
        }
        self.state = ProbeState::Ready;
        GpuSnapshot {
            status: Some(GpuStatus::Ok { gpus }),
            details: if details.is_empty() {
                None
            } else {
                Some(details)
            },
        }
    }

    fn collect_device(nvml: &Nvml, index: u32) -> Result<(GpuSummary, GpuDetail), NvmlError> {
        let device = nvml.device_by_index(index)?;
        let name = device.name()?;
        let utilization = device.utilization_rates()?;
        let memory = device.memory_info()?;
        let temp = device.temperature(TemperatureSensor::Gpu)?;
        let mem_used_mb = memory.used / (1024 * 1024);
        let mem_total_mb = memory.total / (1024 * 1024);
        let mem_util_percent = if memory.total > 0 {
            Some((memory.used as f64 / memory.total as f64 * 100.0) as f32)
        } else {
            None
        };
        let power_w = device.power_usage().ok().map(|mw| mw as f32 / 1000.0);
        let power_limit_w = device
            .power_management_limit()
            .ok()
            .map(|mw| mw as f32 / 1000.0);
        let fan_percent = device.fan_speed(0).ok();
        let sm_clock_mhz = device.clock_info(Clock::SM).ok();
        let mem_clock_mhz = device.clock_info(Clock::Memory).ok();
        let pstate = device.performance_state().ok().and_then(pstate_label);

        let summary = GpuSummary {
            index,
            name,
            util_percent: utilization.gpu,
            mem_used_mb,
            mem_total_mb,
            temp_c: temp as f32,
            mem_util_percent,
            power_w,
            power_limit_w,
            fan_percent,
            sm_clock_mhz,
            mem_clock_mhz,
            pstate: pstate.clone(),
        };
        let detail = GpuDetail {
            index,
            sm_active: None,
            sm_occupancy: None,
            dram_active: Some(utilization.memory as f32),
            tensor_active: None,
            power_w: power_w.map(|w| w.round() as u32),
            fan_percent,
            sm_clock_mhz,
            mem_clock_mhz,
            pstate,
            throttle_reasons: None,
        };
        Ok((summary, detail))
    }

    /// Processes currently holding GPU memory. Names come from
    /// `/proc/<pid>/comm` (NVML only exposes pids). Best-effort: a failed
    /// device/probe yields no rows instead of failing the snapshot.
    pub fn gpu_processes(&self) -> Option<Vec<GpuProcess>> {
        let nvml = self.nvml.as_ref()?;
        if !matches!(self.state, ProbeState::Ready) {
            return None;
        }
        let count = nvml.device_count().ok()?;
        let mut out = Vec::new();
        for index in 0..count {
            let Ok(device) = nvml.device_by_index(index) else {
                continue;
            };
            for (kind, rows) in [
                (
                    "compute",
                    device.running_compute_processes().unwrap_or_default(),
                ),
                (
                    "graphics",
                    device.running_graphics_processes().unwrap_or_default(),
                ),
            ] {
                for info in rows {
                    let mem_mb = match info.used_gpu_memory {
                        nvml_wrapper::enums::device::UsedGpuMemory::Used(bytes) => {
                            Some(bytes / (1024 * 1024))
                        }
                        nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => None,
                    };
                    out.push(GpuProcess {
                        gpu_index: index,
                        gpu_uuid: None,
                        pid: info.pid,
                        name: process_name(info.pid).unwrap_or_default(),
                        mem_mb,
                        kind: Some(kind.to_string()),
                    });
                }
            }
        }
        if out.is_empty() {
            None
        } else {
            out.sort_by(|a, b| {
                b.mem_mb
                    .unwrap_or(0)
                    .cmp(&a.mem_mb.unwrap_or(0))
                    .then_with(|| a.pid.cmp(&b.pid))
            });
            out.truncate(100);
            Some(out)
        }
    }
}

fn process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn pstate_label(state: PerformanceState) -> Option<String> {
    let index = match state {
        PerformanceState::Zero => 0,
        PerformanceState::One => 1,
        PerformanceState::Two => 2,
        PerformanceState::Three => 3,
        PerformanceState::Four => 4,
        PerformanceState::Five => 5,
        PerformanceState::Six => 6,
        PerformanceState::Seven => 7,
        PerformanceState::Eight => 8,
        PerformanceState::Nine => 9,
        PerformanceState::Ten => 10,
        PerformanceState::Eleven => 11,
        PerformanceState::Twelve => 12,
        PerformanceState::Thirteen => 13,
        PerformanceState::Fourteen => 14,
        PerformanceState::Fifteen => 15,
        PerformanceState::Unknown => return None,
    };
    Some(format!("P{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pstate_labels() {
        assert_eq!(pstate_label(PerformanceState::Zero).as_deref(), Some("P0"));
        assert_eq!(
            pstate_label(PerformanceState::Fifteen).as_deref(),
            Some("P15")
        );
        assert_eq!(pstate_label(PerformanceState::Unknown), None);
    }
}
