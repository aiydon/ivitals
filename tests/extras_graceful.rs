//! Graceful degradation of optional metrics (goal §5, §12.10):
//!
//! When an extras source is unavailable (missing /proc file, no DCGM, no
//! slurmctld, empty remote sections), the corresponding field must be
//! **absent** from the serialized line — never `null`, never an
//! `"unavailable"` pseudo-value, and never a cause of panic or warn spam.
//!
//! The remote path is used for the deterministic tests because it depends
//! only on the supplied text; one smoke test drives the local /proc path
//! with range assertions that hold on any Linux host.

use std::collections::{BTreeMap, HashMap};

use ivitals::collector::extras::{disk_io, net, ExtrasCollector, Flags};
use ivitals::model::{CpuInfo, Extras, GpuStatus, MemoryInfo, MetricSample};
use ivitals::SCHEMA_VERSION;

const ALL_FLAGS: Flags = Flags {
    cpu_detail: true,
    memory_detail: true,
    disk_io: true,
    net: true,
    gpu_detail: true,
    slurm: true,
    psi: true,
};

fn sample_with(extras: Extras) -> MetricSample {
    MetricSample {
        schema_version: SCHEMA_VERSION,
        ts_ms: 1_700_000_000_000,
        server: "test".into(),
        mode: "local",
        collect_ms: 1,
        cpu: CpuInfo {
            global_percent: 0.0,
            core_count: 1,
            load_avg_1m: 0.0,
        },
        memory: MemoryInfo {
            total_bytes: 1 << 30,
            used_bytes: 1 << 29,
            available_bytes: 1 << 29,
        },
        gpu: GpuStatus::Absent,
        extras,
    }
}

fn assert_no_pseudo_values(v: &serde_json::Value) {
    match v {
        serde_json::Value::Null => panic!("serialized line contains a null"),
        serde_json::Value::String(s) => {
            assert_ne!(
                s, "unavailable",
                "serialized line contains the pseudo-value \"unavailable\""
            );
        }
        serde_json::Value::Object(m) => {
            for value in m.values() {
                assert_no_pseudo_values(value);
            }
        }
        serde_json::Value::Array(a) => {
            for value in a {
                assert_no_pseudo_values(value);
            }
        }
        _ => {}
    }
}

/// The core acceptance test: every extras family unavailable at once.
/// A second tick ensures the delta path actually runs (a first tick is
/// trivially empty — it has no baseline), and the full sample serializes
/// to exactly the core key set plus the real `extras_ms` measurement.
#[test]
fn unavailable_extras_are_absent_not_pseudo_values() {
    let mut ec = ExtrasCollector::new(ALL_FLAGS);
    let empty = HashMap::new();

    let first = ec.collect_remote(&empty, None, None);
    for (name, present) in [
        ("cpu_detail", first.cpu_detail.is_some()),
        ("memory_detail", first.memory_detail.is_some()),
        ("disk_io", first.disk_io.is_some()),
        ("net", first.net.is_some()),
        ("gpu_detail", first.gpu_detail.is_some()),
        ("slurm", first.slurm.is_some()),
    ] {
        assert!(!present, "first tick must not fabricate {name}");
    }
    assert!(first.psi.is_empty());

    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = ec.collect_remote(&empty, None, None);
    assert!(second.cpu_detail.is_none());
    assert!(second.memory_detail.is_none());
    assert!(second.disk_io.is_none());
    assert!(second.net.is_none());
    assert!(second.gpu_detail.is_none());
    assert!(second.slurm.is_none());
    assert!(second.psi.is_empty());
    // 0 ms is a real measurement of the (empty) work, not a pseudo-value.
    assert_eq!(second.extras_ms, 0);

    let mut extras = Extras::default();
    second.apply(&mut extras);

    let v = serde_json::to_value(sample_with(extras)).unwrap();
    assert_no_pseudo_values(&v);

    let want: std::collections::BTreeSet<&str> = [
        "schema_version",
        "ts_ms",
        "server",
        "mode",
        "collect_ms",
        "cpu",
        "memory",
        "gpu",
        "extras_ms",
    ]
    .into_iter()
    .collect();
    let keys: std::collections::BTreeSet<&str> =
        v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
    assert_eq!(keys, want);
    assert_eq!(v["gpu"]["state"], "absent");
}

/// PSI sections: valid text yields the flat key, garbage yields no key at
/// all (not 0.0 — a fake reading would be a pseudo-value).
#[test]
fn psi_section_parses_and_garbage_yields_no_key() {
    let mut ec = ExtrasCollector::new(ALL_FLAGS);
    let baseline = HashMap::new();
    let _ = ec.collect_remote(&baseline, None, None);
    std::thread::sleep(std::time::Duration::from_millis(2));

    let mut sections: HashMap<String, String> = HashMap::new();
    sections.insert(
        "psi_cpu".into(),
        "some avg10=1.25 avg60=0.10 avg300=0.00 avg600=0.00\nfull avg10=0.00 avg60=0.00 avg300=0.00 avg600=0.00"
            .to_string(),
    );
    sections.insert("psi_mem".into(), "garbage".into());

    let data = ec.collect_remote(&sections, None, None);
    let mut want: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    want.insert("psi_cpu_some".into(), serde_json::Value::from(1.25));
    assert_eq!(data.psi, want);
    assert!(!data.psi.contains_key("psi_mem_some"));
}

