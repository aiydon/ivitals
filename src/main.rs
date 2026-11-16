//! ivitals binary entry point.
//!
//! Zero command-line arguments. The only input is `config.toml`, resolved
//! in the order `$IVITALS_CONFIG` -> `$PWD/config.toml` -> `<exe dir>/config.toml`.

use ivitals::collector::local::LocalCollector;
use ivitals::collector::Collector;
use ivitals::config::Config;

fn main() -> anyhow::Result<()> {
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

    // C4: one collection round per local server, logged as JSON so the
    // model/collector contract can be inspected without the storage layer.
    for server in &cfg.servers {
        if server.mode != ivitals::config::Mode::Local {
            continue;
        }
        let mut collector = LocalCollector::new(&server.alias, cfg.global.top_n_processes);
        if let Ok(sample) = collector.metrics() {
            log::info!(
                "sample: {}",
                serde_json::to_string(&sample).unwrap_or_default()
            );
        }
        if let Ok(snapshot) = collector.processes() {
            log::info!(
                "snapshot: top={} total={:?}",
                snapshot.processes.len(),
                snapshot.total_procs
            );
            for row in snapshot.processes.iter().take(5) {
                log::info!(
                    "  pid={} {} cpu={:.1}% mem={:.2}% state={:?} cum_ms={:?}",
                    row.pid,
                    row.name,
                    row.cpu_percent,
                    row.mem_percent,
                    row.state,
                    row.cum_cpu_ms
                );
            }
        }
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
