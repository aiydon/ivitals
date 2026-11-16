//! `slurm` extra: cluster allocation state from `sinfo`/`squeue`.
//!
//! Only active when a `slurmctld` endpoint was detected (see
//! docs/metrics-research.md §6: absent on all four v1 hosts, so this
//! parser has no live data source yet — it is pinned against the
//! documented `sinfo -N -h -o '%T %c %C %G'` / `squeue -h -o '%t'` output
//! so the v2 dcgmi/sinfo probe can reuse it without changes).
//!
//! Nodes section: one line per node `state cpus_alloc cpus_total gres`.
//! `gres` of `*` means the GRES plugin is unconfigured -> `gpus_*` are
//! `None` (absence, never a pseudo-0).

use crate::model::SlurmStat;

/// Parses the combined section text: node lines first, then a
/// `#jobs` marker line, then one job state per line.
pub fn parse(text: &str) -> Option<SlurmStat> {
    let (nodes_text, jobs_text) = text.split_once("#jobs")?;
    let mut nodes_alloc = 0u32;
    let mut nodes_idle = 0u32;
    let mut cpus_alloc = 0u32;
    let mut cpus_total = 0u32;
    let mut gpus_alloc = 0u32;
    let mut gpus_total = 0u32;
    let mut gres_known = false;
    for line in nodes_text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 4 {
            continue;
        }
        let (state, alloc, total, gres) = (t[0], t[1], t[2], t[3]);
        if state.starts_with("alloc") || state.starts_with("drain") {
            nodes_alloc += 1;
        } else if state.starts_with("idle") {
            nodes_idle += 1;
        }
        cpus_alloc += alloc.parse().unwrap_or(0);
        cpus_total += total.parse().unwrap_or(0);
        let g: Option<u32> = match gres {
            "*" | "N/A" => None,
            s => s.strip_prefix("ngpu:").and_then(|n| n.parse().ok()),
        };
        match g {
            Some(n) => {
                gres_known = true;
                gpus_total += n;
                if state.starts_with("alloc") || state.starts_with("drain") {
                    gpus_alloc += n;
                }
            }
            None => gres_known = false,
        }
    }
    let mut jobs_running = 0u32;
    let mut jobs_pending = 0u32;
    for line in jobs_text.lines() {
        let state = line.trim();
        if state.starts_with("PD") {
            jobs_pending += 1;
        } else if state.starts_with("RUN")
            || state.starts_with("COMPLET")
            || state.starts_with("CANCELL")
        {
            jobs_running += 1;
        }
    }
    Some(SlurmStat {
        nodes_alloc,
        nodes_idle,
        cpus_alloc,
        cpus_total,
        gpus_alloc: if gres_known { Some(gpus_alloc) } else { None },
        gpus_total: if gres_known { Some(gpus_total) } else { None },
        jobs_running,
        jobs_pending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nodes_and_jobs() {
        let text = concat!(
            "alloc 32 64 ngpu:4\n",
            "alloc 64 64 ngpu:8\n",
            "idle 0 64 ngpu:8\n",
            "drain 16 64 ngpu:4\n",
            "#jobs\n",
            "RUN\n",
            "RUN\n",
            "PD\n",
        );
        let s = parse(text).unwrap();
        assert_eq!(s.nodes_alloc, 3); // alloc + alloc + drain
        assert_eq!(s.nodes_idle, 1);
        assert_eq!(s.cpus_alloc, 112);
        assert_eq!(s.cpus_total, 256);
        assert_eq!(s.gpus_alloc, Some(16)); // 4 + 8 + 4 (drain = reserved)
        assert_eq!(s.gpus_total, Some(24));
        assert_eq!(s.jobs_running, 2);
        assert_eq!(s.jobs_pending, 1);
    }

    #[test]
    fn unknown_gres_is_none() {
        let text = "alloc 32 64 *\nidle 0 64 *\n#jobs\nRUN\n";
        let s = parse(text).unwrap();
        assert_eq!(s.gpus_alloc, None);
        assert_eq!(s.gpus_total, None);
    }

    #[test]
    fn missing_jobs_section_is_none() {
        assert!(parse("alloc 32 64 ngpu:4\n").is_none());
    }
}
