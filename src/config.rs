//! TOML configuration: schema, path resolution, startup validation.
//!
//! Zero CLI arguments by design — everything the daemon does is driven by
//! `config.toml` (see README). `[extras]` fields all have defaults: missing
//! keys behave exactly like the shipped `config.toml`.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// One configured server (local or SSH remote).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub alias: String,
    pub mode: Mode,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub ssh_user: Option<String>,
    /// Private key file for publickey auth. When omitted, ivitals tries the
    /// ssh-agent first, then probes ~/.ssh/id_ed25519|id_ecdsa|id_rsa.
    #[serde(default)]
    pub ssh_key: Option<String>,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_exec_timeout")]
    pub exec_timeout_secs: u64,
}

fn default_connect_timeout() -> u64 {
    5
}
fn default_exec_timeout() -> u64 {
    15
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Local,
    Remote,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Local => "local",
            Mode::Remote => "remote",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuDetailMode {
    /// Collect whatever is free (NVML-derived); no paid probing.
    #[default]
    Auto,
    /// Explicit opt-in to the costly DCGM path (dedicated refresh thread).
    Dcgm,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlurmMode {
    /// Probe for slurmctld; collect only when reachable.
    #[default]
    Auto,
    Off,
}

/// Optional extended metrics. Every field has a default, so a missing
/// `[extras]` section means: all proc-based extras on, gpu/slurm auto.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ExtrasConfig {
    pub cpu_detail: bool,
    pub memory_detail: bool,
    pub disk_io: bool,
    pub net: bool,
    pub gpu_detail: GpuDetailMode,
    pub slurm: SlurmMode,
}

impl Default for ExtrasConfig {
    fn default() -> Self {
        Self {
            cpu_detail: true,
            memory_detail: true,
            disk_io: true,
            net: true,
            gpu_detail: GpuDetailMode::Auto,
            slurm: SlurmMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Global {
    pub interval_secs: u64,
    pub process_interval_secs: u64,
    pub top_n_processes: usize,
    pub retention_days: u64,
    pub data_dir: String,
    pub log_level: String,
}

impl Default for Global {
    fn default() -> Self {
        Self {
            interval_secs: 5,
            process_interval_secs: 60,
            top_n_processes: 20,
            retention_days: 30,
            data_dir: "data".into(),
            log_level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub global: Global,
    #[serde(default)]
    pub extras: ExtrasConfig,
    #[serde(default)]
    pub servers: Vec<Server>,
}

impl Config {
    /// Load from the resolved config path and validate.
    pub fn load() -> Result<Self> {
        let path = resolve_config_path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Startup validation; any failure is fatal (better to die at boot).
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != crate::SCHEMA_VERSION {
            bail!(
                "unsupported schema_version {} (expected {})",
                self.schema_version,
                crate::SCHEMA_VERSION
            );
        }
        if self.global.interval_secs < 2 {
            bail!(
                "interval_secs must be >= 2, got {}",
                self.global.interval_secs
            );
        }
        if self.global.process_interval_secs < self.global.interval_secs {
            bail!(
                "process_interval_secs ({}) must be >= interval_secs ({})",
                self.global.process_interval_secs,
                self.global.interval_secs
            );
        }
        if self.global.top_n_processes == 0 {
            bail!("top_n_processes must be >= 1");
        }
        if self.global.retention_days == 0 {
            bail!("retention_days must be >= 1");
        }
        if self.global.data_dir.trim().is_empty() {
            bail!("data_dir must not be empty");
        }
        if self.servers.is_empty() {
            bail!("at least one [[servers]] entry is required");
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.servers {
            if !seen.insert(s.alias.as_str()) {
                bail!("duplicate server alias: {}", s.alias);
            }
            if !s
                .alias
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                bail!(
                    "alias {} must match [a-zA-Z0-9_-]+ (used in file names)",
                    s.alias
                );
            }
            if s.mode == Mode::Remote {
                if s.ssh_host.as_deref().unwrap_or("").is_empty() {
                    bail!("remote server {} requires ssh_host", s.alias);
                }
                if s.ssh_user.as_deref().unwrap_or("").is_empty() {
                    bail!("remote server {} requires ssh_user", s.alias);
                }
            }
        }
        Ok(())
    }
}

/// Resolve the config path: `$IVITALS_CONFIG` > `$PWD/config.toml` >
/// `<exe dir>/config.toml`. The final fallback is the $PWD name so error
/// messages name the expected location.
pub fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("IVITALS_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let cwd = PathBuf::from("config.toml");
    if cwd.is_file() {
        return cwd;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("config.toml");
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        r#"
schema_version = 1

[global]
interval_secs = 5
process_interval_secs = 60
top_n_processes = 20
retention_days = 30
data_dir = "data"
log_level = "info"

[[servers]]
alias = "local"
mode = "local"

[[servers]]
alias = "math2"
mode = "remote"
ssh_host = "math2"
ssh_user = "monitor"
"#
        .to_string()
    }

    #[test]
    fn parses_and_validates_sample() {
        let cfg: Config = toml::from_str(&sample()).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.servers.len(), 2);
        assert!(cfg.extras.cpu_detail);
        assert_eq!(cfg.extras.gpu_detail, GpuDetailMode::Auto);
    }

    #[test]
    fn extras_defaults_when_section_missing() {
        let stripped: String = sample()
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.starts_with('#')
                    && !matches!(
                        t,
                        "cpu_detail = true"
                            | "memory_detail = true"
                            | "disk_io = true"
                            | "net = true"
                            | "gpu_detail = \"auto\""
                            | "slurm = \"auto\""
                    )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cfg: Config = toml::from_str(&stripped).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.extras, ExtrasConfig::default());
    }

    #[test]
    fn rejects_bad_intervals() {
        let bad = sample().replace("interval_secs = 5", "interval_secs = 1");
        let cfg: Config = toml::from_str(&bad).unwrap();
        assert!(cfg.validate().is_err());

        let bad2 = sample().replace("process_interval_secs = 60", "process_interval_secs = 3");
        let cfg: Config = toml::from_str(&bad2).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_aliases_and_missing_ssh_fields() {
        let mut cfg: Config = toml::from_str(&sample()).unwrap();
        cfg.servers.push(cfg.servers[0].clone());
        assert!(cfg.validate().is_err());

        let bad = sample().replace("ssh_user = \"monitor\"", "ssh_user = \"\"");
        let cfg: Config = toml::from_str(&bad).unwrap();
        assert!(cfg.validate().is_err());
    }
}
