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
schema_version = 2

[global]
interval_secs = 5          # metrics tick period (>= 2)
process_interval_secs = 60 # process snapshot period (>= interval_secs)
top_n_processes = 50       # per snapshot, by CPU then memory
retention_days = 30        # whole UTC date dirs older than this are removed
data_dir = "data"          # relative to the working directory, or absolute
log_level = "info"

[extras]                   # every key optional; defaults shown
cpu_detail = true          # iowait/steal/softirq, load 5m/15m, full per-core vector
memory_detail = true       # cache, swap used + in/out rates, oom_kill count
disk_io = true             # per-whole-disk util/await/iops/throughput
net = true                 # aggregate + per-interface rates + TCP retransmission %
gpu_detail = "auto"        # auto | dcgm | off
slurm = "auto"             # auto | off (only collected when sinfo exists)
psi = true                 # flat psi_cpu_some / psi_mem_some / psi_io_some
fs_space = true            # per-mount capacity/usage (statvfs / df)

[[servers]]
alias = "sustech"          # unique, used in file names
mode = "remote"            # remote | local
ssh_host = "10.16.122.147" # required for remote (ssh config alias ok)
ssh_user = "monitor"       # required for remote
ssh_port = 7105            # optional; defaults to ssh config Port, else 22
connect_timeout_secs = 5
exec_timeout_secs = 15
```

`ssh_host` may be a literal host or an alias from `~/.ssh/config`:
ivitals resolves `HostName`, `Port`, and `IdentityFile` from the first
matching `Host` block (the config file is read once at startup).
`ssh_user` always wins over a `User` line in the ssh config, and an
explicit `ssh_port` always wins over its `Port` line. Without a matching
block the host is used literally on port 22 (or `ssh_port`). The shipped
`config.toml` uses literal hosts so it also runs on machines without
the same ssh config.

An extras family set to `false`/`"off"` is not collected at all: no
fields, no cost. Availability of the optional sources (NVIDIA driver,
DCGM, slurm, `/proc/pressure`) is probed once and cached; a missing
source simply omits the corresponding fields.

## Data layout

```
data/
├── 2026-11-15/
│   ├── sustech-metrics.jsonl
│   ├── sustech-processes.jsonl
│   ├── sustech-host.jsonl
│   └── local-metrics.jsonl
└── 2026-11-21/
    ├── sustech-metrics.jsonl
    └── local-processes.jsonl
