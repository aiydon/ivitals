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
use std::net::TcpStream;
use std::time::Duration;

use ssh2::Session;

use crate::collector::{now_ms, CollectError, Collector};
use crate::model::{
    CpuInfo, Extras, GpuStatus, MemoryInfo, MetricSample, ProcRow, ProcessSnapshot,
};
use crate::SCHEMA_VERSION;

/// Embedded remote-side script (core fields; extras flags added later).
const SCRIPT: &str = include_str!("scripts/collect.sh");

pub struct RemoteCollector {
    alias: String,
    host: String,
    user: String,
    ssh_key: Option<String>,
    connect_timeout: Duration,
    exec_timeout: Duration,
    top_n: usize,
}

impl RemoteCollector {
    pub fn new(
        alias: &str,
        host: &str,
        user: &str,
        ssh_key: Option<String>,
        connect_timeout_secs: u64,
        exec_timeout_secs: u64,
        top_n: usize,
    ) -> Self {
        Self {
            alias: alias.to_string(),
            host: host.to_string(),
            user: user.to_string(),
            ssh_key,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            exec_timeout: Duration::from_secs(exec_timeout_secs),
            top_n,
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

    /// One SSH round-trip executing the embedded script.
    /// Returns the parsed core sample plus the process snapshot when
    /// `with_processes`.
    fn run_script(
        &self,
        with_processes: bool,
    ) -> Result<(MetricSample, Option<ProcessSnapshot>), CollectError> {
        let addr = format!("{}:22", self.host);
        let tcp = TcpStream::connect_timeout(
            &addr
                .parse()
                .map_err(|e| CollectError::Remote(format!("bad host {addr}: {e}")))?,
            self.connect_timeout,
        )
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

        let args = format!(
            "--alias {} --topn {}{}",
            self.alias,
            self.top_n,
            if with_processes { " --processes" } else { "" }
        );
        let cmd = format!(
            "printf '%s' '{}' | base64 -d | bash -s -- {}",
            b64_encode(SCRIPT.as_bytes()),
            args
        );
        channel
            .exec(&cmd)
            .map_err(|e| CollectError::Remote(format!("exec: {e}")))?;

        let mut out = String::new();
        channel
            .read_to_string(&mut out)
            .map_err(|e| CollectError::Remote(format!("read: {e}")))?;

        let mut lines = out.lines();
        let metrics_line = lines
            .next()
            .filter(|l| !l.trim().is_empty())
            .ok_or_else(|| CollectError::Parse("empty remote stdout".into()))?;
        let sample = parse_metrics_line(metrics_line, &self.alias)?;

        let snapshot = lines
            .next()
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_process_line(l, &self.alias))
            .transpose()?;

        Ok((sample, snapshot))
    }
}

impl Collector for RemoteCollector {
    fn metrics(&mut self) -> Result<MetricSample, CollectError> {
        let (sample, _) = self.run_script(false)?;
        Ok(sample)
    }

    fn processes(&mut self) -> Result<ProcessSnapshot, CollectError> {
        let (_, snapshot) = self.run_script(true)?;
        snapshot.ok_or_else(|| CollectError::Parse("remote: no process line".into()))
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
}
