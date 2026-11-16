# ivitals 指标调研报告

日期:2026-11-16 · 配套原始实测数据见 `docs/research-raw.md`(同目录,由调研 agent 在
15:22–15:26 CST 采集,含逐源耗时、逐主机样本原文)。

## 1. 核心问题

> CPU / 内存 / GPU 总利用率都不高时,数据要能区分"**真的空闲**"和
> "**某个隐藏子系统在饱和或异常**"。

七个调研方向(§7.1)逐一走过:CPU 细分状态、内存细分行为、磁盘 I/O、网络、
GPU 效率、调度器层(Slurm)、进程层补充。结论先行:**七个方向全部有可采
数据源,其中六个在 v1 落实现**(Slurm 模块实现并随 v1 交付,但在当前四台
主机上均无 Slurm 部署,字段恒缺失——模块就绪、探测即采)。

## 2. 实测环境(字段可用性矩阵的依据)

| 主机 | 内核 | 核数 | 内存 | GPU | nvidia-smi | dcgmi | sinfo | bash | awk | jq |
|---|---|---|---|---|---|---|---|---|---|---|
| local (valadu) | 6.12.116+deb13 | 16 | 15 GB | 无 | 无 | 无 | 无 | 5.2 | 有 | 无 |
| math2 (R7625) | 5.15.0-191 | **384** | 1.5 TB | 2× NVIDIA L20 | 有 (~52 ms) | 无 | 无 | 5.1 | 有 | **无** |
| bot (frp 隧道) | 5.15.0-139 | 8 | 8 GB | 无 | 无 | 无 | 无 | 5.0 | 有 | 有 |
| amax | 6.12.116+deb13 | 32 | 125 GB | RTX 5070 Ti | 有 (~36 ms) | 无 | 无 | 5.2 | 有 | **无** |

四台主机(含 local)**全部**具备:`/proc/stat`、`/proc/loadavg`、
`/proc/meminfo`、`/proc/vmstat`(含 `oom_kill`)、`/proc/diskstats`(20 字段,
≥4.18 版式)、`/proc/net/snmp`(含 `Tcp:` 段)、`/proc/net/dev`、
`/proc/pressure/{cpu,memory,io}`(PSI)。

关键负面发现:

- **DCGM 四台全无**(`dcgmi` 缺失)→ SM Active / Occupancy / DRAM Active /
  Tensor Active 在 v1 不可采,列入 v2(§5)。
- **Slurm 四台全无**(`sinfo`/`squeue` 缺失)→ Slurm 指标 v1 实现、恒降级。
- **`TcpExt:` 段在 local(6.12 内核)不存在**→ 重传率只能用 `Tcp:` 段的
  `RetransSegs`/`OutSegs`,不能用 `TCPRetransSegs`;解析器必须容忍 `TcpExt`
  缺失(四台里三台有、一台没有,不能假设统一)。
- **math2 384 核** → 逐核利用率数组每 tick 约 3.6 KB,是全量 extras 下
  最大的单项体积来源,直接影响 §6 的采样周期设计。
- **math2/bot 无 `jq`** → 远程脚本禁止依赖 jq;远程主机统一只允许
  `cat`/`grep`(POSIX coreutils,四台均在)做"取原始计数",**速率/百分比
  的算术全部放在 Rust 侧**(collector 实例缓存上一 tick 快照求差)。
- 注意:`monitor` 用户 SSH 登录 math2 被拒(publickey 未授权,原始记录见
  raw 文件);当前实际可用的是 `~/.ssh/config` 中的别名默认用户
  (math2→iydon、bot→sinotruk、amax→math)。`monitor` 账号上线后无需改
  代码,只改 `config.toml` 的 `ssh_user`。

## 3. 逐方向结论(§7.1)

