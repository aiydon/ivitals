//! Local NVML GPU probe with graceful degradation.
//!
//! `nvml-wrapper` dlopens `libnvidia-ml.so.1` at runtime (via libloading),
//! so the binary builds and runs on machines without any GPU driver.
//!
//! Probe outcome handling (see goal spec, GPU three-state table):
//! - init failure (no driver / no lib)      -> `absent`, one warn, cached
//! - `device_count() == 0`                  -> `absent`, one warn, cached
//! - any runtime failure                     -> `error`, one warn, cached
//!   until process restart (a dead driver does not resurrect itself on
//!   the 5s tick; restarting ivitals re-probes)
//! - success                                 -> `ok`, re-probed every tick
//!   (NVML queries are microseconds; no caching needed)

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::Nvml;

use crate::model::{GpuStatus, GpuSummary};

#[derive(Debug, Default, PartialEq, Eq)]
enum ProbeState {
    #[default]
    Unprobed,
    Absent,
    Error(String),
}

/// Long-lived NVML handle plus the cached probe outcome.
pub struct NvmlProbe {
    nvml: Option<Nvml>,
    state: ProbeState,
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

    /// Per-GPU power draw (watts) for the `gpu_detail` extra, after
    /// `probe()` has run for this tick. `None` when NVML is absent/error
    /// (the `gpu_detail` field is then simply absent — never a
    /// pseudo-zero) or when any read fails. NVML does not expose the
    /// throttle-reason bits, so `throttle_reasons` stays `None` locally;
    /// the remote path fills it from nvidia-smi.
    pub fn gpu_detail_power(&mut self) -> Option<Vec<(u32, u32)>> {
        if matches!(self.state, ProbeState::Absent | ProbeState::Error(_)) {
            return None;
        }
        let nvml = self.nvml.as_ref()?;
        // A transient failure here only degrades this tick's gpu_detail;
        // the core probe already owns the warn.
        let count = nvml.device_count().ok()?;
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count {
            let dev = nvml.device_by_index(index).ok()?;
            let mw = dev.power_usage().ok()?;
            out.push((index, mw / 1000));
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Three-state GPU status for this tick. Never panics, never blocks
    /// for more than a couple of NVML calls.
    pub fn probe(&mut self) -> GpuStatus {
        match &self.state {
            ProbeState::Absent => return GpuStatus::Absent,
            ProbeState::Error(msg) => {
                return GpuStatus::Error {
                    message: msg.clone(),
                }
            }
            ProbeState::Unprobed => {}
        }

        let nvml = match self.nvml.as_mut() {
            Some(n) => n,
            None => match Nvml::init() {
                Ok(n) => {
                    self.nvml = Some(n);
                    self.nvml.as_mut().expect("just inserted")
                }
                Err(e) => {
                    self.state = ProbeState::Absent;
                    log::warn!("gpu: NVML unavailable ({e}); gpu.state=absent (no warn repeats)");
                    return GpuStatus::Absent;
                }
            },
        };

        let count = match nvml.device_count() {
            Ok(c) => c,
            Err(e) => return self.fail(format!("nvmlDeviceGetCount: {e}")),
        };
        if count == 0 {
            self.state = ProbeState::Absent;
            log::warn!("gpu: NVML reports 0 devices; gpu.state=absent (no warn repeats)");
            return GpuStatus::Absent;
        }

        let mut gpus = Vec::with_capacity(count as usize);
        for index in 0..count {
            match Self::collect_device(nvml, index) {
                Ok(g) => gpus.push(g),
                Err(e) => return self.fail(format!("device {index}: {e}")),
            }
        }
        GpuStatus::Ok { gpus }
    }

    fn collect_device(nvml: &Nvml, index: u32) -> Result<GpuSummary, NvmlError> {
        let device = nvml.device_by_index(index)?;
        let name = device.name()?;
        let utilization = device.utilization_rates()?.gpu;
        let memory = device.memory_info()?;
        let temp = device.temperature(TemperatureSensor::Gpu)?;
        Ok(GpuSummary {
            index,
            name,
            util_percent: utilization,
            mem_used_mb: memory.used / (1024 * 1024),
            mem_total_mb: memory.total / (1024 * 1024),
            temp_c: temp as f32,
        })
    }

    fn fail(&mut self, message: String) -> GpuStatus {
        if matches!(self.state, ProbeState::Unprobed) {
            log::warn!("gpu: NVML runtime failure: {message}; gpu.state=error (no warn repeats)");
        }
        self.state = ProbeState::Error(message.clone());
        GpuStatus::Error { message }
    }
}