```

- One directory per UTC day (`YYYY-MM-DD`); files are
  `{alias}-{metrics|processes|host}.jsonl`. Nothing is ever written at
  the root of `data/`.
- `*-host.jsonl` holds one identity record per run and per UTC day:
  hostname, OS, kernel, arch, CPU model, core count, memory total, boot
  time and GPU model names.
- The writer is a single thread; it rolls to a new directory at the UTC
  midnight crossing (derived from each record's `ts_ms`) and flushes
  every 30 s or 200 records.
- Retention: at startup and every 24 h, date directories strictly older
  than `retention_days` are removed wholesale. Non-date entries are
  never touched.

## Schema

Every metrics line:

```json
{"schema_version":2,"ts_ms":1763300000000,"server":"sustech","mode":"remote",
 "collect_ms":4,"cpu":{"global_percent":12.5,"core_count":96,"load_avg_1m":3.2},
 "memory":{"total_bytes":766520000000,"used_bytes":300000000000,"available_bytes":466000000000},
 "gpu":{"state":"ok","gpus":[{"index":0,"name":"NVIDIA A800 80GB","util_percent":98,
 "mem_used_mb":70000,"mem_total_mb":81920,"temp_c":62,"mem_util_percent":85.4,
 "power_w":402.1,"power_limit_w":500,"fan_percent":70,"sm_clock_mhz":1410,
 "mem_clock_mhz":1593,"pstate":"P0"}]},
 "cpu_detail":{"iowait_percent":0.4,"steal_percent":0.0,"softirq_percent":0.9,
 "load_avg_5m":3.0,"load_avg_15m":2.8,"per_core_percent":[12.1,0.0,71.2],
 "per_core_min_percent":0.0,"per_core_mean_percent":18.2,"per_core_p50_percent":9.4,
 "per_core_max_percent":71.2,"per_core_p95_percent":35.0,"cores_over_50":3,
 "cores_over_90":0,"busy_histogram":[60,20,5,3,1,0,0,0,0,1]},
 "memory_detail":{"cache_bytes":88000000000,"buffers_bytes":900000000,
 "slab_bytes":4000000000,"dirty_bytes":0,"writeback_bytes":0,
 "swap_total_bytes":0,"swap_used_bytes":0,
 "swap_in_bytes_per_s":0,"swap_out_bytes_per_s":0,"oom_kill_count":0},
 "disk_io":[{"device":"nvme0n1","util_percent":1.2,"await_ms":0.06,
 "read_bytes_per_s":3000000,"write_bytes_per_s":1000000,"iops":85}],
 "net":{"rx_bytes_per_s":50000000,"tx_bytes_per_s":20000000,"retrans_percent":0.1,
 "interfaces":[{"name":"eth0","rx_bytes_per_s":50000000,"tx_bytes_per_s":20000000,
 "rx_packets_per_s":40000,"tx_packets_per_s":20000,"rx_errors_per_s":0,
 "tx_errors_per_s":0,"rx_drops_per_s":0,"tx_drops_per_s":0}]},
 "gpu_detail":[{"index":0,"power_w":402,"fan_percent":70,"sm_clock_mhz":1410,
 "mem_clock_mhz":1593,"pstate":"P0"}],
 "filesystems":[{"device":"/dev/nvme0n1p1","mount":"/","fstype":"ext4",
 "total_bytes":1821843804160,"used_bytes":300000000000,"available_bytes":1521843804160,
 "used_percent":16.5,"inodes_total":113041408,"inodes_free":111534022}],
 "extras_ms":1,
 "psi_cpu_some":0.02,"psi_mem_some":0.0,"psi_io_some":0.11}
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

Process lines: `{"schema_version":2,"ts_ms":…,"server":…,"total_procs":N,
"total_threads":N,"state_counts":{"S":123,"R":4},"gpu_processes":[
{"gpu_index":0,"gpu_uuid":"GPU-…","pid":1,"name":"trainer","mem_mb":40000,"kind":"compute"}],
"processes":[{"pid":1,"name":"init","user":"root","uid":0,"gid":0,
"ppid":0,"parent_name":"systemd","cmdline":"/sbin/init quiet",
"cwd":"/","exe":"/sbin/init","cpu_percent":0.1,"mem_percent":0.05,
"rss_bytes":12345678,"threads":4,"start_time_ms":…,"cum_cpu_ms":123456,
"state":"S"}]}` — top `top_n_processes` by CPU then memory. User/UID/GID,
parent, full command line, working directory and executable are recorded
for every top-N row (bounded: cmdline 512 chars, cwd/exe 256). `state`: `R`/`S`/`D`/`Z` (D = stuck in uninterruptible
I/O, Z = zombie).

Host identity line (one per run/day):

```json
{"schema_version":2,"ts_ms":1763300000000,"server":"valadu","mode":"remote",
 "hostname":"valadu","os":"Debian GNU/Linux 13 (trixie)",
 "kernel":"6.12.117+deb13-amd64","arch":"x86_64",
 "cpu_model":"AMD Ryzen 7 5800H with Radeon Graphics","core_count":16,
 "mem_total_bytes":16089804800,"boot_time_ms":1763200000000,
 "uptime_secs":100000}
```

### Extras fields