| # | 方向 | 结论 | 判定 |
|---|---|---|---|
| 1 | CPU 细分状态 | `/proc/stat` 差分可得 iowait/steal/softirq 与逐核%;PSI `/proc/pressure/*` 四台全有、读取 ~0 ms | **v1 全采** |
| 2 | 内存细分行为 | meminfo(Buffers/Cached/SReclaimable、swap)+ vmstat(`pswpin`/`pswpout` 速率、`oom_kill`)+ PSI mem,四台全有 | **v1 全采** |
| 3 | 磁盘 I/O | diskstats 20 字段版式四台一致;util=`io_ticks` 差分/窗口,await=加权 IO 时间差/完成数差;NVMe 上 io_ticks 恒 0 的内核怪癖在 §7 记录 | **v1 采**(整盘过滤) |
| 4 | 网络 | snmp `Tcp:` 段四台都有(重传率)+ net/dev(吞吐,排除 lo);`TcpExt` 缺失时回退 `Tcp:` | **v1 采** |
| 5 | GPU 效率 | NVML/nvidia-smi 可得功耗与降频原因;DCGM 系指标(SM/Occupancy/DRAM/Tensor)四台无 DCGM | **v1 采 NVML 子集,DCGM 系延后 v2** |
| 6 | 调度器层 | 四台均无 Slurm;`sinfo` 输出解析器实现并随 v1 交付,探测到才采 | **v1 实现,当前恒降级** |
| 7 | 进程层补充 | `cum_cpu_ms` + `state`(R/S/D/Z)已纳入 `ProcRow`(`/proc/[pid]/stat` 现成,代价极低) | **v1 采**(已实现) |

## 4. 指标取舍表(§7.2)

成本口径:本地 = Rust 直接读 `/proc`(单文件 ≤0.1 ms,见 raw 文件 LOCAL 表);
远程 = 脚本内取数增量 + SSH 传输(SSH 往返本身 110–330 ms 已计入核心采集,
不算增量)。"缺失时行为"统一为:**整字段缺失**(不写 null、不写伪值)。

| 指标(落盘键) | 采集源 | 采集成本 | 本地 | 远程 | 额外部署 | 缺失行为 | 能解释的根因 | 判定 |
|---|---|---|---|---|---|---|---|---|
| `cpu_detail.iowait_percent` / `.steal_percent` / `.softirq_percent` | `/proc/stat` 差分 | 本地 <0.1 ms;远程 +~5 ms(脚本内 cat) | 是 | 是(同一脚本) | 无 | 首 tick / 源缺失 → 字段缺失 | CPU 其实在等 I/O,或被虚拟化偷走、被软中断吃满 | v1 |
| `cpu_detail.load_avg_5m` / `.load_avg_15m` | `/proc/loadavg` | ~0 | 是 | 是 | 无 | 字段缺失 | 排队深度趋势 | v1 |
| `cpu_detail.per_core_percent` | `/proc/stat` 逐核差分 | 本地 <0.1 ms;**384 核时行体积 +3.6 KB** | 是 | 是 | 无 | 字段缺失(见 §6 周期设计) | 核间不均/NUMA 偏斜:平均利用率低但个别核打满 | v1(降频 60 s) |
| `psi_cpu_some` / `psi_mem_some` / `psi_io_some`(扁平键) | `/proc/pressure/{cpu,memory,io}` avg10 | 本地 ~0.1 ms;远程 +~5 ms | 是 | 是 | 无(内核 4.20+,四台均满足) | 字段缺失 | "不可中断地等待资源"的直接度量,比利用率更贴近卡顿根因 | v1 |
| `memory_detail.cache_bytes` | meminfo Buffers+Cached+SReclaimable | ~0 | 是 | 是 | 无 | 字段缺失 | used 低≠无压力;cache 收缩速度是内存吃紧的前兆 | v1 |
| `memory_detail.swap_used_bytes` / `.swap_in_bytes_per_s` / `.swap_out_bytes_per_s` | meminfo + vmstat pswpin/pswpout 差分×4096 | ~0 | 是 | 是 | 无 | 首 tick 速率缺失 | 低 used 下频繁换页 = 应用实际在等内存 | v1 |
| `memory_detail.oom_kill_count` | vmstat `oom_kill` | ~0 | 是 | 是 | 无(内核 ≥5.15 均有) | 字段缺失 | 静默杀进程会伪装成"任务失败/掉线" | v1 |
| `disk_io[].util_percent` / `.await_ms` / `.iops` / `.read_bytes_per_s` / `.write_bytes_per_s` | diskstats 差分 | 本地 <0.1 ms;远程 +~5 ms | 是 | 是 | 无 | 首 tick / 无活动盘 → 整段缺失 | 任务在等盘(await 高)或盘被打满(util 高) | v1 |
| `net.rx_bytes_per_s` / `.tx_bytes_per_s` / `.retrans_percent` | net/dev + snmp Tcp 差分 | ~0 | 是 | 是 | 无 | 首 tick → 字段缺失 | 跨节点通信拖慢:带宽饱和或丢包重传 | v1 |
| `gpu_detail[].power_w` | NVML(本地)/ `nvidia-smi --query-gpu=power.draw`(远程) | 本地 <1 ms;远程并入既有 nvidia-smi 调用(~40–52 ms,不新增进程) | 是(有卡时) | 是(math2/amax) | 无 | 无卡/工具缺失 → 字段缺失 | 功耗远低于 TDP 而 util 高 = 带宽/依赖瓶颈;功耗骤降 = 降频 | v1 |
| `gpu_detail[].throttle_reasons` | `nvidia-smi --query-gpu=clocks_throttle_reasons_active` | 同上(同一进程) | 是 | 是 | 无 | 字段缺失 | "假忙":GPU 被温度/功耗墙/SW 降频,util 高但吞吐上不去 | v1 |
| `gpu_detail[].sm_active` / `sm_occupancy` / `dram_active` / `tensor_active` | 仅 DCGM(`dcgmi` 或 dcgm-exporter) | — | **四台均无 DCGM** | 同左 | 需部署 DCGM | 字段缺失 | SM 空转率:util 高但 SM active 低 = 内核启动/PCIe/内存带宽瓶颈 | **v2**(当前无一处可采,部署成本>收益) |
| `slurm.nodes_alloc/nodes_idle` / `cpus_alloc/cpus_total` / `gpus_alloc/gpus_total` / `jobs_running/jobs_pending` | `sinfo -h -o …`(探测到才采,结果缓存) | 探测 `command -v sinfo` ~5 ms,缓存后 0;采集时 sinfo ~100–300 ms | 是(装 Slurm 时) | 是 | 需 Slurm 集群 | 探测失败 → 字段缺失 | "分配了没用"(alloc>>util)或"根本没分出去"(alloc 低、pending 高) | v1 实现 / 当前恒降级 |
| `ProcRow.cum_cpu_ms` / `.state` | `/proc/[pid]/stat` | 进程 tick 内每进程 2 个字段,~0 | 是 | 是(ps 输出扩展) | 无 | 字段缺失 | D 状态堆积 = I/O 阻塞;Z 状态 = 僵尸泄漏;累计 CPU 时间识别长期低效占用 | v1(已实现) |

