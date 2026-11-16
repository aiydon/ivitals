//! Remote collector: runs the embedded `scripts/collect.sh` on the remote
//! host over SSH (ssh2) and parses its JSON output.
//!
//! The script is shipped base64-encoded in the exec command
//! (`printf '%s' '<b64>' | base64 -d | bash -s -- <args>`) so it never
//! passes through shell quoting, and stdin is never written (no
//! half-close semantics needed).
//!
//! Behavior per the goal spec:
//! - unreachable / auth failure -> Err, tick skipped (scheduler warns
//!   rate-limited);
//! - invalid JSON on stdout    -> Err, tick skipped;
//! - no password material in the config: publickey auth only — ssh-agent
//!   first, then the optional `ssh_key` file, then default `~/.ssh/id_*`
//!   (BatchMode semantics).

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use ssh2::Session;

use crate::collector::extras::{self, ExtrasCollector};
use crate::collector::{now_ms, CollectError, Collector};
use crate::model::{
    CpuInfo, Extras, GpuStatus, MemoryInfo, MetricSample, ProcRow, ProcessSnapshot,
};
use crate::SCHEMA_VERSION;

/// Embedded remote-side script; the extras section block is gated by the
/// flags passed per run (metrics runs only).
const SCRIPT: &str = include_str!("scripts/collect.sh");

/// Base64 of the embedded script, encoded once (the script is a compile-time
/// constant; re-encoding 3.6 KB on every SSH tick is pure waste).
static SCRIPT_B64: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| b64_encode(SCRIPT.as_bytes()));

/// Resolved connection target from `~/.ssh/config`.
struct SshEntry {
    host_name: String,
    port: u16,
    identity_files: Vec<String>,
}

/// One `Host` block: the host patterns it matches plus the options seen in
/// it (first occurrence of each option wins, ssh semantics).
struct SshBlock {
    hosts: Vec<String>,
    host_name: Option<String>,
    port: Option<u16>,
    identity_files: Vec<String>,
}

/// Parse `~/.ssh/config` into Host blocks. Only the option subset the
/// collector needs is kept: `HostName`, `Port`, `IdentityFile`. A host
/// matches a block iff it appears verbatim in one of the block's `Host`
/// patterns (wildcards are not expanded — this project's aliases are
/// literal); the first matching block wins, and within a block the first
/// occurrence of each option wins (ssh semantics).
fn parse_ssh_config(text: &str) -> Vec<SshBlock> {
    let mut blocks: Vec<SshBlock> = Vec::new();
    let mut cur: Option<usize> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(char::is_whitespace) {
            Some(p) => p,
            None => continue,
        };
        let key = key.to_ascii_lowercase();
        let value = value.trim();
        if key == "host" {
            blocks.push(SshBlock {
                hosts: value.split_whitespace().map(str::to_string).collect(),
                host_name: None,
                port: None,
                identity_files: Vec::new(),
            });
            cur = Some(blocks.len() - 1);
            continue;
        }
        let Some(i) = cur else {
            continue; // option before any Host line (global scope): skip
        };
        let b = &mut blocks[i];
        match key.as_str() {
            "hostname" => {
                if b.host_name.is_none() {
                    b.host_name = Some(value.to_string());
                }
            }
            "port" => {
                if b.port.is_none() {
                    b.port = value.parse().ok();
                }
            }
            "identityfile" => {
                for f in value.split_whitespace() {
                    b.identity_files.push(f.to_string());
                }
            }
            _ => {}
        }
    }
    blocks
}