| Field | Source | Notes |
|---|---|---|
| `cpu_detail.iowait/steal/softirq_percent` | `/proc/stat` deltas | first tick of a run is missing (delta) |
| `cpu_detail.load_avg_5m/15m` | `/proc/loadavg` | |
| `cpu_detail.per_core_percent` | `/proc/stat` per-core deltas | **every tick**, rounded to 2 decimals; 32–384 cores |
| `cpu_detail.per_core_min/mean/p50/max/p95`, `cores_over_50/90`, `busy_histogram` | same | every tick; compact distribution summary |
| `memory_detail.*` | `/proc/meminfo`, `/proc/vmstat` | cache/buffers/slab/dirty/writeback, swap total/used/rates, oom_kill; rates are first-tick-missing deltas |
| `disk_io[]` | `/proc/diskstats` deltas | whole disks only (partitions, `loop*`, `dm-*`, `sr*` filtered); devices with no I/O in the window are omitted; counter regression (reboot) clamps to 0 |
| `net.*` | `/proc/net/dev` + `Tcp:` in `/proc/net/snmp` | aggregate excludes `lo`; `net.interfaces[]` keeps per-NIC bytes/packets/errors/drops per second |
| `filesystems[]` | local `statvfs` / remote `df -P -T -B1` | per-mount total/used/available/inodes; pseudo filesystems filtered |
| `gpu_detail[].power_w/fan/clocks/pstate` | NVML (local) / `nvidia-smi` (remote) | present only where a GPU is ok |
| `gpu_detail[].throttle_reasons` | `nvidia-smi` (remote) | `0x000…0` / `[Not Active]` normalized to absent |
| `gpu_detail[].sm_active/occupancy/dram/tensor` | DCGM | absent until a host runs DCGM; `dram_active` is filled from NVML memory utilization locally |
| `extras_errors` | measured | number of enabled families with non-empty input that failed to parse this tick |
| `*-host.jsonl` | `/proc`, `/etc/os-release`, `uname`, `nvidia-smi` | one identity line per run/day; never repeated on 5 s metrics ticks |
| `slurm.*` | `sinfo`/`squeue` | only where slurmctld is detected; GPU sub-fields absent when the slurm gpu plugin is off |
| `psi_cpu/mem/io_some` | `/proc/pressure/*` (kernel ≥ 4.20) | flat keys; "some" pressure, 10 s average |
| `extras_ms` | measured | extras collection cost; the scheduler warns (rate-limited) when it exceeds 20 ms |

### GPU three-state

| Situation | `gpu.state` |
|---|---|
| no NVIDIA hardware / driver, remote without `nvidia-smi` | `absent` (warned once) |
| driver present, probe ok | `ok` |
| NVML / `nvidia-smi` runtime failure | `error` (warned, rate-limited; no retry storm) |

## Field availability matrix (v2 run, 2026-11-21)

✓ = observed in the regenerated v2 data; — = absent (deliberate
degradation, not a fault); * = missing on the first tick of a run
(delta metrics).

| Field | sustech | sinotruk | local |
|---|---|---|---|
| per-core vector / histogram | ✓ 384 cores | ✓ 8 cores | ✓ 32 cores |
| `cpu_detail.iowait/steal/softirq_percent`* | ✓ | ✓ | ✓ |
| `psi_*_some` | ✓ | ✓ | ✓ |
| `memory_detail.*` | ✓ | ✓ | ✓ |
| `filesystems[]` | 6 mounts | 2 mounts | 1 mount |
| `net.interfaces[]` | 9 NICs | 2 NICs | 2 NICs |
| GPU state | 2 × L20 `ok` | `absent` | 1 × RTX 5070 Ti `ok` |
| GPU power/clocks/pstate | ✓ | — (no GPU) | ✓ |
| GPU processes | 2 compute | — | 1 compute + 4 graphics |
| `slurm.*` | — (no sinfo) | — (no sinfo) | — (no sinfo) |
| `ProcRow.rss_bytes/threads` | ✓ | ✓ | ✓ |

## Volume

Measured from the v2 run with per-core recorded every tick, all extras on
(5 s metrics, 60 s process snapshots):

