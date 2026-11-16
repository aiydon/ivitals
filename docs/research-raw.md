# ivitals metric source raw measurements

Date: Mon Nov 16 15:22:53 CST 2026 (local probes run 15:22–15:26 CST)

Local: `Linux valadu 6.12.116+deb13-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.12.116-1 (2026-11-07) x86_64 GNU/Linux`

Method notes:
- Read cost = 20 iterations of `t0=$(date +%s%N); cat FILE >/dev/null; t1=$(date +%s%N)`; ms integer-truncated per iteration; avg/max over n=20.
- Remote probes ran over ssh with `-o BatchMode=yes -o ConnectTimeout=10` using the alias default users (math2→`iydon@10.16.122.147`, bot→`sinotruk@frp.iydon.top:7105`, amax→`math@10.16.71.12`, per `~/.ssh/config`).
- "n/a (tool missing)" = `command -v` found no such binary on that host.

## LOCAL

Identity: host `valadu`, kernel 6.12.116+deb13-amd64, `nproc` = 16, `grep -c ^processor /proc/cpuinfo` = 16.

Tool presence:
```
nvidia-smi: MISSING
dcgmi: MISSING
sinfo: MISSING
squeue: MISSING
```

| source | present | cost_ms_avg | cost_ms_max | sample (first line, ≤200 chars) |
|---|---|---|---|---|
| /proc/stat | yes | 0.050 | 1 | `cpu  49586756 3944 6628306 518536525 1147288 0 409863 0 0 0` |
| /proc/loadavg | yes | 0.000 | 0 | `2.12 1.92 1.66 3/4015 3159944` |
| /proc/meminfo | yes | 0.000 | 0 | `MemTotal:       15712684 kB` |
| /proc/vmstat | yes | 0.000 | 0 | `nr_free_pages 382186` |
| /proc/diskstats | yes | 0.000 | 0 | ` 259       0 nvme0n1 86064692 13850535 5882481362 53280516 17052835 34422142 509996812 173424851 0 6210324 227506975 333670 0 143369608 115304 283893 686304` |
| /proc/net/snmp | yes | 0.000 | 0 | `Ip: Forwarding DefaultTTL InReceives InHdrErrors InAddrErrors ForwDatagrams InUnknownProtos InDiscards InDelivers OutRequests OutDiscards OutNoRoutes ReasmTimeout ReasmReqds ReasmOKs ReasmFails FragOK` |
| /proc/net/dev | yes | 0.000 | 0 | `Inter-|   Receive                                                |  Transmit` |
| /proc/pressure/cpu | yes | 0.100 | 2 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=1583467544` |
| /proc/pressure/memory | yes | 0.000 | 0 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=565905444` |
| /proc/pressure/io | yes | 0.000 | 0 | `some avg10=0.00 avg60=0.01 avg300=0.04 total=1253483568` |

Local bundle (single `bash -c 'cat <all 10 sources> >/dev/null'`), 3 runs: 5, 3, 3 ms.

### /proc/diskstats device names (3rd column, verbatim order)

```
nvme0n1
nvme0n1p1
nvme0n1p2
nvme0n1p3
```

### /proc/net/snmp field capture

Header lines present in this kernel's /proc/net/snmp (line numbers): `Ip:` (1,2), `Icmp:` (3,4), `IcmpMsg:` (5,6), `Tcp:` (7,8), `Udp:` (9,10), `UdpLite:` (11,12).

`Tcp:` header + value line (verbatim):
```
Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors
Tcp: 1 200 120000 -1 295501 36978 58841 55763 109 43630927 52541320 2173346 664 35129 0
```
So in `Tcp:`: field order is `... CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors`; sampled values: InSegs=43630927, OutSegs=52541320, RetransSegs=2173346.

`TcpExt:` line: **absent** on this local host. Verified: `grep -c 'TcpExt' /proc/net/snmp` → 0, and the full header inventory above shows no `TcpExt:` line.

## MATH2

Alias `math2` → `iydon@10.16.122.147` (per ~/.ssh/config).

