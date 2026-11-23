# ivitals dashboard

只读可视化看板：集群总览（汇报用）、单机硬件身份、GPU/文件系统/多核统计、CPU/内存/iowait/GPU 时序、多核热力图、进程与 GPU 进程快照。视觉方向参考 [unslop](https://github.com/heydamir/unslop)（Damir Sokolovsky，CC BY 4.0）。

## 技术栈

- **Bun 1.4+**：HTTP API、静态资源、依赖管理（`bun install`）
- **React 19 + ECharts 6**：前端
- **Worker + `readline` 流式扫描**：时序与多核查询分别在独立 Worker 中逐行解析 JSONL，
  按时间桶聚合，内存占用与文件大小无关，不阻塞 API 事件循环
- **集群总览（历史分析）**：CPU 均值/p95 + 走势图、内存均值 + 走势图、GPU/磁盘/覆盖；
  磁盘显示范围末条实时使用率，鼠标悬停可看“已用 / 总量”和实时读写；CPU/内存/GPU 悬停同样给出区间值与最新绝对量
- **导航结构**：侧边栏只有“总览 + 选择主机”；进入某台主机后出现“概览/时序/多核/进程”标签页和返回总览的面包屑
- **日期选择**：本地日期；最新一天 / 最近 3 天 / 最近 7 天 / 全部 一键切换，原始输入放在“自定义”里；
  进程快照用带星期的下拉日期
- **时区**：磁盘仍按 UTC 分区，看板把日期范围换算成本地日历日并在服务端过滤（`tzOffset` 参数）
- **深色 / 浅色**：跟随系统，可手动切换并持久化
- **自动刷新**：overview 30s，时序/多核/进程 60s；缺口检测会显示中断区间，图表在缺口处断开
- **进程详情**：表格行可展开，查看用户/UID、父进程、完整命令行、工作目录 `cwd`、可执行文件 `exe`
- **磁盘可视化**：时序页新增“磁盘吞吐 + 最高盘使用率”图（读/写 B/s 与百分比双轴）；
  主机概览显示当前读/写/util 与每个挂载点容量条

## 快速开始

在仓库根目录：

```bash
make install   # cd dashboard && bun install
make dev       # http://<LAN-IP>:8899（默认绑定 0.0.0.0）
make build     # dashboard/dist（生产静态文件）
make start     # 生产模式，读取 dashboard/dist（前台）
make start-bg  # 生产模式，后台常驻并写 dashboard/ivitals.log / ivitals.pid
make stop      # 停止 start-bg 启动的进程
make clean     # 仅删除 dashboard/dist，不触碰 ../data
```

开发模式使用 Bun 的 HTML import 自动打包；生产模式由 `src/server.ts` 直接托管
`dashboard/dist`。端口/监听地址可用 `PORT`、`HOST` 覆盖；默认 `0.0.0.0:8899`，
启动日志会打印所有内网地址。

长时间运行时用 `make start-bg`：进程以 `setsid nohup` 脱离终端，关闭 SSH 或终端不会退出；
日志在 `dashboard/ivitals.log`。看板掉线后前端会显示“服务未连接”，每 5 秒自动重试。

**安全**：看板没有鉴权，会暴露主机名、进程命令行和路径。只在可信内网使用，
或放到带鉴权的反向代理后面；`data/` 的读权限也应限制在同样范围。

## 只读保证

- 看板只调用 `readdir` / `stat` / `open(..., "r")` / `Bun.file(...).text()`；
  **没有任何写、删除、重命名、锁文件的代码路径**。
- `make clean` 的目标写死为 `dashboard/dist`，不会出现 `data/`。
- 总览只读取每个 metrics 文件的尾部 256 KiB；进程快照读取尾部 2 MiB。
- 时序扫描有硬限制：单次最多 31 个日历日、3,000,000 行、1 GiB、60s；超出会拒绝或截断并提示。

## 配置与数据目录

默认：

- 配置：`../config.toml`
- 数据：`../data/`
- 日期目录：`data/YYYY-MM-DD/`（UTC）
- 文件：`{alias}-metrics.jsonl` / `{alias}-processes.jsonl` / `{alias}-host.jsonl`

可用环境变量覆盖：

```bash
IVITALS_CONFIG=/path/to/config.toml \
IVITALS_DATA_DIR=/path/to/data \
PORT=9000 \
HOST=127.0.0.1 \
bun src/server.ts
```

服务器列表来自 `config.toml` 的 `[[servers]]`；数据目录中出现、但配置中没有的
alias 也会列出（标记为 `unknown`），并在 UI 顶部提示。

## API

| 路径 | 说明 |
|---|---|
| `GET /api/meta?tzOffset=` | 配置、数据目录、**本地**日期列表、服务器列表、告警 |
| `GET /api/overview` | 最后一条 metrics + host 身份：CPU/内存/iowait、per-core 统计、GPU 三态与逐卡详情、文件系统、最后在线 |
| `GET /api/series?server=&from=&to=&maxPoints=&tzOffset=` | 本地日范围的 CPU/内存/iowait/load1 + GPU 时间桶，含 coverage/gaps |
| `GET /api/cores?server=&from=&to=&maxFrames=&tzOffset=` | 本地日范围的 per-core 热力图帧、每核平均/峰值 |
| `GET /api/processes?server=&date=&top=&tzOffset=` | 本地日最后一个进程快照（跨两个 UTC 分区查找） |

错误统一为 `{ "error": "..." }`，包含缺文件、空文件、非法日期/别名、超限等提示。

## 数据兼容性

- 当前仓库数据为 schema v2，服务器 `sustech` / `sinotruk` / `local` / `valadu` /
  `frp` / `data`；看板完全由配置与目录发现，不写死 alias 或日期。
- 可选的本地日期最多到今天，不会出现“明天”这种必然为空的日期。
- 日期目录是 UTC，但看板选择的是**本地日历日**：本地一天会读取相邻两个 UTC 分区，
  并由服务端按时间戳过滤；进程快照会在这些分区里找该本地日最后一个快照。
- 如果某个服务器/日期没有文件，总览显示“无数据”，时序与进程面板给出可读错误，
  不会崩溃或阻塞其他面板。
- 正在写入的 JSONL 最后一行可能不完整：总览/进程读取会跳过它；时序扫描会计入
  “解析失败/部分行”统计，不影响其余数据。
