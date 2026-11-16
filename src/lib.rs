//! ivitals — config-driven resident resource monitoring daemon.
//!
//! Collects resource metrics and process snapshots from a local host and SSH
//! remote hosts on a fixed interval, writing date-partitioned JSONL files.
//! No web UI, no alerting, no hot reload: record data only.
//!
//! Modules are introduced incrementally:
//! - [`config`] — TOML configuration schema, path resolution, validation
//! - [`model`] — stable core schema plus flat, extensible extras
//! - [`collector`] — local (sysinfo/NVML) and remote (ssh2 + embedded script)
//! - [`storage`] — single-writer thread, UTC date-partitioned files, retention
//! - [`scheduler`] — two-worker tokio tick loop with rate-limited warnings

/// Data schema version written into every record.
pub const SCHEMA_VERSION: u32 = 1;

pub mod config;