Shipped `monitor` user test — verbatim failure (expected, recorded before probing with the alias default user):
```
$ ssh -o BatchMode=yes -o ConnectTimeout=5 monitor@math2 true
monitor@10.16.122.147: Permission denied (publickey,password).
(exit=255)
```

Identity:
```
hostname: user-PowerEdge-R7625
uname -r: 5.15.0-191-generic
nproc: 384
free -g | head -2 (verbatim):
               total        used        free      shared  buff/cache   available
内存：       1511         743          21           0         745         759
```
`ls /proc/pressure`: `cpu io memory`.

| source | present | cost_ms_avg | cost_ms_max | sample (first line, ≤200 chars) |
|---|---|---|---|---|
| /proc/stat | yes | 2.250 | 3 | `cpu  24470852929 550956174 1604472627 103409404962 3293518 0 9248933 0 0 0` |
| /proc/loadavg | yes | 1.200 | 3 | `46.29 46.12 45.89 10/23068 1674930` |
| /proc/meminfo | yes | 1.100 | 2 | `MemTotal:       1584774060 kB` |
| /proc/vmstat | yes | 1.150 | 2 | `nr_free_pages 5714339` |
| /proc/diskstats | yes | 2.350 | 4 | `   7       0 loop0 25 0 80 0 0 0 0 0 0 44 0 0 0 0 0 0 0` |
| /proc/net/snmp | yes | 1.750 | 3 | `Ip: Forwarding DefaultTTL InReceives InHdrErrors InAddrErrors ForwDatagrams InUnknownProtos InDiscards InDelivers OutRequests OutDiscards OutNoRoutes ReasmTimeout ReasmReqds ReasmOKs ReasmFails FragOK` |
| /proc/net/dev | yes | 2.650 | 4 | `Inter-|   Receive                                                |  Transmit` |
| /proc/pressure/cpu | yes | 1.400 | 2 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=9277036129` |
| /proc/pressure/memory | yes | 1.350 | 2 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=3502302852` |
| /proc/pressure/io | yes | 1.300 | 2 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=261840024` |

(These per-read costs include per-iteration remote shell/pipe overhead of the probe loop, not raw local-file read time.)

### /proc/diskstats (device names verbatim order; first data line has 20 fields)

```
loop0 loop1 loop2 loop3 loop4 loop5 loop6 loop7 nvme0c0n1 nvme0n1 nvme0n1p1 nvme0n1p2 nvme0n1p3 sdb sdb1 sda sda1 sdd sdd1 sdc sdc1 dm-0 dm-1 loop8 loop9 loop10 loop11 loop12 loop13 loop14 loop15 loop16 loop17 loop18 loop19 loop20 loop21 loop22 loop23 loop25 loop24 loop26 loop27 loop28 loop29 loop30
```
First data line (verbatim): `   7       0 loop0 25 0 80 0 0 0 0 0 0 44 0 0 0 0 0 0 0` → `wc -w` = 20.

### meminfo / vmstat
```
MemTotal:       1584774060 kB
MemAvailable:   796568316 kB
SwapTotal:      1994748 kB
SwapFree:             68 kB
oom_kill 6
```

### Shells / tools
```
bash --version | head -1: GNU bash，版本 5.1.16(1)-release (x86_64-pc-linux-gnu)   (full-width comma, zh locale — verbatim)
command -v awk: /usr/bin/awk
command -v jq: (no output → not installed)
```

### GPU
`command -v nvidia-smi` → `/usr/bin/nvidia-smi`. Query verbatim:
```
0, NVIDIA L20, 19 %, 469 MiB, 46068 MiB, 43
1, NVIDIA L20, 0 %, 17 MiB, 46068 MiB, 29
```
nvidia-smi query timed 3× (ms): 53, 54, 51.

### DCGM
`dcgmi: MISSING` (no `dcgmi discovery -l` run).

### Slurm
`command -v sinfo` → not installed; `command -v squeue` → not installed (no sinfo/squeue output).

### Candidate script cost
`bash -c 'cat <all 10 sources> >/dev/null'` 3× (ms): 5, 4, 4.

### SSH exec overhead (local-side wall clock, `ssh math2 true`)
334, 336, 306 ms (rc=0 each).

## BOT

Alias `bot` → `sinotruk@frp.iydon.top:7105` (frp tunnel, per ~/.ssh/config).

Identity:
```
hostname: bot-SUSTech
uname -r: 5.15.0-139-generic
nproc: 8
free -g | head -2 (verbatim):
              total        used        free      shared  buff/cache   available
