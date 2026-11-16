# ivitals

Config-driven resident resource monitor. Collects CPU / memory / GPU
metrics plus extended ("extras") metrics and periodic process snapshots
from a set of servers, and appends them as JSONL under UTC date
directories. Collect and record only — no web UI, no alerting, no hot
reload, no aggregation or judgement on the collector side.

No command-line arguments: `config.toml` is the only input. Machines
without NVIDIA GPUs are first-class citizens (`gpu.state = "absent"`,
no panic, no log spam).

## Running

```bash
cargo build --release
./target/release/ivitals
```

The config file is located in this order:

1. `$IVITALS_CONFIG` (if set),
2. `config.toml` in the working directory,
3. `config.toml` next to the executable.

Stop with Ctrl-C: the writer thread flushes all open files and the
process exits within ~1 s.

## Configuration

```toml
schema_version = 1

[global]
interval_secs = 5          # metrics tick period (>= 2)
process_interval_secs = 60 # process snapshot period (>= interval_secs)
top_n_processes = 20       # per snapshot, by CPU then memory
retention_days = 30        # whole UTC date dirs older than this are removed
data_dir = "data"          # relative to the working directory, or absolute
log_level = "info"

[extras]                   # every key optional; defaults shown
cpu_detail = true          # iowait/steal/softirq, load 5m/15m, per-core
memory_detail = true       # cache, swap used + in/out rates, oom_kill count
disk_io = true             # per-whole-disk util/await/iops/throughput
net = true                 # rx/tx bytes + TCP retransmission %
gpu_detail = "auto"        # auto | dcgm | off
slurm = "auto"             # auto | off (only collected when sinfo exists)
psi = true                 # flat psi_cpu_some / psi_mem_some / psi_io_some

[[servers]]
alias = "math2"            # unique, used in file names
mode = "remote"            # remote | local
ssh_host = "math2"         # required for remote (ssh config alias ok)
ssh_user = "monitor"       # required for remote
connect_timeout_secs = 5
exec_timeout_secs = 15
```

`ssh_host` may be an alias from `~/.ssh/config`: ivitals resolves
`HostName`, `Port`, and `IdentityFile` from the first matching `Host`
block (the config file is read once at startup). `ssh_user` always wins
over a `User` line in the ssh config. Without a matching block the host
is used literally on port 22.

An extras family set to `false`/`"off"` is not collected at all: no
fields, no cost. Availability of the optional sources (NVIDIA driver,
DCGM, slurm, `/proc/pressure`) is probed once and cached; a missing
source simply omits the corresponding fields.

## Data layout

```
data/
├── 2026-11-15/
│   ├── math2-metrics.jsonl
│   ├── math2-processes.jsonl
│   └── local-metrics.jsonl
└── 2026-11-16/
    ├── math2-metrics.jsonl
    └── bot-processes.jsonl
```

- One directory per UTC day (`YYYY-MM-DD`); files are
  `{alias}-{metrics|processes}.jsonl`. Nothing is ever written at the
  root of `data/`.