/// Look up `host` in `~/.ssh/config` (read and parsed once). `None` when
/// the file is absent or no block matches — callers then use the literal
/// host on port 22.
fn ssh_config_lookup(host: &str) -> Option<SshEntry> {
    static BLOCKS: std::sync::OnceLock<Vec<SshBlock>> = std::sync::OnceLock::new();
    let blocks = BLOCKS.get_or_init(|| {
        std::env::var_os("HOME")
            .map(|h| std::path::Path::new(&h).join(".ssh").join("config"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .as_deref()
            .map(parse_ssh_config)
            .unwrap_or_default()
    });
    find_entry(blocks, host)
}

/// Resolve the connect endpoint: `~/.ssh/config` wins for the host name, an
/// explicit `ssh_port` in config.toml always wins over the config `Port`,
/// and a literal host falls back to port 22.
fn resolve_endpoint(host: &str, ssh_port: Option<u16>) -> (String, u16) {
    match ssh_config_lookup(host) {
        Some(e) => (e.host_name, ssh_port.unwrap_or(e.port)),
        None => (host.to_string(), ssh_port.unwrap_or(22)),
    }
}

/// First matching block wins; `HostName` defaults to the alias itself and
/// `Port` to 22. `IdentityFile` entries with a leading `~` are expanded
/// against `$HOME`.
fn find_entry(blocks: &[SshBlock], host: &str) -> Option<SshEntry> {
    blocks
        .iter()
        .find(|b| b.hosts.iter().any(|h| h == host))
        .map(|b| SshEntry {
            host_name: b.host_name.clone().unwrap_or_else(|| host.to_string()),
            port: b.port.unwrap_or(22),
            identity_files: b
                .identity_files
                .iter()
                .map(|f| {
                    if let Some(rest) = f.strip_prefix('~') {
                        std::env::var_os("HOME")
                            .map(|h| format!("{}{}", h.to_string_lossy(), rest))
                            .unwrap_or_else(|| f.clone())
                    } else {
                        f.clone()
                    }
                })
                .collect(),
        })
}

pub struct RemoteCollector {
    alias: String,
    host: String,
    user: String,
    ssh_key: Option<String>,
    /// Explicit port from config.toml; wins over `~/.ssh/config`.
    ssh_port: Option<u16>,
    connect_timeout: Duration,
    exec_timeout: Duration,
    top_n: usize,
    extras: ExtrasCollector,
    /// Script flags for the enabled extras families, built once.
    extras_args: String,
}

impl RemoteCollector {
    /// Flat parameters keep the call site a plain data pass-through
    /// (no config object threaded through the collector layer).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        alias: &str,
        host: &str,
        user: &str,
        ssh_key: Option<String>,
        ssh_port: Option<u16>,
        connect_timeout_secs: u64,
        exec_timeout_secs: u64,
        top_n: usize,
        extras_flags: extras::Flags,
    ) -> Self {
        Self {
            alias: alias.to_string(),
            host: host.to_string(),
            user: user.to_string(),
            ssh_key,
            ssh_port,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            exec_timeout: Duration::from_secs(exec_timeout_secs),
            top_n,
            extras: ExtrasCollector::new(extras_flags),
            extras_args: build_extras_args(&extras_flags),
        }
    }

    /// No password in the config: publickey auth only (BatchMode semantics).
    /// Order: ssh-agent, then the configured key file, then the default
    /// ~/.ssh/id_* files.
    fn authenticate(&self, session: &Session) -> Result<(), CollectError> {
        if session.userauth_agent(&self.user).is_ok() && session.authenticated() {
            return Ok(());
        }
        let mut candidates: Vec<String> = Vec::new();
        if let Some(key) = &self.ssh_key {
            candidates.push(key.clone());
        }
        // IdentityFile entries from ~/.ssh/config (the ssh CLI honors them).
        if let Some(entry) = ssh_config_lookup(&self.host) {
            for f in &entry.identity_files {
                if std::path::Path::new(f).is_file() {
                    candidates.push(f.clone());
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
                let p = std::path::Path::new(&home).join(".ssh").join(name);
                if p.is_file() {
                    candidates.push(p.to_string_lossy().into_owned());
                }
            }
        }
        for key in &candidates {
            let _ = session.userauth_pubkey_file(&self.user, None, std::path::Path::new(key), None);
            if session.authenticated() {
                return Ok(());
            }
        }
        Err(CollectError::Remote(format!(
            "auth as {}@{} failed (tried ssh-agent + {} key file(s)); configure ssh_key \
             or load the key into the ssh-agent",
            self.user,
            self.host,
            candidates.len()
        )))
    }

    /// One SSH round-trip executing the embedded script; returns the raw
    /// stdout (JSON lines, then the optional `## section` extras block).
    /// `with_extras` appends the enabled extras flags (metrics runs only —
    /// process runs re-collecting them would double the remote cost).
    fn run_script(&self, with_processes: bool, with_extras: bool) -> Result<String, CollectError> {
        let (host, port) = resolve_endpoint(&self.host, self.ssh_port);
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| CollectError::Remote(format!("resolve {host}:{port}: {e}")))?
            .next()
            .ok_or_else(|| CollectError::Remote(format!("resolve {host}:{port}: no addresses")))?;
        let tcp = TcpStream::connect_timeout(&addr, self.connect_timeout)
            .map_err(|e| CollectError::Remote(format!("connect {addr}: {e}")))?;
        tcp.set_nodelay(true).ok();

        let mut session =
            Session::new().map_err(|e| CollectError::Remote(format!("session: {e}")))?;
        session.set_timeout(self.connect_timeout.as_millis() as u32);
        session.set_tcp_stream(tcp);
        session
            .handshake()
            .map_err(|e| CollectError::Remote(format!("handshake {addr}: {e}")))?;

        self.authenticate(&session)?;

        session.set_timeout(self.exec_timeout.as_millis() as u32);
        let mut channel = session
            .channel_session()
            .map_err(|e| CollectError::Remote(format!("channel: {e}")))?;

        let mut args = format!("--alias {} --topn {}", self.alias, self.top_n);
        if with_processes {
            args.push_str(" --processes");
        }
        if with_extras && !self.extras_args.is_empty() {
            args.push(' ');
            args.push_str(&self.extras_args);
        }
        let cmd = format!(
            "printf '%s' '{}' | base64 -d | bash -s -- {}",
            SCRIPT_B64.as_str(),
            args
        );
        channel
            .exec(&cmd)
            .map_err(|e| CollectError::Remote(format!("exec: {e}")))?;

        let mut out = String::new();
        channel
            .read_to_string(&mut out)
            .map_err(|e| CollectError::Remote(format!("read: {e}")))?;

        Ok(out)
    }
}

/// Splits the script output into (JSON record lines, extras sections).
/// Sections start at a `## <name>` header line and continue until the
/// next header; JSON records are the non-empty lines before any header
/// (the script emits the metrics line first, then optionally the
/// processes line, then the section block).
fn split_output(out: &str) -> (Vec<String>, extras::Sections) {
    let mut json = Vec::new();
    let mut sections = extras::Sections::new();
    let mut current: Option<String> = None;
    for line in out.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            current = Some(name.trim().to_string());
            continue;
        }
        match &current {
            Some(name) => {
                let e = sections.entry(name.clone()).or_default();
                e.push_str(line);
                e.push('\n');
            }
            None => {
                if !line.trim().is_empty() {
                    json.push(line.to_string());
                }
            }
        }
    }
    (json, sections)
}