Mem:              7           0           3           0           3           6
```
`ls /proc/pressure`: `cpu io memory`.

| source | present | cost_ms_avg | cost_ms_max | sample (first line, ≤200 chars) |
|---|---|---|---|---|
| /proc/stat | yes | 1.000 | 1 | `cpu  1402715 59442492 146983 413429909 1075774 0 7148 0 0 0` |
| /proc/loadavg | yes | 1.000 | 1 | `1.06 1.04 1.00 3/387 288964` |
| /proc/meminfo | yes | 1.000 | 1 | `MemTotal:        8008144 kB` |
| /proc/vmstat | yes | 1.000 | 1 | `nr_free_pages 894033` |
| /proc/diskstats | yes | 1.000 | 1 | `   7       0 loop0 14 0 34 68 0 0 0 0 0 80 68 0 0 0 0 0 0 0` |
| /proc/net/snmp | yes | 1.000 | 1 | `Ip: Forwarding DefaultTTL InReceives InHdrErrors InAddrErrors ForwDatagrams InUnknownProtos InDiscards InDelivers OutRequests OutDiscards OutNoRoutes ReasmTimeout ReasmReqds ReasmOKs ReasmFails FragOK` |
| /proc/net/dev | yes | 1.000 | 1 | `Inter-|   Receive                                                |  Transmit` |
| /proc/pressure/cpu | yes | 1.000 | 1 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=15816765` |
| /proc/pressure/memory | yes | 1.000 | 1 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=20824687` |
| /proc/pressure/io | yes | 1.000 | 1 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=593281188` |

### /proc/diskstats (device names verbatim order; first data line has 20 fields)

```
loop0 loop1 loop2 loop3 loop4 loop5 loop6 loop7 sda sda1 sda2 sr0 loop8 loop9 loop10 loop11 loop12 loop13 loop14 loop15 loop16 loop17 loop18 loop19 loop20 loop21 loop22 loop23 loop24 loop25 loop26 loop27 loop28 loop29 loop30
```
First data line (verbatim): `   7       0 loop0 14 0 34 68 0 0 0 0 0 80 68 0 0 0 0 0 0 0` → `wc -w` = 20.

### meminfo / vmstat
```
MemTotal:        8008144 kB
MemAvailable:    6734052 kB
SwapTotal:       8388604 kB
SwapFree:        8213516 kB
oom_kill 0
```

### Shells / tools
```
bash --version | head -1: GNU bash, version 5.0.17(1)-release (x86_64-pc-linux-gnu)
command -v awk: /usr/bin/awk
command -v jq: /usr/bin/jq
```

### GPU
`nvidia-smi: MISSING`.

### DCGM
`dcgmi: MISSING`.

### Slurm
`command -v sinfo` → not installed; `command -v squeue` → not installed.

### Candidate script cost
`bash -c 'cat <all 10 sources> >/dev/null'` 3× (ms): 3, 3, 2.

### SSH exec overhead (local-side wall clock, `ssh bot true`)
17163, 111, 111 ms (rc=0 each; run1 includes first-time connection/ControlMaster socket setup through the frp tunnel, runs 2–3 used the persisted master).

## AMAX

Alias `amax` → `math@10.16.71.12` (per ~/.ssh/config).

Identity:
```
hostname: amax
uname -r: 6.12.116+deb13-amd64
nproc: 32
free -g | head -2 (verbatim):
               total        used        free      shared  buff/cache   available
Mem:             125          21          51           1          55         104
```
`ls /proc/pressure`: `cpu io memory`.

