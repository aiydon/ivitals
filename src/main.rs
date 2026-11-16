//! ivitals binary entry point.
//!
//! Zero command-line arguments: the only input is `config.toml`, resolved in
//! the order `$IVITALS_CONFIG`, `$PWD/config.toml`, `<exe dir>/config.toml`.

fn main() -> anyhow::Result<()> {
    let cfg = ivitals::config::Config::load()?;
    init_logger(&cfg.global.log_level);
    log::info!(
        "ivitals starting: servers=[{}] interval={}s process_interval={}s top_n={} retention={}d data_dir={}",
        cfg.servers
            .iter()
            .map(|s| s.alias.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        cfg.global.interval_secs,
        cfg.global.process_interval_secs,
        cfg.global.top_n_processes,
        cfg.global.retention_days,
        cfg.global.data_dir,
    );
    Ok(())
}

/// Initialize the logger. An explicit `RUST_LOG` always wins; otherwise the
/// `log_level` from config applies.
fn init_logger(level: &str) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if std::env::var("RUST_LOG").is_err() {
        builder.filter_level(parse_level(level));
    }
    builder.init();
}

fn parse_level(s: &str) -> log::LevelFilter {
    match s.to_ascii_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}