/// Builds the script flag string for the enabled extras families.
fn build_extras_args(f: &extras::Flags) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if f.cpu_detail {
        parts.push("--cpu-detail");
    }
    if f.memory_detail {
        parts.push("--memory-detail");
    }
    if f.disk_io {
        parts.push("--disk-io");
    }
    if f.net {
        parts.push("--net");
    }
    if f.gpu_detail {
        parts.push("--gpu-detail");
    }
    if f.slurm {
        parts.push("--slurm");
    }
    if f.psi {
        parts.push("--psi");
    }
    parts.join(" ")
}

impl Collector for RemoteCollector {
    fn metrics(&mut self) -> Result<MetricSample, CollectError> {
        let out = self.run_script(false, true)?;
        let (json, sections) = split_output(&out);
        let line = json
            .first()
            .ok_or_else(|| CollectError::Parse("empty remote stdout".into()))?;
        let mut sample = parse_metrics_line(line, &self.alias)?;
        // gpu_detail / slurm sections are pre-parsed here (fixed formats),
        // the rest flow through the shared ExtrasCollector pipeline.
        let gpu = sections
            .get("gpu_detail")
            .and_then(|t| extras::gpu_detail::parse(t));
        let slurm = match (sections.get("slurm_nodes"), sections.get("slurm_jobs")) {
            (Some(nodes), Some(jobs)) => extras::slurm::parse(&format!("{nodes}\n#jobs\n{jobs}")),
            _ => None,
        };
        let data = self.extras.collect_remote(&sections, gpu, slurm);
        data.apply(&mut sample.extras);
        Ok(sample)
    }

    fn processes(&mut self) -> Result<ProcessSnapshot, CollectError> {
        let out = self.run_script(true, false)?;
        let (json, _sections) = split_output(&out);
        // The script always emits the core metrics line first; the
        // processes line is second.
        let line = json
            .get(1)
            .ok_or_else(|| CollectError::Parse("remote: no process line".into()))?;
        parse_process_line(line, &self.alias)
    }
}

#[derive(serde::Deserialize)]
struct RemoteMetricsLine {
    schema_version: u32,
    ts_ms: i64,
    server: String,
    mode: String,
    collect_ms: u32,
    cpu: CpuInfo,
    memory: MemoryInfo,
    gpu: GpuStatus,
}

