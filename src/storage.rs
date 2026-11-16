//! Single-writer storage: one OS thread drains the record channel and
//! appends JSONL lines to `data/{YYYY-MM-DD}/{alias}-{kind}.jsonl`.
//!
//! Guarantees (goal spec §8):
//! - `data/` holds only `YYYY-MM-DD` directories; files live inside them;
//! - the date is derived from each record's own `ts_ms` (UTC), so crossing
//!   UTC midnight flushes and closes the old day's files and opens the new
//!   day's — a single writer thread owns all open files;
//! - flushes every 30 s or every 200 records, whichever comes first;
//! - when the channel closes (Ctrl-C drops the sender) the writer drains
//!   the backlog, flushes everything, and exits;
//! - a disk write error is logged with a running count; ten consecutive
//!   failures stop the writer (main observes the dead join handle).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::DateTime;
use chrono::Utc;

use crate::model::Record;

const FLUSH_INTERVAL: Duration = Duration::from_secs(30);
const FLUSH_RECORDS: u64 = 200;
const MAX_CONSECUTIVE_FAILURES: u32 = 10;
const PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// UTC date directory name (`YYYY-MM-DD`) for a record timestamp.
///
/// Pure function of `ts_ms`, unit-testable without a clock. Out-of-range
/// timestamps fall back to the Unix epoch day.
pub fn utc_date_str(ts_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts_ms)
        .unwrap_or(DateTime::UNIX_EPOCH)
        .format("%Y-%m-%d")
        .to_string()
}

/// Strict `YYYY-MM-DD` directory-name parser: exactly 10 chars, digits in
/// the right places, and a real calendar date. Anything else is not ours.
fn parse_date_dir(name: &str) -> Option<chrono::NaiveDate> {
    let b = name.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    for (i, c) in b.iter().enumerate() {
        if i != 4 && i != 7 && !c.is_ascii_digit() {
            return None;
        }
    }
    chrono::NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()
}

/// Deletes whole `{data_dir}/{YYYY-MM-DD}` directories whose date is
/// strictly older than `now - retention_days` (UTC).
///
/// Only directories whose name is a strict, valid date are candidates;
/// everything else (other directories, stray files) is left untouched.
/// Returns the number of directories removed. `now` is injected so the
/// policy is unit-testable without a clock.
pub fn prune(
    data_dir: &std::path::Path,
    retention_days: u32,
    now: DateTime<Utc>,
) -> io::Result<u32> {
    let cutoff = now.date_naive() - chrono::Duration::days(retention_days as i64);
    let mut removed = 0u32;
    if !data_dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(data_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(date) = parse_date_dir(&name) else {
            continue;
        };
        if date < cutoff && entry.path().is_dir() {
            match fs::remove_dir_all(entry.path()) {
                Ok(()) => {
                    removed += 1;
                    log::info!("retention: removed expired date dir {name}");
                }
                Err(e) => log::warn!("retention: failed to remove {name}: {e}"),
            }
        }
    }
    Ok(removed)
}

struct OpenFile {
    date: String,
    writer: BufWriter<File>,
}

struct Storage {
    data_dir: PathBuf,
    retention_days: u32,
    /// (alias, kind) -> file currently open, tagged with the UTC date it
    /// was opened for so day changes force a roll.
    files: HashMap<(String, &'static str), OpenFile>,
    since_flush: Instant,
    unflushed: u64,
    fail_streak: u32,
}

impl Storage {
    fn new(data_dir: PathBuf, retention_days: u32) -> Self {
        Self {
            data_dir,
            retention_days,
            files: HashMap::new(),
            since_flush: Instant::now(),
            unflushed: 0,
            fail_streak: 0,
        }
    }

    fn prune(&self) {
        if let Err(e) = prune(&self.data_dir, self.retention_days, Utc::now()) {
            log::warn!("retention prune failed: {e}");
        }
    }

    fn path(&self, date: &str, alias: &str, kind: &str) -> PathBuf {
        self.data_dir
            .join(date)
            .join(format!("{alias}-{kind}.jsonl"))
    }

    /// File for this record, opened on first use and rolled when the
    /// record's UTC date differs from the date the file was opened under.
    fn file(
        &mut self,
        alias: &str,
        kind: &'static str,
        date: &str,
    ) -> io::Result<&mut BufWriter<File>> {
        let key = (alias.to_string(), kind);
        let stale = match self.files.get(&key) {
            Some(f) => f.date != date,
            None => true,
        };
        if stale {
            // Drop closes (and flushes) the old day's file.
            self.files.remove(&key);
            fs::create_dir_all(self.data_dir.join(date))?;
            let f = File::options()
                .create(true)
                .append(true)
                .open(self.path(date, alias, kind))?;
            self.files.insert(
                key.clone(),
                OpenFile {
                    date: date.to_string(),
                    writer: BufWriter::new(f),
                },
            );
        }
        Ok(&mut self.files.get_mut(&key).unwrap().writer)
    }