**放弃项**(给理由):
- `TcpExt` 细粒度重传(`TCPRetransSegs`):local 内核上该段不存在,为
  统一性放弃,用 `Tcp:` 段的粗粒度重传率(信息量 80%,成本相同);
- `diskstats` 的 discard/flush 计数:对"使用率为什么上不去"无直接解释力,
  且体积白付;
- 每核 iowait:总 iowait 已足够定位,逐核细分收益边际、体积翻倍;
- `/proc/uptime`、`/proc/net/arp` 等:与核心问题无关。

## 5. DCGM 延后 v2 的说明

硬约束禁止 DCGM C 绑定进 `[dependencies]`,只允许运行时探测(`dcgmi` 输出
或 dcgm-exporter HTTP 端点)。实测四台主机 `dcgmi` 全部缺失,且
dcgm-exporter 需要额外部署一个常驻 Prometheus exporter——**当前没有任何
一台机器能采到 DCGM 字段,先写解析器是纯投机代码**。故 v1 的 `gpu_detail`
只含 NVML 可得子集(`power_w`、`throttle_reasons`),结构体保留
`sm_active/sm_occupancy/dram_active/tensor_active` 的 `Option` 字段
(`skip_serializing_if`,不产生伪值),v2 在任一台部署 DCGM 后由
"运行时探测 + 解析 `dcgmi dmon`"补齐,不动 schema。

## 6. 性能与体积预算核算

**本地 p99 增量**:extras 全部走 Rust 直读 `/proc`,单文件 0–0.1 ms
(raw LOCAL 表),每 tick 共 ~8 个文件 + 差分算术 ≈ **<1 ms**,远低于 5 ms
预算。远程:脚本新增部分 = 各 `cat`/`grep` 的捆绑(实测 3 台 3–12 ms,
raw TIMINGS 表)+ 已有 nvidia-smi 调用中并入两个查询字段(不新增进程)
≈ **<60 ms**,远低于 1 s 预算。SSH 往返(~110–330 ms)属核心采集既有成本。

