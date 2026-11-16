//! `net` extra: aggregate NIC throughput + TCP retransmission rate.
//!
//! Sources: /proc/net/dev (per-interface rx/tx bytes, `lo` excluded —
//! remote loopback would dominate any aggregate) and the `Tcp:` section of
//! /proc/net/snmp (`OutSegs`, `RetransSegs` counters; `TcpExt` is absent
//! on some kernels, so the coarser `Tcp:` counters are the portable
//! source).

use crate::model::NetStat;

/// One snapshot: (interface, rx_bytes, tx_bytes) per interface plus the
/// TCP counters (out_segs, retrans_segs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetRaw {
    pub dev: Vec<(String, u64, u64)>,
    pub tcp: Option<(u64, u64)>,
}

impl NetRaw {
    /// Parses the combined section text: /proc/net/dev first, then a
    /// `#snmp_tcp` marker line, then the two `Tcp:` snmp lines.
    pub fn parse(text: &str) -> Option<Self> {
        let (dev_text, snmp_text) = text.split_once("#snmp_tcp")?;
        let mut dev = Vec::new();
        for line in dev_text.lines().skip(2) {
            // Two header lines, then " iface: rx_bytes rx_pkts ... tx_bytes..."
            let (ifname, rest) = match line.split_once(':') {
                Some(p) => p,
                None => continue,
            };
            let name = ifname.trim();
            if name.is_empty() || name == "lo" {
                continue;
            }
            let t: Vec<&str> = rest.split_whitespace().collect();
            // 16 values: rx_bytes=t[0], tx_bytes=t[8]; malformed rows
            // are skipped, one bad interface must not kill the family.
            let Some(rx) = t.first().and_then(|s| s.parse().ok()) else {
                continue;
            };
            let Some(tx) = t.get(8).and_then(|s| s.parse().ok()) else {
                continue;
            };
            dev.push((name.to_string(), rx, tx));
        }
        let tcp = parse_snmp_tcp(snmp_text);
        Some(Self { dev, tcp })
    }

    /// Local read.
    pub fn read_local() -> Option<Self> {
        let dev = std::fs::read_to_string("/proc/net/dev").ok()?;
        let snmp = std::fs::read_to_string("/proc/net/snmp").ok()?;
        let snmp: Vec<String> = snmp
            .lines()
            .take_while(|l| !l.starts_with("Udp:"))
            .filter(|l| l.starts_with("Tcp:"))
            .map(|l| l.to_string())
            .collect();
        Self::parse(&format!("{dev}\n#snmp_tcp\n{}", snmp.join("\n")))
    }
}

/// Extracts (OutSegs, RetransSegs) from the two-line `Tcp:` snmp block:
/// line 1 the field names, line 2 the values, column-aligned.
fn parse_snmp_tcp(text: &str) -> Option<(u64, u64)> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return None;
    }
    let names: Vec<&str> = lines[0].split_whitespace().collect();
    let vals: Vec<&str> = lines[1].split_whitespace().collect();
    let mut out_segs = None;
    let mut retrans_segs = None;
    for (i, n) in names.iter().enumerate() {
        // The leading "Tcp:" column is not numeric; skip non-values.
        let Some(v) = vals.get(i).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        match *n {
            "OutSegs" => out_segs = Some(v),
            "RetransSegs" => retrans_segs = Some(v),
            _ => {}
        }
    }
    Some((out_segs?, retrans_segs?))
}

fn delta(cur: u64, prev: u64) -> f64 {
    (cur as i128 - prev as i128).max(0) as f64
}

/// Computes NetStat from two snapshots `window` seconds apart. All three
/// fields are delta-based, so `None` when either snapshot lacks the TCP
/// counters (whole struct absent, no partial pseudo-values).
pub fn compute(prev: &NetRaw, cur: &NetRaw, window: f64) -> Option<NetStat> {
    if window <= 0.0 {
        return None;
    }
    let mut rx = 0.0f64;
    let mut tx = 0.0f64;
    for (name, c_rx, c_tx) in &cur.dev {
        if let Some((_, p_rx, p_tx)) = prev.dev.iter().find(|(n, _, _)| n == name) {
            rx += delta(*c_rx, *p_rx);
            tx += delta(*c_tx, *p_tx);
        }
        // Interfaces that appeared mid-run have no baseline; skip them.
    }
    let (c_out, c_re) = cur.tcp?;
    let (p_out, p_re) = prev.tcp?;
    let d_out = delta(c_out, p_out);
    let d_re = delta(c_re, p_re);
    let retrans = if d_out > 0.0 {
        (d_re / d_out * 100.0).min(100.0)
    } else {
        0.0
    };
    Some(NetStat {
        rx_bytes_per_s: (rx / window) as u64,
        tx_bytes_per_s: (tx / window) as u64,
        retrans_percent: retrans as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(rx: u64, tx: u64, out: u64, re: u64) -> NetRaw {
        NetRaw {
            dev: vec![("eth0".into(), rx, tx)],
            tcp: Some((out, re)),
        }
    }

    #[test]
    fn bandwidth_and_retrans() {
        let prev = raw(0, 0, 1000, 0);
        let cur = raw(25_000, 50_000, 2000, 100);
        let d = compute(&prev, &cur, 5.0).unwrap();
        assert_eq!(d.rx_bytes_per_s, 5000);
        assert_eq!(d.tx_bytes_per_s, 10000);
        // 100/1000 = 10%
        assert!((d.retrans_percent - 10.0).abs() < 0.001);
    }

    #[test]
    fn no_traffic_zero_retrans() {
        let prev = raw(100, 100, 500, 10);
        let cur = raw(100, 100, 500, 10);
        let d = compute(&prev, &cur, 5.0).unwrap();
        assert_eq!(d.retrans_percent, 0.0);
        assert_eq!(d.rx_bytes_per_s, 0);
    }

    #[test]
    fn missing_tcp_counters_is_none() {
        let prev = raw(0, 0, 100, 0);
        let mut cur = raw(100, 100, 200, 5);
        cur.tcp = None;
        assert!(compute(&prev, &cur, 5.0).is_none());
    }

    #[test]
    fn parses_section_text() {
        let text = concat!(
            "Inter-|   Receive                                                |  Transmit\n",
            " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
            "    lo: 1234567     100    0    0    0     0          0         0  1234567     100    0    0    0     0       0          0\n",
            "  eth0: 999999     500    0    0    0     0          0         0  888888     400    0    0    0     0       0          0\n",
            "#snmp_tcp\n",
            "Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors\n",
            "Tcp: 1 200 120000 -1 10 20 3 4 5 10000 20000 100 0 6 0\n",
        );
        let r = NetRaw::parse(text).unwrap();
        assert_eq!(r.dev.len(), 1);
        assert_eq!(r.dev[0].0, "eth0");
        assert_eq!(r.dev[0].1, 999999);
        assert_eq!(r.dev[0].2, 888888);
        assert_eq!(r.tcp, Some((20000, 100)));
    }
}
