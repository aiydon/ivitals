//! no-GPU smoke: run the real binary on a machine without an NVIDIA GPU
//! (skipped when /proc/driver/nvidia exists) and assert the spec contract:
//!
//! - the process runs for multiple ticks without panicking and exits 0 on
//!   SIGINT (graceful shutdown, well under 1 s);
//! - the NVML warn appears exactly once, never repeated;
//! - every metrics line has `gpu.state == "absent"` and no `gpu_detail`
//!   field (missing, not a pseudo-value);
//! - `data/` holds only `YYYY-MM-DD` directories with
//!   `{alias}-{kind}.jsonl` files, every line JSON-parseable;
//! - process lines are far fewer than metric lines (split sampling).

use std::time::{Duration, Instant};

fn has_nvidia_driver() -> bool {
    std::path::Path::new("/proc/driver/nvidia").exists()
}

#[test]
fn no_gpu_run_is_stable_and_absent() {
    if has_nvidia_driver() {
        eprintln!("SKIP: this machine has an NVIDIA driver; no-GPU smoke not applicable");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("ivitals-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let data_dir = tmp.join("data");
    let cfg_path = tmp.join("config.toml");
    std::fs::write(
        &cfg_path,
        format!(
            "schema_version = 2\n\
             [global]\n\
             interval_secs = 2\n\
             process_interval_secs = 4\n\
             top_n_processes = 5\n\
             retention_days = 3\n\
             data_dir = \"{data_dir}\"\n\
             log_level = \"info\"\n\
             [[servers]]\n\
             alias = \"local\"\n\
             mode = \"local\"\n",
            data_dir = data_dir.display(),
        ),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_ivitals");
    let mut child = std::process::Command::new(bin)
        .env("IVITALS_CONFIG", &cfg_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn ivitals");

    // Let it run several ticks (interval 2s, ~7s -> 3-4 metric rounds).
    std::thread::sleep(Duration::from_secs(7));

    // SIGINT via /bin/kill (the child must take the graceful path, exit 0).
    let pid = child.id();
    std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("failed to send SIGINT");

    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                panic!("ivitals did not exit within 2s of SIGINT");
            }
        }
    };
    assert!(status.success(), "ivitals exit status: {status}");

    // Drain output (child is dead; the pipes hold what it wrote).
    let output = child.wait_with_output().unwrap();
    let log_text = String::from_utf8_lossy(&output.stderr);

    // 1) The NVML warn appears exactly once across all ticks.
    let nvml_warns = log_text
        .lines()
        .filter(|l| l.contains("NVML unavailable"))
        .count();
    assert_eq!(nvml_warns, 1, "NVML warn count != 1\n{log_text}");

    // 2) No panic anywhere.
    assert!(!log_text.contains("panic"), "panic in log\n{log_text}");

    // 3) Layout: data/ has only YYYY-MM-DD dirs; files are {alias}-{kind}.jsonl.
    let mut metrics_lines = 0;
    let mut processes_lines = 0;
    let date_re = |name: &str| {
        let b = name.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && (0..10).all(|i| i == 4 || i == 7 || b[i].is_ascii_digit())
    };
    for entry in std::fs::read_dir(&data_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy().into_owned();
        assert!(
            entry.path().is_dir() && date_re(&name),
            "unexpected non-date entry in data/: {name}"
        );
        for f in std::fs::read_dir(entry.path()).unwrap() {
            let f = f.unwrap();
            let fname = f.file_name().to_string_lossy().into_owned();
            assert!(
                fname == "local-metrics.jsonl" || fname == "local-processes.jsonl",
                "unexpected file name: {fname}"
            );
            for line in std::fs::read_to_string(f.path()).unwrap().lines() {
                let v: serde_json::Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("line not valid JSON in {fname}: {line}: {e}"));
                if fname == "local-metrics.jsonl" {
                    metrics_lines += 1;
                    assert_eq!(v["gpu"]["state"], "absent", "gpu.state not absent: {line}");
                    assert!(
                        v.get("gpu_detail").is_none(),
                        "gpu_detail must be missing, not present: {line}"
                    );
                } else {
                    processes_lines += 1;
                }
            }
        }
    }
    assert!(metrics_lines >= 3, "too few metric lines: {metrics_lines}");
    assert!(
        processes_lines < metrics_lines,
        "split sampling violated: {processes_lines} process lines vs {metrics_lines} metric lines"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