| source | present | cost_ms_avg | cost_ms_max | sample (first line, ≤200 chars) |
|---|---|---|---|---|
| /proc/stat | yes | 5.450 | 6 | `cpu  26447338 2342 1254638 220718161 66850 0 26450 0 0 0` |
| /proc/loadavg | yes | 5.150 | 6 | `0.39 0.75 3.36 2/2991 370155` |
| /proc/meminfo | yes | 5.100 | 6 | `MemTotal:       131686884 kB` |
| /proc/vmstat | yes | 5.250 | 6 | `nr_free_pages 13536993` |
| /proc/diskstats | yes | 5.300 | 6 | ` 259       0 nvme0n1 479541 29864 41973108 51799 1239597 913107 103914554 2759297 0 663336 2948952 0 0 0 0 108063 137855` |
| /proc/net/snmp | yes | 5.300 | 6 | `Ip: Forwarding DefaultTTL InReceives InHdrErrors InAddrErrors ForwDatagrams InUnknownProtos InDiscards InDelivers OutRequests OutDiscards OutNoRoutes ReasmTimeout ReasmReqds ReasmOKs ReasmFails FragOK` |
| /proc/net/dev | yes | 4.250 | 6 | `Inter-|   Receive                                                |  Transmit` |
| /proc/pressure/cpu | yes | 3.650 | 6 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=119090545` |
| /proc/pressure/memory | yes | 5.150 | 6 | `some avg10=0.00 avg60=0.00 avg300=0.00 total=18` |
| /proc/pressure/io | yes | 3.700 | 5 | `some avg10=0.00 avg60=0.00 avg300=0.04 total=39107816` |

(amax per-read costs are inflated by the probe loop's remote-shell per-iteration overhead and the 10.16 LAN RTT; note /proc/pressure/memory total=18, verbatim, suggests a very recent boot at sample time.)

### /proc/diskstats (device names verbatim order; first data line has 20 fields)

```
nvme0n1 nvme0n1p1 nvme0n1p2
```
First data line (verbatim): ` 259       0 nvme0n1 479541 29864 41973108 51799 1239597 913107 103914554 2759297 0 663336 2948952 0 0 0 0 108063 137855` → `wc -w` = 20.

### meminfo / vmstat
```
MemTotal:       131686884 kB
MemAvailable:   109606228 kB
SwapTotal:      144855576 kB
SwapFree:       144855576 kB
oom_kill 0
```

### Shells / tools
```
bash --version | head -1: GNU bash 5.2.37(1)-release (x86_64-pc-linux-gnu)
command -v awk: /usr/bin/awk
command -v jq: (no output → not installed)
```

### GPU
`command -v nvidia-smi` → `/usr/bin/nvidia-smi`. Query verbatim:
```
0, NVIDIA GeForce RTX 5070 Ti, 0 %, 6975 MiB, 16303 MiB, 47.50
```
nvidia-smi query timed 3× (ms): 40, 34, 36.

### DCGM
`dcgmi: MISSING`.

### Slurm
`command -v sinfo` → not installed; `command -v squeue` → not installed.

### Candidate script cost
`bash -c 'cat <all 10 sources> >/dev/null'` 3× (ms): 11, 12, 11.

### SSH exec overhead (local-side wall clock, `ssh amax true`)
270, 286, 262 ms (rc=0 each).

## TIMINGS

All values in ms, verbatim from the runs above. "tool missing" = no such binary on that host (n/a).

| probe | local_ms | math2_ms | bot_ms | amax_ms |
|---|---|---|---|---|
| ssh exec `true` run1 | — | 334 | 17163 | 270 |
| ssh exec `true` run2 | — | 336 | 111 | 286 |
| ssh exec `true` run3 | — | 306 | 111 | 262 |
| bundle of 10 /proc sources (bash -c cat) run1 | 5 | 5 | 3 | 11 |
| bundle of 10 /proc sources (bash -c cat) run2 | 3 | 4 | 3 | 12 |
| bundle of 10 /proc sources (bash -c cat) run3 | 3 | 4 | 2 | 11 |
| nvidia-smi query run1 | tool missing | 53 | tool missing | 40 |
| nvidia-smi query run2 | tool missing | 54 | tool missing | 34 |
| nvidia-smi query run3 | tool missing | 51 | tool missing | 36 |
| dcgmi discovery -l | tool missing | tool missing | tool missing | tool missing |
| sinfo -h -o '%T %c %C %G' | tool missing | tool missing | tool missing | tool missing |
| squeue -h | tool missing | tool missing | tool missing | tool missing |