/// Counter regression (device reset / reboot mid-run) clamps deltas to 0;
/// an idle device is then not emitted at all. A healthy device in the same
/// snapshot must still come out with exact values.
#[test]
fn diskstats_counter_regression_is_clamped() {
    fn raw(rows: &[(&str, u64, u64, u64, u64)]) -> disk_io::DiskRaw {
        disk_io::DiskRaw {
            rows: rows
                .iter()
                .map(|(name, r, w, ticks, wait)| {
                    let mut vals = vec![0u64; 20];
                    vals[2] = *r; // sectors read
                    vals[6] = *w; // sectors written
                    vals[9] = *ticks; // io_ticks
                    vals[10] = *wait; // wait ticks
                    (name.to_string(), vals)
                })
                .collect(),
        }
    }

    let prev = raw(&[("sda", 1000, 500, 100, 50), ("sdb", 100, 100, 10, 10)]);
    // sda's counters went backwards; sdb did 1000 read + 500 write sectors.
    let cur = raw(&[("sda", 10, 10, 10, 10), ("sdb", 1100, 600, 200, 100)]);

    let out = disk_io::compute(&prev, &cur, 1.0).unwrap();
    assert!(
        out.iter().all(|d| d.device != "sda"),
        "regressed device must not be emitted: {out:?}"
    );
    let sdb = out.iter().find(|d| d.device == "sdb").unwrap();
    assert_eq!(sdb.read_bytes_per_s, 1000 * 512);
    assert_eq!(sdb.write_bytes_per_s, 500 * 512);
    assert_eq!(sdb.iops, 1500);
    assert!((sdb.util_percent as f64 - 19.0).abs() < 1e-3);
    assert!((sdb.await_ms as f64 - 0.06).abs() < 1e-3);

    // Structural filter: partitions, loop and dm devices are dropped.
    let names: Vec<String> = ["nvme0n1", "nvme0n1p1", "loop0", "dm-2"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(disk_io::filter_devices(&names), vec!["nvme0n1"]);
}

/// Net: loopback excluded from the aggregate, retransmission percent is a
/// delta ratio, a missing TCP counter section removes the whole struct
/// (no partial pseudo-values), and counter regressions clamp to 0.
#[test]
fn net_aggregate_excludes_loop_and_clamps() {
    fn raw(dev: &[(&str, u64, u64)], tcp: Option<(u64, u64)>) -> net::NetRaw {
        net::NetRaw {
            dev: dev
                .iter()
                .map(|(n, r, t)| (n.to_string(), *r, *t))
                .collect(),
            tcp,
        }
    }

    let prev = raw(
        &[("eth0", 1_000, 500), ("lo", 9_999, 9_999)],
        Some((10_000, 100)),
    );
    let cur = raw(
        &[("eth0", 21_000, 10_500), ("lo", 9_999, 9_999)],
        Some((20_000, 600)),
    );

    let out = net::compute(&prev, &cur, 1.0).unwrap();
    assert_eq!(out.rx_bytes_per_s, 20_000);
    assert_eq!(out.tx_bytes_per_s, 10_000);
    assert!((out.retrans_percent as f64 - 5.0).abs() < 1e-3);

    // No TCP counters -> the whole family is absent.
    let no_tcp = raw(&[("eth0", 21_000, 10_500)], None);
    assert!(net::compute(&prev, &no_tcp, 1.0).is_none());

    // Regression on eth0 counters clamps the rates to 0.
    let regressed = raw(&[("eth0", 0, 0), ("lo", 9_999, 9_999)], Some((20_000, 600)));
    let out = net::compute(&prev, &regressed, 1.0).unwrap();
    assert_eq!(out.rx_bytes_per_s, 0);
    assert_eq!(out.tx_bytes_per_s, 0);
}

/// Local /proc path smoke: every family enabled, sources present or not,
/// the pipeline must not panic and must stay inside the 20 ms budget.
/// Value ranges hold on any Linux host; which families are present is
/// host-dependent and is asserted only for validity.
#[test]
fn collect_local_is_graceful_and_within_budget() {
    let mut ec = ExtrasCollector::new(ALL_FLAGS);
    let _first = ec.collect_local(None);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let data = ec.collect_local(None);

    assert!(data.extras_ms < 20, "extras took {} ms", data.extras_ms);

    if let Some(cd) = &data.cpu_detail {
        for v in [cd.iowait_percent, cd.steal_percent, cd.softirq_percent] {
            assert!(
                (0.0..=100.0).contains(&(v as f64)),
                "cpu state % out of range: {v}"
            );
        }
        if let Some(per_core) = &cd.per_core_percent {
            for v in per_core {
                assert!((0.0..=100.0).contains(&(*v as f64)));
            }
        }
    }
    if let Some(ndevice) = &data.disk_io {
        for d in ndevice {
            assert!((0.0..=100.0).contains(&(d.util_percent as f64)));
        }
    }
    if let Some(n) = &data.net {
        assert!((0.0..=100.0).contains(&(n.retrans_percent as f64)));
    }
    for v in data.psi.values() {
        let f = v.as_f64().unwrap();
        assert!((0.0..=100.0).contains(&f), "PSI value out of range: {f}");
    }
}