    fn write(&mut self, rec: &Record) -> io::Result<()> {
        let date = utc_date_str(rec.ts_ms());
        let alias = rec.alias();
        let kind = rec.kind();
        let f = self.file(alias, kind, &date)?;
        let mut line = serde_json::to_string(rec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
        self.unflushed += 1;
        if self.unflushed >= FLUSH_RECORDS {
            self.flush_all();
        }
        Ok(())
    }

    fn flush_all(&mut self) {
        for (alias, f) in self.files.iter_mut() {
            if let Err(e) = f.writer.flush() {
                log::error!("flush failed for {alias:?}: {e}");
            }
        }
        self.since_flush = Instant::now();
        self.unflushed = 0;
    }
}

fn run(rx: mpsc::Receiver<Record>, data_dir: PathBuf, retention_days: u32) {
    let mut store = Storage::new(data_dir, retention_days);
    store.prune();
    let mut last_prune = Instant::now();
    'drain: loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(rec) => {
                if let Err(e) = store.write(&rec) {
                    store.fail_streak += 1;
                    log::error!(
                        "write failed ({}/{} in a row): {e}",
                        store.fail_streak,
                        MAX_CONSECUTIVE_FAILURES
                    );
                    if store.fail_streak >= MAX_CONSECUTIVE_FAILURES {
                        log::error!(
                            "giving up on storage after {MAX_CONSECUTIVE_FAILURES} consecutive failures"
                        );
                        break 'drain;
                    }
                } else {
                    store.fail_streak = 0;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break 'drain,
        }
        if store.since_flush.elapsed() >= FLUSH_INTERVAL {
            store.flush_all();
        }
        if last_prune.elapsed() >= PRUNE_INTERVAL {
            last_prune = Instant::now();
            store.prune();
        }
    }
    // Final drain: flush everything so the last line on disk is complete.
    store.flush_all();
}

/// Spawns the writer thread. `rx` is the receiver half of the storage
/// channel; dropping the sender ends the thread after a final flush.
pub fn spawn_writer(
    rx: mpsc::Receiver<Record>,
    data_dir: PathBuf,
    retention_days: u32,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("ivitals-writer".into())
        .spawn(move || run(rx, data_dir, retention_days))
        .expect("failed to spawn writer thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Date derivation: mid-day, the 23:59:59/00:00:01 UTC midnight pair,
    /// and out-of-range fallback.
    #[test]
    fn utc_date_str_derives_from_timestamp() {
        assert_eq!(utc_date_str(1_794_821_040_000), "2026-11-16");
        assert_eq!(utc_date_str(1_794_873_599_000), "2026-11-16"); // 23:59:59
        assert_eq!(utc_date_str(1_794_873_601_000), "2026-11-17"); // 00:00:01
        assert_eq!(utc_date_str(-1), "1969-12-31"); // 1 ms before the epoch
        assert_eq!(utc_date_str(0), "1970-01-01");
        assert_eq!(utc_date_str(i64::MAX), "1970-01-01"); // out of range -> epoch day
    }

    use crate::model::{
        CpuInfo, Extras, GpuStatus, MemoryInfo, MetricSample, ProcRow, ProcessSnapshot,
    };
    use crate::SCHEMA_VERSION;

    fn sample(server: &str, ts_ms: i64) -> MetricSample {
        MetricSample {
            schema_version: SCHEMA_VERSION,
            ts_ms,
            server: server.into(),
            mode: "local",
            collect_ms: 1,
            cpu: CpuInfo {
                global_percent: 1.0,
                core_count: 8,
                load_avg_1m: 0.1,
            },
            memory: MemoryInfo {
                total_bytes: 1000,
                used_bytes: 100,
                available_bytes: 900,
            },
            gpu: GpuStatus::Absent,
            extras: Extras::default(),
        }
    }

    fn snapshot(server: &str, ts_ms: i64) -> ProcessSnapshot {
        ProcessSnapshot {
            schema_version: SCHEMA_VERSION,
            ts_ms,
            server: server.into(),
            mode: "local",
            collect_ms: 1,
            top_n: 5,
            total_procs: Some(1),
            processes: vec![ProcRow {
                pid: 1,
                name: "init".into(),
                cpu_percent: 0.5,
                mem_percent: 0.1,
                cum_cpu_ms: Some(10),
                state: Some('S'),
            }],
        }
    }

    /// End-to-end writer contract: records land in
    /// `{data}/{utc date of ts_ms}/{alias}-{kind}.jsonl`, one line each,
    /// parseable as the record's own JSON (no wrapper), and a second UTC
    /// day rolls into its own directory.
    #[test]
    fn writer_partitions_by_utc_date_and_alias() {
        let dir = std::env::temp_dir().join(format!("ivitals-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let (tx, rx) = mpsc::channel();
        let handle = spawn_writer(rx, dir.clone(), 30);
        // Day 2026-11-16 (UTC)
        tx.send(Record::Metrics(Box::new(sample(
            "math2",
            1_794_821_040_000,
        ))))
        .unwrap();
        tx.send(Record::Processes(snapshot("math2", 1_794_821_040_000)))
            .unwrap();
        tx.send(Record::Metrics(Box::new(sample("amax", 1_794_821_045_000))))
            .unwrap();
        // Same server next UTC day -> new directory.
        tx.send(Record::Metrics(Box::new(sample(
            "math2",
            1_794_873_601_000,
        ))))
        .unwrap();
        drop(tx);
        handle.join().unwrap();

        let d1 = dir.join("2026-11-16");
        let d2 = dir.join("2026-11-17");
        let m1 = fs::read_to_string(d1.join("math2-metrics.jsonl")).unwrap();
        let p1 = fs::read_to_string(d1.join("math2-processes.jsonl")).unwrap();
        let a1 = fs::read_to_string(d1.join("amax-metrics.jsonl")).unwrap();
        let m2 = fs::read_to_string(d2.join("math2-metrics.jsonl")).unwrap();

        let m1: Vec<serde_json::Value> = m1
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0]["ts_ms"], 1_794_821_040_000_i64);
        assert_eq!(m1[0]["server"], "math2");
        assert_eq!(m1[0]["gpu"]["state"], "absent");
        assert!(m1[0].get("cpu_detail").is_none()); // no pseudo-values

        let p1: Vec<serde_json::Value> = p1
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0]["processes"][0]["pid"], 1);

        assert_eq!(a1.lines().count(), 1);
        let m2: Vec<serde_json::Value> = m2
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0]["ts_ms"], 1_794_873_601_000_i64);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A record whose UTC day is *earlier* than the one currently open
    /// (out-of-order arrival) must reopen the earlier day's file in append
    /// mode instead of appending to the wrong day.
    #[test]
    fn writer_reopens_earlier_day() {
        let dir = std::env::temp_dir().join(format!("ivitals-rewrite-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let (tx, rx) = mpsc::channel();
        let handle = spawn_writer(rx, dir.clone(), 30);
        tx.send(Record::Metrics(Box::new(sample("a", 1_794_821_040_000))))
            .unwrap(); // d1
        tx.send(Record::Metrics(Box::new(sample("a", 1_794_873_601_000))))
            .unwrap(); // d2
        tx.send(Record::Metrics(Box::new(sample("a", 1_794_821_050_000))))
            .unwrap(); // d1 again
        drop(tx);
        handle.join().unwrap();

        let d1 = fs::read_to_string(dir.join("2026-11-16").join("a-metrics.jsonl")).unwrap();
        let d2 = fs::read_to_string(dir.join("2026-11-17").join("a-metrics.jsonl")).unwrap();
        let d1: Vec<serde_json::Value> = d1
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(d1.len(), 2);
        assert_eq!(d1[0]["ts_ms"], 1_794_821_040_000_i64);
        assert_eq!(d1[1]["ts_ms"], 1_794_821_050_000_i64);
        assert_eq!(d2.lines().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Retention policy: whole expired date dirs are removed; the boundary
    /// day (exactly `retention_days` old) is kept; non-date dirs, invalid
    /// dates and stray files are never touched.
    #[test]
    fn prune_removes_only_expired_date_dirs() {
        let dir = std::env::temp_dir().join(format!("ivitals-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // now = 2026-11-16T12:00:00Z; retention 30 -> cutoff 2026-10-17 (kept).
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for name in [
            "2020-01-01", // expired: removed
            "2026-10-16", // expired: removed
            "2026-10-17", // boundary: kept
            "2026-11-15", // recent: kept
            "2026-2-3",   // not strict zero-padded: kept
            "2026-02-30", // invalid calendar date: kept
            "keepme",     // not a date: kept
        ] {
            let d = dir.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("x.jsonl"), "line\n").unwrap();
        }
        fs::write(dir.join("stray.txt"), "not ours\n").unwrap();

        let removed = prune(&dir, 30, now).unwrap();
        assert_eq!(removed, 2);
        assert!(!dir.join("2020-01-01").exists());
        assert!(!dir.join("2026-10-16").exists());
        assert!(dir.join("2026-10-17").join("x.jsonl").exists());
        assert!(dir.join("2026-11-15").exists());
        assert!(dir.join("2026-2-3").exists());
        assert!(dir.join("2026-02-30").exists());
        assert!(dir.join("keepme").exists());
        assert!(dir.join("stray.txt").exists());

        // Missing data dir is not an error.
        assert_eq!(prune(&dir.join("nope"), 30, now).unwrap(), 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