- The writer is a single thread; it rolls to a new directory at the UTC
  midnight crossing (derived from each record's `ts_ms`) and flushes
  every 30 s or 200 records.
- Retention: at startup and every 24 h, date directories strictly older
  than `retention_days` are removed wholesale. Non-date entries are
  never touched.

## Schema

Every metrics line:

```json
{"schema_version":1,"ts_ms":1763300000000,"server":"math2","mode":"remote",
 "collect_ms":4,"cpu":{"global_percent":12.5,"core_count":96,"load_avg_1m":3.2},
 "memory":{"total_bytes":766520000000,"used_bytes":300000000000,"available_bytes":466000000000},
 "gpu":{"state":"ok","gpus":[{"index":0,"name":"NVIDIA A800 80GB","util_percent":98,
 "mem_used_mb":70000,"mem_total_mb":81920,"temp_c":62}]},
 "cpu_detail":{"iowait_percent":0.4,"steal_percent":0.0,"softirq_percent":0.9,
 "load_avg_5m":3.0,"load_avg_15m":2.8,"per_core_max_percent":71.2,
 "per_core_p95_percent":35.0,"cores_over_50":3},
 "memory_detail":{"cache_bytes":88000000000,"swap_used_bytes":0,
 "swap_in_bytes_per_s":0,"swap_out_bytes_per_s":0,"oom_kill_count":0},
 "disk_io":[{"device":"nvme0n1","util_percent":1.2,"await_ms":0.06,
 "read_bytes_per_s":3000000,"write_bytes_per_s":1000000,"iops":85}],
 "net":{"rx_bytes_per_s":50000000,"tx_bytes_per_s":20000000,"retrans_percent":0.1},
 "gpu_detail":[{"index":0,"power_w":402,"throttle_reasons":"0x0000000000000000"}],
 "extras_ms":1,"psi_cpu_some":0.02,"psi_mem_some":0.0,"psi_io_some":0.11}
```

`cpu` / `memory` / `gpu` are a **stable schema**: fields are never
renamed or removed, so downstream queries stay valid long-term. All
other fields are **extras, flattened to the top level** — downstream
tools read new metrics without changing query paths.

**Absence means unavailable.** A field is either present with real data
or missing entirely. No `null`, no `"unavailable"` pseudo-values.
Which fields are expected to exist on which host is the availability
matrix below; per-metric rationale and costs are in
[docs/metrics-research.md](docs/metrics-research.md).

Process lines: `{"ts_ms":…,"server":…,"total_procs":N,"processes":[
{"pid":1,"name":"init","cpu_percent":0.1,"mem_percent":0.05,
"cum_cpu_ms":123456,"state":"S"}]}` — top `top_n_processes` by CPU
then memory. `state`: `R`/`S`/`D`/`Z` (D = stuck in uninterruptible
I/O, Z = zombie).

### Extras fields

| Field | Source | Notes |
|---|---|---|
| `cpu_detail.iowait/steal/softirq_percent` | `/proc/stat` deltas | first tick of a run is missing (delta) |
| `cpu_detail.load_avg_5m/15m` | `/proc/loadavg` | |
| `cpu_detail.per_core_percent` | `/proc/stat` per-core deltas | sampled every 60 s (12th metrics tick); absent on other ticks by design — the shape is kept by the scalars below |
| `cpu_detail.per_core_max/p95_percent`, `cores_over_50` | same | every tick, < 50 B |
| `memory_detail.*` | `/proc/meminfo`, `/proc/vmstat` | swap rates are first-tick-missing deltas; `oom_kill_count` from vmstat (kernel ≥ 5.15) |
| `disk_io[]` | `/proc/diskstats` deltas | whole disks only (partitions, `loop*`, `dm-*`, `sr*` filtered); devices with no I/O in the window are omitted; counter regression (reboot) clamps to 0 |
| `net.*` | `/proc/net/dev` + `Tcp:` in `/proc/net/snmp` | `lo` excluded from the aggregate; missing on the first tick |
| `gpu_detail[].power_w` | NVML (local) / `nvidia-smi` (remote) | present only where a GPU is ok |
| `gpu_detail[].throttle_reasons` | `nvidia-smi` (remote) | NVML exposes no portable equivalent locally |
| `gpu_detail[].sm_active/occupancy/dram/tensor` | DCGM | **v2** — absent until a host runs DCGM; the `Option` slots are already in the schema |
| `slurm.*` | `sinfo`/`squeue` | only where slurmctld is detected; GPU sub-fields absent when the slurm gpu plugin is off |
| `psi_cpu/mem/io_some` | `/proc/pressure/*` (kernel ≥ 4.20) | flat keys; "some" pressure, 10 s average |
| `extras_ms` | measured | extras collection cost; the scheduler warns (rate-limited) when it exceeds 20 ms |

### GPU three-state

| Situation | `gpu.state` |
|---|---|
| no NVIDIA hardware / driver, remote without `nvidia-smi` | `absent` (warned once) |
| driver present, probe ok | `ok` |
| NVML / `nvidia-smi` runtime failure | `error` (warned, rate-limited; no retry storm) |

## Field availability matrix (measured 2026-11-16)

✓ = field lands on that host; — = field absent (deliberate
degradation, not a fault); * = missing on the first tick of a run
(delta metrics), present from the second tick on.

| Field | local | math2 | bot | amax |
|---|---|---|---|---|
| `cpu_detail.iowait/steal/softirq_percent`* | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.load_avg_5m/15m` | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.per_core_percent` (60 s) | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.per_core_max/p95/cores_over_50` | ✓ | ✓ | ✓ | ✓ |
| `psi_cpu_some` / `psi_mem_some` / `psi_io_some` | ✓ | ✓ | ✓ | ✓ |
| `memory_detail.*` | ✓ | ✓ | ✓ | ✓ |
| `disk_io[]` (after whole-disk filter) | 1 disk | 5 disks | 1 disk | 1 disk |
| `net.*`* | ✓ | ✓ | ✓ | ✓ |
| `gpu_detail[].power_w` | — (no GPU) | ✓ | — (no GPU) | ✓ |
| `gpu_detail[].throttle_reasons` | — | ✓ | — | ✓ |
| `gpu_detail[].sm_active` etc. (DCGM) | — (v2) | — (v2) | — (v2) | — (v2) |
| `slurm.*` | — (no sinfo) | — (no sinfo) | — (no sinfo) | — (no sinfo) |
| `ProcRow.cum_cpu_ms` / `.state` | ✓ | ✓ | ✓ | ✓ |

## Volume

Estimated with all extras on (per
[docs/metrics-research.md §6](docs/metrics-research.md)):

| Host | Per day (17 280 metrics ticks + 1 440 per-core ticks) |
|---|---|
| local | ~16 MB |
| math2 | ~37 MB |
| bot | ~16 MB |
| amax | ~20 MB |
| **Total** | **≈ 102 MB/day ≈ 3.0 GB/month** |

Measured on 2026-11-16 (12-min full-config run, all extras on):
local 16.4 / bot 15.7 / amax 18.5 / math2 26.8 MB/day —
**≈ 77.7 MB/day ≈ 2.3 GB/month**, within ±30% of the estimate.

Core-only (extras off) runs ~37 MB/day total, matching the "30 MB/day
class" target. Disabling `disk_io` + `gpu_detail` brings the full set
down to ≈ 78 MB/day. With `retention_days = 30` the peak on-disk
footprint is ≈ 3.0 GB.

## Downstream analysis

```bash
# DuckDB: CPU over time across all hosts
duckdb -c "SELECT ts_ms, server, cpu.global_percent
           FROM read_json_auto('data/*/*-metrics.jsonl')"

# jq: iowait history of one host
jq -c 'select(.server=="amax") | {ts: .ts_ms, iow: .cpu_detail.iowait_percent}' \
  data/*/amax-metrics.jsonl

# DuckDB: disks that were actually busy (util > 10%)
duckdb -c "SELECT ts_ms, server, d.device, d.util_percent, d.await_ms
           FROM read_json_auto('data/*/*-metrics.jsonl'),
                UNNEST(disk_io) t(d)
           WHERE d.util_percent > 10"
```

## Testing

```bash
cargo test
```

- unit tests: parsers, delta clamping, storage date partitioning/retention;
- `tests/no_gpu_smoke.rs`: runs the real binary on a GPU-less machine
  (skipped when a driver is present) — asserts `gpu.state == "absent"`,
  single NVML warn, clean SIGINT shutdown;
- `tests/extras_graceful.rs`: asserts that with every extras source
  unavailable the serialized line contains no extras fields at all —
  no `null`, no pseudo-values — and that counter regressions clamp
  instead of going negative.
