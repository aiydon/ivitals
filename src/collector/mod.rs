//! Collectors: one per configured server, local or remote.

pub mod extras;
pub mod local;
pub mod nvml;
pub mod remote;

use crate::model::{MetricSample, ProcessSnapshot};

/// Error types for a single collection attempt. A failure never takes the
/// daemon down: the scheduler logs (rate-limited) and skips the tick for
/// that server.
#[derive(Debug)]
pub enum CollectError {
    Io(std::io::Error),
    Parse(String),
    Remote(String),
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectError::Io(e) => write!(f, "io error: {e}"),
            CollectError::Parse(msg) => write!(f, "parse error: {msg}"),
            CollectError::Remote(msg) => write!(f, "remote error: {msg}"),
        }
    }
}

impl std::error::Error for CollectError {}

impl From<std::io::Error> for CollectError {
    fn from(e: std::io::Error) -> Self {
        CollectError::Io(e)
    }
}

/// Both calls are synchronous and blocking; the scheduler invokes them via
/// `spawn_blocking`. Instances are long-lived and reused across ticks so
/// CPU-usage deltas and (later) NVML handles stay warm.
pub trait Collector: Send {
    fn metrics(&mut self) -> Result<MetricSample, CollectError>;
    fn processes(&mut self) -> Result<ProcessSnapshot, CollectError>;
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