| Host | Metrics line | Per day (17 280 ticks) |
|---|---|---|
| sustech (384 cores, 2 GPUs) | ≈ 5.6 KB | ≈ 97 MB |
| sinotruk (8 cores) | ≈ 1.9 KB | ≈ 33 MB |
| local (32 cores, 1 GPU) | ≈ 2.3 KB | ≈ 40 MB |
| **Metrics total** | | **≈ 170 MB/day** |
| Process snapshots (top 50 with user/cmdline/cwd) | ≈ 12–25 KB each | ≈ 18–36 MB/day/host |

With `retention_days = 30`, expect roughly **9–10 GB** on disk for five
hosts. The per-core vector and process command lines are the dominant
costs; set `cpu_detail = false` if per-core history is not needed, or
lower `top_n_processes`. Command lines may contain secrets, so protect
the data directory accordingly.

## Downstream analysis

```bash
# DuckDB: CPU over time across all hosts
duckdb -c "SELECT ts_ms, server, cpu.global_percent
           FROM read_json_auto('data/*/*-metrics.jsonl')"

# jq: iowait history of one host
jq -c 'select(.server=="local") | {ts: .ts_ms, iow: .cpu_detail.iowait_percent}' \
  data/*/local-metrics.jsonl

# DuckDB: disks that were actually busy (util > 10%)
duckdb -c "SELECT ts_ms, server, d.device, d.util_percent, d.await_ms
           FROM read_json_auto('data/*/*-metrics.jsonl'),
                UNNEST(disk_io) t(d)
           WHERE d.util_percent > 10"
```

## Dashboard (read-only)

`dashboard/` is a Bun + React + ECharts web UI over `data/`. It never writes
to the data directory and reads JSONL incrementally (tail reads for the
overview/process snapshot; Workers + streaming line readers for GPU/time
series and per-core heatmaps). The UI is a graphite instrument panel with a
fleet overview that aggregates a chosen **local** date range for reporting, a
host scope with its own tabs (no cross-host numeric comparison), dark/light
themes, 60 s auto-refresh, one-click date presets, per-GPU detail, per-core
occupancy heatmaps, filesystem capacity, disk throughput plus filesystem-usage trends, CPU/memory sparklines,
hoverable absolute disk/memory/GPU figures, GPU processes and explicit
data-gap warnings. Date pickers and chart axes use the browser's local timezone while
the on-disk partitions stay UTC. Visual direction follows
[unslop](https://github.com/heydamir/unslop) by Damir Sokolovsky (CC BY 4.0).

```bash
make install   # bun install
make dev       # binds 0.0.0.0:8899 by default
make build     # dashboard/dist
make start     # serve the production build
make clean     # removes dashboard/dist only — never data/
```

The server binds to `0.0.0.0` so the internal network can reach it; the startup
log prints every LAN URL. Override with `HOST=127.0.0.1` or `PORT=...`.

**Security:** the dashboard has no authentication and exposes hostnames, process
command lines and filesystem paths. Run it only on a trusted network (or put it
behind an authenticated reverse proxy), and keep `data/` readable to that
audience only.

See `dashboard/README.md` for the API, environment overrides
(`IVITALS_CONFIG`, `IVITALS_DATA_DIR`, `PORT`, `HOST`) and scan limits.

## Operations

```bash
make collector-start   # detached collector; log: ivitals.log, pid: ivitals.pid
                       # run exactly ONE collector per data directory
make collector-stop
make start-bg          # detached dashboard on 0.0.0.0:8899
make stop
```

Both run outside the terminal (`setsid nohup`), so closing SSH does not stop
monitoring. The dashboard reconnects automatically after a restart.

Each host runs in its own tick task, so a slow SSH host only delays itself
(`MissedTickBehavior::Skip` drops its own backlog). Blocking threads are sized
from the server count (64–1024), keeping a large fleet away from the default
512-thread cap; the main loop only waits on signals and writer health.

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
