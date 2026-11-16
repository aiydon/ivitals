//! ivitals binary entry point.
//!
//! Zero command-line arguments: the only input is `config.toml`, resolved in
//! the order `$IVITALS_CONFIG`, `$PWD/config.toml`, `<exe dir>/config.toml`.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let path = resolve_config_path();
    if !path.is_file() {
        anyhow::bail!("ivitals: config not found: {}", path.display());
    }
    eprintln!("ivitals: config at {}", path.display());
    Ok(())
}

/// Resolve the config path without reading the file.
fn resolve_config_path() -> PathBuf {
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
    // Fallback: report the expected $PWD location so errors are actionable.
    PathBuf::from("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_pwd_config_when_present() {
        // Running from the crate root in CI/dev: config.toml is there.
        if PathBuf::from("config.toml").is_file() {
            assert_eq!(resolve_config_path(), PathBuf::from("config.toml"));
        }
    }
}