**math2 384 核的逐核数组**:全量 `per_core_percent` 每 tick 约 3.6 KB,
17 280 tick/天 ≈ 62 MB/天,一个字段超过整机其余全部 extras。逐核分布对
"为什么上不去"问题的贡献是**形状信息**(偏不偏斜、哪段核热),5 s 粒度
远超所需。设计:`per_core_percent` 每 12 个 metrics tick(60 s,与进程
快照同频)采一次,其余 tick 该字段缺失(缺失=本轮未采,非伪值);
`core_count ≤ 64` 的主机(local/bot/amax)同样 60 s 一次,保持口径统一。
大核数主机另有 `per_core_max_percent` / `per_core_p95_percent` /
`cores_over_50` 三个标量**每 tick** 提供,体积 <50 B,保住分布形状信号。

**新增依赖:0 个。** 全部指标 = `/proc` 直读 + 既有依赖(sysinfo 已覆盖
核心块;差分/解析为纯函数)+ `std::process::Command`(远程走既有
ssh2 脚本;Slurm/DCGM 探测未来也走 Command,不引 crate)。满足
§10 的"能不加 crate 就不加",上限 ≤3 用掉 0。

**体积预估**(行尺寸为 2026-11-16 实测:core metrics 行 269–274 B
无卡 local;带 2 卡 GPU 的 ok 行 ≈470 B;20 进程 processes 行 2 307 B;
extras 各段按 §4 键名与样本值构造估算):

| 主机 | 每 metrics 行(extras 全开,稳态,不含 60 s 逐核数组) | /天(17 280 tick + 1 440 逐核 tick) |
|---|---|---|
| local | ~900 B | 16.0 MB |
| math2 | ~1.8 KB | 36.6 MB |
| bot | ~900 B | 15.9 MB |
| amax | ~1.1 KB | 19.8 MB |
| processes 4 台 | 2.3 KB × 1 440 × 4 | 13.3 MB |
| **合计** | | **≈ 102 MB/天 ≈ 3.0 GB/月** |

对照目标书中的 ~40 MB/天:该数字近似于**本队列关 extras 时的体积**
(实测 core-only ≈ 24 MB/天 + 13 MB 进程 ≈ 37 MB/天,吻合"30 MB/天量级"
描述);extras 的增量(≈+64 MB/天)主要来自 math2 的 5 块整盘
`disk_io` 段、逐核数组(60 s 频率)与 GPU 两卡。关 `disk_io`+`gpu_detail`
回落至 ≈ 78 MB/天。30 天 retention 峰值磁盘占用 ≈ **3.0 GB**,可控。
验收 #9 以本表为基准(±30%)。

## 7. 已知内核/工具怪癖(解析器必须容忍)

1. **NVMe `io_ticks`(第 10 字段)在部分内核恒 0** → `util_percent` 算出
   0。这是内核报告值而非伪值,照实落盘;`await`/`iops`/吞吐不受影响。
2. **`TcpExt:` 段存在与否随内核而异**(local 无)→ 解析先找 `TcpExt`,
   缺失回退 `Tcp:`。
3. **diskstats 设备名集合差异大**:math2 有 40+ 个 loop/dm/分区;过滤
   规则(结构化、非按活动量):跳过 `loopN`、`dm-N`、`srN`(光驱),以及
   "父设备也在列表中"的分区(如 `nvme0n1p1`→`nvme0n1` 存在则跳)。
   整盘保留,含 math2 的 `nvme0c0n1`(新命名法,无父项)。
4. **`oom_kill` 需内核 ≥5.15**:四台均满足;更老内核无该字段 →
   `memory_detail` 整段缺失(不写 0)。
5. **bot 走 frp 隧道,首连 17 s、之后 ~110 ms**(ControlMaster 复用,
   raw TIMINGS)→ 采集器对首 tick 超时要宽容(既有 `connect_timeout_secs`
   /`exec_timeout_secs` 机制已覆盖,不重试不刷屏)。
6. **`/proc/stat` 首行字段数**:四台均 11 值(user…steal),softirq 列
   存在;解析按列名位置取,不假设总长。

## 8. 字段可用性矩阵(实测)

