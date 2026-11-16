//! ivitals binary entry point.
//!
//! Zero command-line arguments. The only input is `config.toml`, resolved
//! in the order `$IVITALS_CONFIG` -> `$PWD/config.toml` -> `<exe dir>/config.toml`.

use std::collections::HashMap;
use std::sync::mpsc;

use ivitals::collector::local::LocalCollector;
use ivitals::collector::remote::RemoteCollector;
use ivitals::collector::Collector;
use ivitals::config::{Config, Mode};
use ivitals::model::Record;
use ivitals::scheduler::Scheduler;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    init_logger(&cfg.global.log_level);
    log::info!(
        "ivitals starting: {} servers ({:?}), interval={}s process_interval={}s data_dir={}",
        cfg.servers.len(),
        cfg.servers
            .iter()
            .map(|s| s.alias.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        cfg.global.interval_secs,
        cfg.global.process_interval_secs,
        cfg.global.data_dir,
    );

    // Storage channel + single writer thread: appends JSONL under
    // data/{YYYY-MM-DD}/{alias}-{kind}.jsonl, flushes on 30s/200-record
    // cadence and on channel close (Ctrl-C).
    let (tx, rx) = mpsc::channel::<Record>();
    let writer = ivitals::storage::spawn_writer(
        rx,
        cfg.global.data_dir.clone().into(),
        // day count; u64 -> u32 is a no-op for any sane config
        cfg.global.retention_days as u32,
    );

    let mut collectors: HashMap<String, Box<dyn Collector>> = HashMap::new();
    for server in &cfg.servers {
        match server.mode {
            Mode::Local => {
                collectors.insert(
                    server.alias.clone(),
                    Box::new(LocalCollector::new(
                        &server.alias,
                        cfg.global.top_n_processes,
                    )),
                );
            }
            Mode::Remote => {
                let host = server.ssh_host.clone().unwrap_or_default();
                let user = server.ssh_user.clone().unwrap_or_default();
                collectors.insert(
                    server.alias.clone(),
                    Box::new(RemoteCollector::new(
                        &server.alias,
                        &host,
                        &user,
                        server.ssh_key.clone(),
                        server.connect_timeout_secs,
                        server.exec_timeout_secs,
                        cfg.global.top_n_processes,
                    )),
                );
            }
        }
    }

    let sched = Scheduler::new(&cfg, collectors, tx);
    let sched_task = tokio::spawn(async move { sched.run().await });

    tokio::signal::ctrl_c().await?;
    log::info!("ctrl-c: shutting down");
    sched_task.abort(); // drops the channel sender; writer drains and flushes
    if let Err(e) = writer.join() {
        log::error!("writer thread ended abnormally: {e:?}");
    }
    Ok(())
}

fn init_logger(level: &str) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    // RUST_LOG (if set) wins over the configured level.
    if std::env::var("RUST_LOG").is_err() {
        builder.filter_level(parse_level(level));
    }
    builder.init();
}

fn parse_level(level: &str) -> log::LevelFilter {
    match level.to_ascii_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}