#[derive(serde::Deserialize)]
struct RemoteProcessLine {
    top_n: usize,
    total_procs: usize,
    processes: Vec<ProcRow>,
}

fn parse_metrics_line(line: &str, alias: &str) -> Result<MetricSample, CollectError> {
    let raw: RemoteMetricsLine = serde_json::from_str(line).map_err(|e| {
        CollectError::Parse(format!("metrics: {e} (line: {})", truncate(line, 200)))
    })?;
    if raw.mode != "remote" {
        return Err(CollectError::Parse(format!(
            "unexpected mode {:?}",
            raw.mode
        )));
    }
    Ok(MetricSample {
        schema_version: raw.schema_version,
        ts_ms: raw.ts_ms,
        server: if raw.server.is_empty() {
            alias.to_string()
        } else {
            raw.server
        },
        mode: "remote",
        collect_ms: raw.collect_ms,
        cpu: raw.cpu,
        memory: raw.memory,
        gpu: raw.gpu,
        extras: Extras::default(),
    })
}

fn parse_process_line(line: &str, alias: &str) -> Result<ProcessSnapshot, CollectError> {
    let raw: RemoteProcessLine = serde_json::from_str(line).map_err(|e| {
        CollectError::Parse(format!("processes: {e} (line: {})", truncate(line, 200)))
    })?;
    Ok(ProcessSnapshot {
        schema_version: SCHEMA_VERSION,
        ts_ms: now_ms(),
        server: alias.to_string(),
        mode: "remote",
        collect_ms: 0,
        top_n: raw.top_n,
        total_procs: Some(raw.total_procs),
        processes: raw.processes,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Standard base64, no external crate (30 lines beats a dependency here).
fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_entry_defaults_port_to_22_and_hostname_to_alias() {
        let blocks = parse_ssh_config("Host plainbox\n    User nobody\n");
        let entry = find_entry(&blocks, "plainbox").expect("alias resolves");
        assert_eq!(entry.host_name, "plainbox");
        assert_eq!(entry.port, 22);
        let explicit = parse_ssh_config("Host tunbox\n    HostName 10.0.0.9\n    Port 7105\n");
        let entry = find_entry(&explicit, "tunbox").expect("alias resolves");
        assert_eq!((entry.host_name.as_str(), entry.port), ("10.0.0.9", 7105));
    }

    #[test]
    fn parses_metrics_line_from_script() {
        // Exact shape emitted by scripts/collect.sh (core fields only).
        let line = r#"{"schema_version":1,"ts_ms":1794817677936,"server":"test","mode":"remote","collect_ms":225,"cpu":{"global_percent":9.6,"core_count":16,"load_avg_1m":1.89},"memory":{"total_bytes":16089788416,"used_bytes":10493288448,"available_bytes":5596499968},"gpu":{"state":"absent"}}"#;
        let sample = parse_metrics_line(line, "test").unwrap();
        assert_eq!(sample.server, "test");
        assert_eq!(sample.mode, "remote");
        assert_eq!(sample.cpu.core_count, 16);
        assert!(matches!(sample.gpu, GpuStatus::Absent));
        // Flattened extras: none present.
        let json = serde_json::to_string(&sample).unwrap();
        assert!(!json.contains("cpu_detail"));
    }

    #[test]
    fn parses_gpu_ok_line() {
        let line = r#"{"schema_version":1,"ts_ms":1,"server":"math2","mode":"remote","collect_ms":30,"cpu":{"global_percent":1.0,"core_count":384,"load_avg_1m":1.0},"memory":{"total_bytes":100,"used_bytes":50,"available_bytes":50},"gpu":{"state":"ok","gpus":[{"index":0,"name":"NVIDIA L20","util_percent":19,"mem_used_mb":469,"mem_total_mb":46068,"temp_c":43.0}]}}"#;
        let sample = parse_metrics_line(line, "math2").unwrap();
        match sample.gpu {
            GpuStatus::Ok { gpus } => {
                assert_eq!(gpus.len(), 1);
                assert_eq!(gpus[0].name, "NVIDIA L20");
                assert_eq!(gpus[0].util_percent, 19);
            }
            other => panic!("expected ok, got {other:?}"),
        }
    }

    #[test]
    fn parses_process_line() {
        let line = r#"{"top_n":5,"total_procs":488,"processes":[{"pid":12709,"name":"firefox-esr","cpu_percent":55.5,"mem_percent":6.3,"state":"S","cum_cpu_ms":8806000}]}"#;
        let snap = parse_process_line(line, "test").unwrap();
        assert_eq!(snap.total_procs, Some(488));
        assert_eq!(snap.processes[0].state, Some('S'));
        assert_eq!(snap.processes[0].cum_cpu_ms, Some(8806000));
        assert_eq!(snap.mode, "remote");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_metrics_line("nonsense", "x").is_err());
        assert!(parse_metrics_line(
            r#"{"schema_version":1,"ts_ms":1,"server":"x","mode":"local","collect_ms":1,"cpu":{"global_percent":1.0,"core_count":1,"load_avg_1m":1.0},"memory":{"total_bytes":1,"used_bytes":1,"available_bytes":0},"gpu":{"state":"absent"}}"#,
            "x"
        )
        .is_err()); // wrong mode
        assert!(parse_process_line("{}", "x").is_err());
    }

    #[test]
    fn b64_roundtrip_matches_std() {
        // Cross-check the hand-rolled encoder against the system base64.
        let sample = SCRIPT.as_bytes();
        let ours = b64_encode(sample);
        let mut cmd = std::process::Command::new("base64");
        cmd.args(["-w", "0"]);
        use std::io::Write as _;
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(sample).unwrap();
        let out = child.wait_with_output().unwrap();
        let std_out = String::from_utf8(out.stdout).unwrap();
        assert_eq!(ours, std_out.trim_end());
    }

    #[test]
    fn ssh_config_resolves_alias_hostname_and_port() {
        let cfg = "# a comment\n\
                   Host bot\n\
                       HostName frp.iydon.top\n\
                       Port 7105\n\
                       User sinotruk\n\
                   Host math2 amax2\n\
                       HostName 10.16.122.147\n";
        let blocks = parse_ssh_config(cfg);
        assert_eq!(blocks.len(), 2);
        // First matching block wins; multiple patterns per Host line.
        let bot = blocks
            .iter()
            .find(|b| b.hosts.iter().any(|h| h == "bot"))
            .unwrap();
        assert_eq!(bot.host_name.as_deref(), Some("frp.iydon.top"));
        assert_eq!(bot.port, Some(7105));
        let math2 = blocks
            .iter()
            .find(|b| b.hosts.iter().any(|h| h == "math2"))
            .unwrap();
        assert_eq!(math2.host_name.as_deref(), Some("10.16.122.147"));
        assert_eq!(math2.port, None); // default 22 applied by lookup
        assert!(blocks.iter().all(|b| b.identity_files.is_empty()));
    }

    #[test]
    fn ssh_config_first_option_wins_and_identity_files_collected() {
        let cfg = "Host dev\n\
                   IdentityFile ~/.ssh/id_ed25519 /tmp/other.key\n\
                   Port 2222\n\
                   Port 2223\n\
                   HostName first.example\n\
                   HostName second.example\n";
        let blocks = parse_ssh_config(cfg);
        let dev = &blocks[0];
        assert_eq!(dev.port, Some(2222));
        assert_eq!(dev.host_name.as_deref(), Some("first.example"));
        assert_eq!(
            dev.identity_files,
            vec!["~/.ssh/id_ed25519", "/tmp/other.key"]
        );
    }

    #[test]
    fn ssh_config_bad_port_yields_none_not_error() {
        let cfg = "Host x\nHostName h\nPort notanumber\n";
        let blocks = parse_ssh_config(cfg);
        assert_eq!(blocks[0].port, None);
    }

    #[test]
    fn ssh_config_options_before_host_are_skipped() {
        let cfg = "ServerAliveInterval 60\nHost a\nHostName h1\n";
        let blocks = parse_ssh_config(cfg);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].hosts, vec!["a"]);
    }

    #[test]
    fn ssh_config_lookup_defaults_and_missing_host() {
        let blocks = parse_ssh_config("Host a\nHost b\nHostName b.example\nPort 2222\n");
        // Matched block without explicit HostName/Port: alias + 22.
        let a = find_entry(&blocks, "a").unwrap();
        assert_eq!(a.host_name, "a");
        assert_eq!(a.port, 22);
        // Matched block with both: values from the config.
        let b = find_entry(&blocks, "b").unwrap();
        assert_eq!(b.host_name, "b.example");
        assert_eq!(b.port, 2222);
        // No matching block: None (caller uses the literal host).
        assert!(find_entry(&blocks, "nope").is_none());
        assert!(find_entry(&[], "a").is_none());
    }
}
