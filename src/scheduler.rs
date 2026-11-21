//! Per-server tick loops: each configured host runs in its own task with
//! its own interval, so a slow SSH host can no longer delay collection
//! from the others (the previous implementation awaited every server's
//! blocking task in one round).
//!
//! Guarantees:
//! - one collector task per alias, driven by an independent interval;
//! - missed ticks are skipped (`MissedTickBehavior::Skip`), never replayed;
//! - collection errors are rate-limited per alias + kind, and the tick is
//!   skipped (no pseudo-values);
//! - a panicking collector disables only that alias, with an error log;
//! - process snapshots follow `process_interval_secs` alignment;
//! - the storage sender is cloned per task; when `run` is aborted the
//!   JoinSet drops and aborts all tasks, closing every sender.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use tokio::time::{interval, MissedTickBehavior};

use crate::collector::Collector;
use crate::config::Config;
use crate::model::Record;

const WARN_WINDOW: Duration = Duration::from_secs(60);
const HOST_REFRESH_SECS: u64 = 86_400;

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
    tick_period: Duration,
    process_interval: Duration,
    slow_threshold: Duration,
    servers: Vec<crate::config::Server>,
    collectors: HashMap<String, Box<dyn Collector>>,
    tx: mpsc::Sender<Record>,
}

impl Scheduler {
    pub fn new(
        cfg: &Config,
        collectors: HashMap<String, Box<dyn Collector>>,
        tx: mpsc::Sender<Record>,
    ) -> Self {
        let tick_period = Duration::from_secs(cfg.global.interval_secs);
        Self {
            tick_period,
            process_interval: Duration::from_secs(cfg.global.process_interval_secs),
            slow_threshold: tick_period / 2,
            servers: cfg.servers.clone(),
            collectors,
            tx,
        }
    }

    /// Spawns one independent loop per server and joins them. Aborting the
    /// supervisor aborts the whole JoinSet, which drops every sender clone.
    pub async fn run(mut self) {
        let mut tasks = JoinSet::new();
        for server in std::mem::take(&mut self.servers) {
            let Some(collector) = self.collectors.remove(&server.alias) else {
                continue;
            };
            let tx = self.tx.clone();
            let tick_period = self.tick_period;
            let process_interval = self.process_interval;
            let slow_threshold = self.slow_threshold;
            let alias = server.alias.clone();
            tasks.spawn(async move {
                run_server(
                    alias,
                    collector,
                    tx,
                    tick_period,
                    process_interval,
                    slow_threshold,
                )
                .await;
            });
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::error!("collector task ended abnormally: {error}");
            }
        }
    }
}

async fn run_server(
    alias: String,
    mut collector: Box<dyn Collector>,
    tx: mpsc::Sender<Record>,
    tick_period: Duration,
    process_interval: Duration,
    slow_threshold: Duration,
) {
    let mut rate = RateLimiter::default();
    let mut tick = interval(tick_period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut tick_seconds = 0u64;
    let mut next_process_at = 0u64;
    let mut next_host_at = 0u64;

    loop {
        tick.tick().await;
        tick_seconds += tick_period.as_secs();

        if tick_seconds >= next_host_at {
            next_host_at += HOST_REFRESH_SECS;
            let (host, returned) = match tokio::task::spawn_blocking(move || {
                let host = collector.host_info();
                (host, collector)
            })
            .await
            {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("host probe for {alias} panicked: {error}; server disabled");
                    return;
                }
            };
            collector = returned;
            if let Some(host) = host {
                if tx.send(Record::HostInfo(host)).is_err() {
                    log::error!("storage channel closed; stopping collector for {alias}");
                    return;
                }
            }
        }

        let with_processes = tick_seconds >= next_process_at;
        if with_processes {
            next_process_at += process_interval.as_secs();
        }

        let t0 = Instant::now();
        let (result, returned) = match tokio::task::spawn_blocking(move || {
            let result = collector.metrics();
            (result, collector)
        })
        .await
        {
            Ok(pair) => pair,
            Err(error) => {
                log::error!("collector for {alias} panicked: {error}; server disabled");
                return;
            }
        };
        collector = returned;
        let elapsed = t0.elapsed();
        match result {
            Ok(sample) => {
                if elapsed > slow_threshold
                    && rate.should_warn(&format!("slow:metrics:{alias}"), WARN_WINDOW)
                {
                    log::warn!("slow metrics tick for {alias}: {elapsed:?} > {slow_threshold:?}");
                }
                if let Some(ms) = sample.extras.extras_ms {
                    if ms > 20 && rate.should_warn(&format!("extras_slow:{alias}"), WARN_WINDOW) {
                        log::warn!("extras collection for {alias} took {ms} ms (>20 ms budget)");
                    }
                }
                if sample.extras.extras_errors.unwrap_or(0) > 0
                    && rate.should_warn(&format!("extras_err:{alias}"), WARN_WINDOW)
                {
                    log::warn!(
                        "extras parse errors for {alias}: {} family/families failed this tick",
                        sample.extras.extras_errors.unwrap_or(0)
                    );
                }
                if tx.send(Record::Metrics(Box::new(sample))).is_err() {
                    if rate.should_warn(&format!("sink:{alias}"), WARN_WINDOW) {
                        log::error!("storage channel closed; stopping collector for {alias}");
                    }
                    return;
                }
            }
            Err(error) => {
                if rate.should_warn(&format!("err:metrics:{alias}"), WARN_WINDOW) {
                    log::warn!("collect metrics for {alias} failed: {error}");
                }
            }
        }

        if !with_processes {
            continue;
        }
        let t0 = Instant::now();
        let (result, returned) = match tokio::task::spawn_blocking(move || {
            let result = collector.processes();
            (result, collector)
        })
        .await
        {
            Ok(pair) => pair,
            Err(error) => {
                log::error!("collector for {alias} panicked: {error}; server disabled");
                return;
            }
        };
        collector = returned;
        let elapsed = t0.elapsed();
        match result {
            Ok(snapshot) => {
                if elapsed > slow_threshold
                    && rate.should_warn(&format!("slow:processes:{alias}"), WARN_WINDOW)
                {
                    log::warn!("slow process tick for {alias}: {elapsed:?} > {slow_threshold:?}");
                }
                if tx.send(Record::Processes(snapshot)).is_err() {
                    if rate.should_warn(&format!("sink:{alias}"), WARN_WINDOW) {
                        log::error!("storage channel closed; stopping collector for {alias}");
                    }
                    return;
                }
            }
            Err(error) => {
                if rate.should_warn(&format!("err:processes:{alias}"), WARN_WINDOW) {
                    log::warn!("collect processes for {alias} failed: {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rate_limiter_windows() {
        let mut rl = RateLimiter::default();
        assert!(rl.should_warn("a", WARN_WINDOW));
        assert!(!rl.should_warn("a", WARN_WINDOW));
        assert!(rl.should_warn("b", WARN_WINDOW));
        assert!(!rl.should_warn("b", WARN_WINDOW));
        std::thread::sleep(Duration::from_millis(10));
        assert!(rl.should_warn("a", Duration::from_millis(5)));
    }
}