✓ = 该主机该字段会落盘;— = 字段缺失(显式降级,非故障);* = 首 tick 缺
(差分类指标需上一轮快照),次 tick 起 ✓。

| 字段 | local | math2 | bot | amax |
|---|---|---|---|---|
| `cpu_detail.iowait/steal/softirq_percent`* | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.load_avg_5m/15m` | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.per_core_percent`(60 s 频率) | ✓ | ✓ | ✓ | ✓ |
| `cpu_detail.per_core_max/p95/cores_over_50` | ✓ | ✓ | ✓ | ✓ |
| `psi_cpu_some` / `psi_mem_some` / `psi_io_some` | ✓ | ✓ | ✓ | ✓ |
| `memory_detail.*` | ✓ | ✓ | ✓ | ✓ |
| `disk_io[]`(整盘过滤后) | 1 盘 | 5 盘 | 1 盘 | 1 盘 |
| `net.*`* | ✓ | ✓ | ✓ | ✓ |
| `gpu_detail[].power_w` | —(无卡) | ✓ | —(无卡) | ✓ |
| `gpu_detail[].throttle_reasons` | — | ✓ | — | ✓ |
| `gpu_detail[].sm_active` 等 DCGM 系 | — (v2) | — (v2) | — (v2) | — (v2) |
| `slurm.*` | —(无 sinfo) | —(无 sinfo) | —(无 sinfo) | —(无 sinfo) |
| `ProcRow.cum_cpu_ms` / `.state` | ✓ | ✓ | ✓ | ✓ |

## 9. v1 选入清单(代码按此实现)

1. `CpuDetail`:iowait / steal / softirq / load5 / load15 / per_core(60 s
   频率,`Option`) / per_core_max / per_core_p95 / cores_over_50 —— §3#1、#7 方向
2. `MemoryDetail`:cache / swap_used / swap_in·out 速率 / oom_kill —— 方向 #2
3. `DiskIo`[](整盘过滤):util / await / iops / r·w 吞吐 —— 方向 #3
4. `NetStat`:r·x 吞吐 / retrans%(Tcp 回退) —— 方向 #4
5. `GpuDetail`[]:power_w / throttle_reasons(DCGM 系保留 `Option` 空位) —— 方向 #5 的 NVML 子集
6. `SlurmStat`:nodes/cpus/gpus alloc·total + jobs_running/pending(探测到 sinfo 才采) —— 方向 #6
7. 扁平 `extra` 键:`psi_cpu_some` / `psi_mem_some` / `psi_io_some` —— PSI 走
   BTreeMap 通道,验证"新指标不破坏 schema"的设计
8. `ProcRow.cum_cpu_ms` / `.state` —— 方向 #7(已随核心模型交付)

**v2**:DCGM 四字段(待任一台部署 DCGM,运行时探测 `dcgmi`);文件压缩;
**放弃**:见 §4 放弃项。

## 10. 远程脚本协议(与 C13 实现绑定)

`collect.sh` 按 `[extras]` 标志接收 `--cpu-detail --memory-detail
--disk-io --net --gpu-detail --slurm` 参数,stdout 输出**分节文本**(不是
JSON,避免在 bash 里拼 JSON 的引号地狱):

```
## cpu_detail
<cat /proc/stat>
<cat /proc/loadavg>
## memory_detail
<cat /proc/meminfo>
<grep -E '^(oom_kill|pswpin|pswpout) ' /proc/vmstat>
## psi
cat /proc/pressure/{cpu,memory,io} 各带文件名头
## disk_io
<cat /proc/diskstats>
## net
<cat /proc/net/dev>
<grep -A1 '^Tcp:' /proc/net/snmp>
## gpu_detail
<nvidia-smi --query-gpu=index,power.draw,clocks_throttle_reasons_active -format=csv,noheader,nounits>(有卡时)
## slurm
<sinfo -h -o '...'> (有 sinfo 时)
```

Rust 侧:`collector/extras/*` 每个模块 = **纯解析函数**(输入分节文本 →
`Option<T>`,local 与 remote 共用)+ local 直读采样;`RemoteCollector` /
`LocalCollector` 各自缓存上一 tick 的原始计数器快照,差分求速率。某节
缺失 → 对应 `Option::None` → 字段不落盘。脚本只用 cat/grep,远程执行
增量 <60 ms(§6)。
