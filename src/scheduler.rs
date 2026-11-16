//! Tick loop: drives every configured collector in parallel via
//! `spawn_blocking` and feeds successful records into the storage channel.
//!
//! Guarantees (per the goal spec):
//! - one slow server never blocks the others (independent blocking tasks);
//! - a collection exceeding `interval / 2` triggers a warn, rate-limited to
//!   once per alias per 60 s;
//! - a failed collection (unreachable host, bad JSON, ...) triggers a warn,
//!   rate-limited the same way; the tick for that server is skipped —
//!   nothing is written, no pseudo-values;
//! - process snapshots run only on ticks aligned to
//!   `process_interval_secs` (alignment survives non-divisible intervals);
//! - missed ticks are skipped (`MissedTickBehavior::Skip`): after a long
//!   stall we resample rather than replay a backlog.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tokio::time::{interval, MissedTickBehavior};

use crate::collector::{CollectError, Collector};
use crate::config::Config;
use crate::model::Record;

/// One in-flight collection: the handle over the blocking task and the
/// collector it moved (re-inserted into the map on return).
type RoundTask =
    tokio::task::JoinHandle<(Result<Record, CollectError>, Duration, Box<dyn Collector>)>;

const WARN_WINDOW: Duration = Duration::from_secs(60);

/// Per-key warn throttling.
#[derive(Default)]
pub struct RateLimiter {
    last: HashMap<String, Instant>,
}

impl RateLimiter {
    /// Returns true (and arms the window) when a warn for `key` is due.
    pub fn should_warn(&mut self, key: &str, window: Duration) -> bool {
        let now = Instant::now();
        match self.last.get(key) {
            Some(t) if now.duration_since(*t) < window => false,
            _ => {
                self.last.insert(key.to_string(), now);
                true
            }
        }
    }
}

pub struct Scheduler {
    interval: Duration,
    process_interval: Duration,
    slow_threshold: Duration,
    /// Configured servers, in config order (stable iteration).
    servers: Vec<crate::config::Server>,
    /// One long-lived collector per alias; reused across ticks so sysinfo
    /// CPU deltas and the NVML handle stay warm. A round borrows each
    /// entry `&mut` for the duration of its blocking task.
    collectors: HashMap<String, Box<dyn Collector>>,
    /// Aliases whose collector task panicked: skipped (rate-limited error)
    /// for the rest of the run. A panic here means a collector bug, not an
    /// input condition; the daemon must not die with them.
    dead: Vec<String>,
    tx: mpsc::Sender<Record>,
    rate: RateLimiter,
    /// Elapsed and next-due process-snapshot times, in accumulated tick
    /// seconds, so 5s/60s alignment holds even when the interval does not
    /// divide the process interval.
    tick_seconds: u64,
    next_process_at: u64,
}

impl Scheduler {
    pub fn new(
        cfg: &Config,
        collectors: HashMap<String, Box<dyn Collector>>,
        tx: mpsc::Sender<Record>,
    ) -> Self {
        let interval = Duration::from_secs(cfg.global.interval_secs);
        let process_interval = Duration::from_secs(cfg.global.process_interval_secs);
        let servers = cfg.servers.clone();
        Self {
            interval,
            process_interval,
            slow_threshold: interval / 2,
            servers,
            collectors,
            dead: Vec::new(),
            tx,
            rate: RateLimiter::default(),
            tick_seconds: 0,
            next_process_at: 0,
        }
    }

    /// Runs until the task is aborted; then drop the storage sender so the
    /// writer thread flushes and exits.
    pub async fn run(mut self) {
        let mut tick = interval(self.interval);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            self.tick_seconds += self.interval.as_secs();
            let with_processes = self.tick_seconds >= self.next_process_at;
            if with_processes {
                self.next_process_at += self.process_interval.as_secs();
            }
            self.collect_round(false).await;
            if with_processes {
                self.collect_round(true).await;
            }
        }
    }

    /// One round over all live servers: `metrics` or `processes`.
    ///
    /// Each collector is `remove`d from the map and moved into its blocking
    /// task (a `&mut` cannot cross `spawn_blocking`), then re-inserted on
    /// return. The map entry is absent for the duration of the round, which
    /// is fine: each alias is driven by exactly one task.
    async fn collect_round(&mut self, processes: bool) {
        let label = if processes { "processes" } else { "metrics" };
        let pending: Vec<(&String, RoundTask)> = self
            .servers
            .iter()
            .filter(|s| !self.dead.contains(&s.alias))
            .filter_map(|s| {
                let mut col = self.collectors.remove(&s.alias)?;
                Some((
                    &s.alias,
                    tokio::task::spawn_blocking(move || {
                        let t0 = Instant::now();
                        let res = if processes {
                            col.processes().map(Record::Processes)
                        } else {
                            col.metrics()
                                .map(|sample| Record::Metrics(Box::new(sample)))
                        };
                        (res, t0.elapsed(), col)
                    }),
                ))
            })
            .collect();

        for (alias, handle) in pending {
            match handle.await {
                Ok((res, elapsed, col)) => {
                    self.collectors.insert(alias.clone(), col);
                    match res {
                        Ok(rec) => {
                            if elapsed > self.slow_threshold
                                && self.rate.should_warn(&format!("slow:{alias}"), WARN_WINDOW)
                            {
                                log::warn!(
                                    "slow tick for {alias} ({label}): {elapsed:?} > threshold {:?} (continuing)",
                                    self.slow_threshold
                                );
                            }
                            if let Err(e) = self.tx.send(rec) {
                                if self.rate.should_warn(&format!("sink:{alias}"), WARN_WINDOW) {
                                    log::error!("storage channel closed, record dropped: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            if self.rate.should_warn(&format!("err:{alias}"), WARN_WINDOW) {
                                log::warn!("collect {label} for {alias} failed: {e}");
                            }
                        }
                    }
                }
                Err(join_err) => {
                    self.dead.push(alias.clone());
                    log::error!(
                        "collector task for {alias} panicked: {join_err}; server disabled for this run"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rate limiter: first call fires, immediate repeat is suppressed,
    /// different keys are independent, and the window re-arms.
    #[tokio::test]
    async fn rate_limiter_windows() {
        let mut rl = RateLimiter::default();
        assert!(rl.should_warn("a", WARN_WINDOW));
        assert!(!rl.should_warn("a", WARN_WINDOW));
        assert!(rl.should_warn("b", WARN_WINDOW)); // different key
        assert!(!rl.should_warn("b", WARN_WINDOW));
        // Re-arm with a short window.
        std::thread::sleep(Duration::from_millis(10));
        assert!(rl.should_warn("a", Duration::from_millis(5)));
    }
}
