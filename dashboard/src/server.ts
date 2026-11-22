/**
 * ivitals dashboard API + static host.
 *
 * Read-only by construction: this process opens data/ for reading only and
 * never writes anywhere except the dashboard's own build output (done by
 * `bun build`, not at runtime).
 */
import { networkInterfaces } from "node:os";
import { join, resolve, sep } from "node:path";
import { Worker } from "node:worker_threads";

import index from "./index.html";
import {
  ALIAS_RE,
  DATA_DIR,
  PROJECT_ROOT,
  CONFIG_PATH,
  dataFile,
  dayStartMs,
  daysBetween,
  discoverAliases,
  fileSize,
  isValidDate,
  listDates,
  loadConfig,
  tailJsonRecord,
} from "./lib/ivitals";
import type { SeriesRequest, SeriesWorkerMessage, SeriesWorkerResult } from "./shared/series";
import type { CoresRequest, CoresWorkerMessage, CoresWorkerResult } from "./shared/cores";
import type {
  CoresResponse,
  FsInfo,
  GpuInfo,
  GpuProcess,
  GpuState,
  HostMeta,
  MetaResponse,
  OverviewResponse,
  OverviewServer,
  ProcessesResponse,
  ProcRow,
  SeriesResponse,
  ServerInfo,
} from "./shared/types";

const PORT = Number(process.env.PORT ?? 8899);
const HOST = process.env.HOST ?? "0.0.0.0";
const PROD = process.env.NODE_ENV === "production";
const DIST = resolve(import.meta.dir, "../dist");
const DAY_MS = 86_400_000;
const MAX_RANGE_DAYS = 31;
const MAX_SCAN_BYTES = 1 << 30; // 1 GiB per series request
const MAX_SCAN_LINES = 3_000_000;
const SCAN_TIMEOUT_MS = 60_000;

class HttpError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
    },
  });
}

function text(message: string, status: number): Response {
  return new Response(message, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}

function asRecord(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object"
    ? (value as Record<string, unknown>)
    : {};
}

function finite(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function intParam(
  raw: string | null,
  fallback: number,
  min: number,
  max: number,
  label: string,
): number {
  if (raw === null || raw === "") return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value) || !Number.isInteger(value)) {
    throw new HttpError(400, `${label} 必须是整数`);
  }
  if (value < min || value > max) {
    throw new HttpError(400, `${label} 必须在 ${min}..${max} 之间`);
  }
  return value;
}

function dedupe(items: string[]): string[] {
  return [...new Set(items.filter(Boolean))];
}

async function guard(fn: () => Promise<unknown>): Promise<Response> {
  try {
    return json(await fn());
  } catch (error) {
    if (error instanceof HttpError) return json({ error: error.message }, error.status);
    console.error("[ivitals-dashboard]", error);
    return json(
      {
        error: `看板内部错误: ${error instanceof Error ? error.message : String(error)}`,
      },
      500,
    );
  }
}

async function buildMeta(): Promise<MetaResponse> {
  const cfg = await loadConfig();
  const dates = await listDates();
  const discovered = await discoverAliases(dates);
  const warnings = [...cfg.warnings];
  if (cfg.error) warnings.unshift(cfg.error);
  if (dates.length === 0) {
    warnings.push(`数据目录中没有 YYYY-MM-DD 子目录: ${DATA_DIR}`);
  }

  const byAlias = new Map(cfg.servers.map((s) => [s.alias, s]));
  const servers: ServerInfo[] = [...byAlias.values()].map((s) => ({
    alias: s.alias,
    mode: s.mode,
    host: s.host,
    configured: true,
  }));
  for (const alias of discovered) {
    if (byAlias.has(alias)) continue;
    servers.push({ alias, mode: "unknown", configured: false });
    warnings.push(`数据中发现配置外的服务器别名: ${alias}`);
  }

  return {
    generatedAt: Date.now(),
    dataDir: DATA_DIR,
    configPath: CONFIG_PATH,
    intervalSecs: cfg.intervalSecs,
    processIntervalSecs: cfg.processIntervalSecs,
    retentionDays: cfg.retentionDays,
    dates,
    servers,
    warnings: dedupe(warnings),
  };
}

interface LatestMetrics {
  date: string;
  tsMs: number;
  cpuPercent: number | null;
  coreCount: number | null;
  load1: number | null;
  memUsedPercent: number | null;
  memUsedBytes: number | null;
  memTotalBytes: number | null;
  iowaitPercent: number | null;
  perCoreMeanPercent: number | null;
  perCoreP95Percent: number | null;
  perCoreMaxPercent: number | null;
  coresOver50: number | null;
  coresOver90: number | null;
  gpu: OverviewServer["gpu"];
  gpus: GpuInfo[];
  filesystems: FsInfo[];
  hostMeta: HostMeta | null;
  mtimeMs: number;
}

function parseGpuList(value: unknown, detailValue?: unknown): GpuInfo[] {
  const detailMap = new Map<number, Record<string, unknown>>();
  if (Array.isArray(detailValue)) {
    for (const item of detailValue) {
      const detail = asRecord(item);
      const index = finite(detail.index);
      if (index !== null) detailMap.set(index, detail);
    }
  }
  if (!Array.isArray(value)) return [];
  const out: GpuInfo[] = [];
  for (const item of value) {
    const g = asRecord(item);
    const index = finite(g.index);
    if (index === null) continue;
    const memUsedMb = finite(g.mem_used_mb) ?? 0;
    const memTotalMb = finite(g.mem_total_mb) ?? 0;
    const detail = detailMap.get(index) ?? {};
    out.push({
      index,
      name: typeof g.name === "string" ? g.name : `GPU ${index}`,
      utilPercent: finite(g.util_percent) ?? 0,
      memUsedMb,
      memTotalMb,
      memUtilPercent:
        finite(g.mem_util_percent) ??
        (memTotalMb > 0 ? (memUsedMb / memTotalMb) * 100 : null),
      tempC: finite(g.temp_c) ?? 0,
      powerW: finite(g.power_w) ?? finite(detail.power_w),
      powerLimitW: finite(g.power_limit_w),
      fanPercent: finite(g.fan_percent) ?? finite(detail.fan_percent),
      smClockMhz: finite(g.sm_clock_mhz) ?? finite(detail.sm_clock_mhz),
      memClockMhz: finite(g.mem_clock_mhz) ?? finite(detail.mem_clock_mhz),
      pstate:
        typeof g.pstate === "string"
          ? g.pstate
          : typeof detail.pstate === "string"
            ? detail.pstate
            : null,
    });
  }
  return out;
}

function parseFilesystems(value: unknown): FsInfo[] {
  if (!Array.isArray(value)) return [];
  const out: FsInfo[] = [];
  for (const item of value) {
    const f = asRecord(item);
    const total = finite(f.total_bytes);
    const used = finite(f.used_bytes);
    const available = finite(f.available_bytes);
    if (total === null || used === null || available === null) continue;
    out.push({
      device: typeof f.device === "string" ? f.device : "",
      mount: typeof f.mount === "string" ? f.mount : "",
      fstype: typeof f.fstype === "string" ? f.fstype : "",
      totalBytes: total,
      usedBytes: used,
      availableBytes: available,
      usedPercent: finite(f.used_percent) ?? (total > 0 ? (used / total) * 100 : 0),
    });
  }
  return out;
}

function parseHostMeta(value: unknown): HostMeta | null {
  const host = asRecord(value);
  const coreCount = finite(host.core_count);
  const memTotalBytes = finite(host.mem_total_bytes);
  const gpus = Array.isArray(host.gpu_models)
    ? host.gpu_models.filter((item): item is string => typeof item === "string")
    : [];
  const hostname = typeof host.hostname === "string" ? host.hostname : null;
  const os = typeof host.os === "string" ? host.os : null;
  const cpuModel = typeof host.cpu_model === "string" ? host.cpu_model : null;
  if (!hostname && !os && !cpuModel && coreCount === null) return null;
  return {
    hostname,
    os,
    kernel: typeof host.kernel === "string" ? host.kernel : null,
    arch: typeof host.arch === "string" ? host.arch : null,
    cpuModel,
    coreCount,
    memTotalBytes,
    bootTimeMs: finite(host.boot_time_ms),
    uptimeSecs: finite(host.uptime_secs),
    gpuModels: gpus,
  };
}

async function readLatestHost(alias: string, dates: string[]): Promise<HostMeta | null> {
  for (const date of dates) {
    const tail = await tailJsonRecord(dataFile(date, alias, "host"));
    if (!tail) continue;
    const parsed = parseHostMeta(tail.value);
    if (parsed) return parsed;
  }
  return null;
}

function parseLatestMetrics(value: unknown, date: string, mtimeMs: number): LatestMetrics | null {
  const sample = asRecord(value);
  const tsMs = finite(sample.ts_ms);
  if (tsMs === null) return null;
  const cpu = asRecord(sample.cpu);
  const memory = asRecord(sample.memory);
  const detail = asRecord(sample.cpu_detail);
  const total = finite(memory.total_bytes);
  const used = finite(memory.used_bytes);
  const gpuRaw = asRecord(sample.gpu);
  const rawState = gpuRaw.state;
  const state: GpuState = rawState === "ok" || rawState === "error" ? rawState : "absent";
  const gpus = parseGpuList(gpuRaw.gpus, sample.gpu_detail);
  const message = typeof gpuRaw.message === "string" ? gpuRaw.message : undefined;

  return {
    date,
    tsMs,
    cpuPercent: finite(cpu.global_percent),
    coreCount: finite(cpu.core_count),
    load1: finite(cpu.load_avg_1m),
    memUsedPercent:
      used !== null && total !== null && total > 0
        ? Math.min(100, Math.max(0, (used / total) * 100))
        : null,
    memUsedBytes: used,
    memTotalBytes: total,
    iowaitPercent: finite(detail.iowait_percent),
    perCoreMeanPercent: finite(detail.per_core_mean_percent),
    perCoreP95Percent: finite(detail.per_core_p95_percent),
    perCoreMaxPercent: finite(detail.per_core_max_percent),
    coresOver50: finite(detail.cores_over_50),
    coresOver90: finite(detail.cores_over_90),
    gpu: { state, count: gpus.length, message },
    gpus,
    filesystems: parseFilesystems(sample.filesystems),
    hostMeta: null,
    mtimeMs,
  };
}

async function readLatestMetrics(alias: string, dates: string[]): Promise<LatestMetrics | null> {
  for (const date of dates) {
    const path = dataFile(date, alias, "metrics");
    const tail = await tailJsonRecord(path);
    if (!tail) continue;
    const parsed = parseLatestMetrics(tail.value, date, tail.mtimeMs);
    if (parsed) return parsed;
  }
  return null;
}

async function buildOverview(): Promise<OverviewResponse> {
  const cfg = await loadConfig();
  const dates = await listDates();
  const discovered = await discoverAliases(dates);
  const warnings = [...cfg.warnings];
  if (cfg.error) warnings.unshift(cfg.error);
  if (dates.length === 0) warnings.push(`数据目录为空或不可读: ${DATA_DIR}`);

  interface Target {
    alias: string;
    mode: string;
    host?: string;
    configured: boolean;
  }
  const configured: Target[] = cfg.servers.map((s) => ({
    alias: s.alias,
    mode: s.mode,
    host: s.host,
    configured: true,
  }));
  const known = new Set(configured.map((s) => s.alias));
  const targets: Target[] = configured.length
    ? configured
    : discovered.map((alias) => ({ alias, mode: "unknown", configured: false }));
  for (const alias of discovered) {
    if (!known.has(alias)) {
      targets.push({ alias, mode: "unknown", host: undefined, configured: false });
    }
  }

  const staleAfterMs = Math.max(cfg.intervalSecs * 3000, 120_000);
  const now = Date.now();
  const servers: OverviewServer[] = [];

  for (const target of targets) {
    const host = await readLatestHost(target.alias, dates);
    const latest = await readLatestMetrics(target.alias, dates);
    if (!latest) {
      servers.push({
        alias: target.alias,
        mode: target.mode,
        host: target.host,
        status: "no-data",
        lastSeenMs: null,
        lastFileMtimeMs: null,
        date: null,
        cpuPercent: null,
        coreCount: null,
        load1: null,
        memUsedPercent: null,
        memUsedBytes: null,
        memTotalBytes: null,
        iowaitPercent: null,
        perCoreMeanPercent: null,
        perCoreP95Percent: null,
        perCoreMaxPercent: null,
        coresOver50: null,
        coresOver90: null,
        gpu: null,
        gpus: [],
        filesystems: [],
        hostMeta: host,
        error: "没有找到可解析的 metrics 记录",
      });
      continue;
    }
    const age = Math.max(0, now - latest.tsMs);
    servers.push({
      alias: target.alias,
      mode: target.mode,
      host: target.host,
      status: age <= staleAfterMs ? "online" : "stale",
      lastSeenMs: latest.tsMs,
      lastFileMtimeMs: latest.mtimeMs,
      date: latest.date,
      cpuPercent: latest.cpuPercent,
      coreCount: latest.coreCount,
      load1: latest.load1,
      memUsedPercent: latest.memUsedPercent,
      memUsedBytes: latest.memUsedBytes,
      memTotalBytes: latest.memTotalBytes,
      iowaitPercent: latest.iowaitPercent,
      perCoreMeanPercent: latest.perCoreMeanPercent,
      perCoreP95Percent: latest.perCoreP95Percent,
      perCoreMaxPercent: latest.perCoreMaxPercent,
      coresOver50: latest.coresOver50,
      coresOver90: latest.coresOver90,
      gpu: latest.gpu,
      gpus: latest.gpus,
      filesystems: latest.filesystems,
      hostMeta: host,
    });
  }

  return { generatedAt: now, staleAfterMs, servers, warnings: dedupe(warnings) };
}

function runSeriesWorker(req: SeriesRequest): Promise<SeriesWorkerResult> {
  const { promise, resolve, reject } = Promise.withResolvers<SeriesWorkerResult>();
  const worker = new Worker(join(import.meta.dir, "series-worker.ts"));
  let settled = false;
  const finish = (fn: () => void) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    void worker.terminate();
    fn();
  };
  const timer = setTimeout(
    () =>
      finish(() =>
        reject(
          new HttpError(
            504,
            `扫描超时（>${SCAN_TIMEOUT_MS / 1000}s）；请缩小日期范围或服务器数量`,
          ),
        ),
      ),
    SCAN_TIMEOUT_MS,
  );
  worker.once("message", (message: SeriesWorkerMessage) =>
    finish(() =>
      message.ok
        ? resolve(message.result)
        : reject(new HttpError(500, `扫描失败: ${message.error}`)),
    ),
  );
  worker.once("error", (error) =>
    finish(() =>
      reject(
        new HttpError(
          500,
          `扫描 worker 失败: ${error instanceof Error ? error.message : String(error)}`,
        ),
      ),
    ),
  );
  worker.once("exit", (code) => {
    if (!settled) {
      finish(() => reject(new HttpError(500, `扫描 worker 提前退出 (code ${code})`)));
    }
  });
  worker.postMessage(req);
  return promise;
}

function runCoresWorker(req: CoresRequest): Promise<CoresWorkerResult> {
  const { promise, resolve, reject } = Promise.withResolvers<CoresWorkerResult>();
  const worker = new Worker(join(import.meta.dir, "cores-worker.ts"));
  let settled = false;
  const finish = (fn: () => void) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    void worker.terminate();
    fn();
  };
  const timer = setTimeout(
    () =>
      finish(() =>
        reject(
          new HttpError(
            504,
            `多核扫描超时（>${SCAN_TIMEOUT_MS / 1000}s）；请缩小日期范围`,
          ),
        ),
      ),
    SCAN_TIMEOUT_MS,
  );
  worker.once("message", (message: CoresWorkerMessage) =>
    finish(() =>
      message.ok
        ? resolve(message.result)
        : reject(new HttpError(500, `多核扫描失败: ${message.error}`)),
    ),
  );
  worker.once("error", (error) =>
    finish(() =>
      reject(
        new HttpError(
          500,
          `多核 worker 失败: ${error instanceof Error ? error.message : String(error)}`,
        ),
      ),
    ),
  );
  worker.once("exit", (code) => {
    if (!settled) {
      finish(() => reject(new HttpError(500, `多核 worker 提前退出 (code ${code})`)));
    }
  });
  worker.postMessage(req);
  return promise;
}

function calendarDates(from: string, to: string): string[] {
  const out: string[] = [];
  const end = dayStartMs(to);
  for (let t = dayStartMs(from); t <= end; t += DAY_MS) {
    out.push(new Date(t).toISOString().slice(0, 10));
  }
  return out;
}

async function buildSeries(url: URL): Promise<SeriesResponse> {
  const server = url.searchParams.get("server") ?? "";
  if (!ALIAS_RE.test(server)) throw new HttpError(400, "server 参数缺失或非法");
  const dates = await listDates();
  if (dates.length === 0) throw new HttpError(404, `数据目录为空或不可读: ${DATA_DIR}`);

  const to = url.searchParams.get("to") ?? dates[0];
  const from = url.searchParams.get("from") ?? to;
  if (!isValidDate(from) || !isValidDate(to)) {
    throw new HttpError(400, "from/to 必须是 YYYY-MM-DD 日期");
  }
  const span = daysBetween(from, to);
  if (span < 0) throw new HttpError(400, "from 不能晚于 to");
  if (span >= MAX_RANGE_DAYS) {
    throw new HttpError(400, `单次最多查询 ${MAX_RANGE_DAYS} 个日历日`);
  }
  const maxPoints = intParam(url.searchParams.get("maxPoints"), 1200, 100, 4000, "maxPoints");

  const allDates = calendarDates(from, to);
  const files = allDates.map((date) => ({
    date,
    path: dataFile(date, server, "metrics"),
  }));
  const sizes = await Promise.all(files.map((f) => fileSize(f.path)));
  const existing = files.filter((_, index) => sizes[index] !== null);
  if (existing.length === 0) {
    const discovered = await discoverAliases(dates);
    throw new HttpError(
      404,
      `服务器 ${server} 在 ${from}..${to} 没有 metrics 文件；请检查别名或日期。存在数据的别名: ${
        discovered.join(", ") || "无"
      }`,
    );
  }

  const fromMs = dayStartMs(from);
  const toMs = dayStartMs(to) + DAY_MS;
  const spanMs = toMs - fromMs;
  const bucketSecs = Math.max(5, Math.ceil(spanMs / maxPoints / 1000 / 5) * 5);
  const started = performance.now();
  const result = await runSeriesWorker({
    files,
    fromMs,
    toMs,
    bucketMs: bucketSecs * 1000,
    maxLines: MAX_SCAN_LINES,
    maxBytes: MAX_SCAN_BYTES,
  });

  return {
    server,
    from,
    to,
    bucketSecs,
    points: result.points,
    samples: result.samples,
    malformed: result.malformed,
    missingDates: result.missingDates,
    truncated: result.truncated,
    scanMs: Math.round(performance.now() - started),
    warnings: result.warnings,
    gpuCount: result.gpuCount,
    latestGpus: result.latestGpus,
    expectedBuckets: result.expectedBuckets,
    coveredBuckets: result.coveredBuckets,
    coveragePercent: result.coveragePercent,
    gaps: result.gaps,
    largestGapSecs: result.largestGapSecs,
  };
}

async function buildProcesses(url: URL): Promise<ProcessesResponse> {
  const server = url.searchParams.get("server") ?? "";
  if (!ALIAS_RE.test(server)) throw new HttpError(400, "server 参数缺失或非法");
  const dates = await listDates();
  const date = url.searchParams.get("date") ?? dates[0] ?? "";
  if (!isValidDate(date)) throw new HttpError(400, "date 必须是 YYYY-MM-DD 日期");
  const topN = intParam(url.searchParams.get("top"), 20, 1, 100, "top");

  const path = dataFile(date, server, "processes");
  if ((await fileSize(path)) === null) {
    const discovered = await discoverAliases(dates);
    throw new HttpError(
      404,
      `服务器 ${server} 在 ${date} 没有进程快照文件。可用日期: ${
        dates.slice(0, 8).join(", ") || "无"
      }；存在数据的别名: ${discovered.join(", ") || "无"}`,
    );
  }
  const tail = await tailJsonRecord(path, 2 * 1024 * 1024);
  if (!tail) {
    throw new HttpError(
      404,
      `${server} ${date} 的进程文件为空，或最后一行尚未写完（采集端 30s 内会 flush）`,
    );
  }
  const snapshot = asRecord(tail.value);
  const tsMs = finite(snapshot.ts_ms);
  if (tsMs === null || !Array.isArray(snapshot.processes)) {
    throw new HttpError(422, `${server} ${date} 的进程快照格式不正确`);
  }

  const processes: ProcRow[] = [];
  for (const item of snapshot.processes) {
    const row = asRecord(item);
    const pid = finite(row.pid);
    const name = typeof row.name === "string" ? row.name : "";
    const cpu = finite(row.cpu_percent);
    const mem = finite(row.mem_percent);
    if (pid === null || !name || cpu === null || mem === null) continue;
    const state = typeof row.state === "string" ? row.state.slice(0, 1) : null;
    processes.push({
      pid,
      name,
      user: typeof row.user === "string" && row.user ? row.user : null,
      uid: finite(row.uid),
      gid: finite(row.gid),
      ppid: finite(row.ppid),
      parentName:
        typeof row.parent_name === "string" && row.parent_name ? row.parent_name : null,
      cmdline: typeof row.cmdline === "string" && row.cmdline ? row.cmdline : null,
      cwd: typeof row.cwd === "string" && row.cwd ? row.cwd : null,
      exe: typeof row.exe === "string" && row.exe ? row.exe : null,
      cpuPercent: cpu,
      memPercent: mem,
      rssBytes: finite(row.rss_bytes),
      threads: finite(row.threads),
      cumCpuMs: finite(row.cum_cpu_ms),
      state,
    });
  }
  processes.sort((a, b) => b.cpuPercent - a.cpuPercent || b.memPercent - a.memPercent);

  const stateCountsRaw = asRecord(snapshot.state_counts);
  const stateCounts: Record<string, number> = {};
  for (const [key, value] of Object.entries(stateCountsRaw)) {
    const count = finite(value);
    if (count !== null) stateCounts[key] = count;
  }
  const gpuProcesses: GpuProcess[] = [];
  if (Array.isArray(snapshot.gpu_processes)) {
    for (const item of snapshot.gpu_processes) {
      const row = asRecord(item);
      const pid = finite(row.pid);
      if (pid === null) continue;
      gpuProcesses.push({
        gpuIndex: finite(row.gpu_index) ?? 0,
        gpuUuid: typeof row.gpu_uuid === "string" ? row.gpu_uuid : null,
        pid,
        name: typeof row.name === "string" ? row.name : "",
        memMb: finite(row.mem_mb),
        kind: typeof row.kind === "string" ? row.kind : null,
      });
    }
    gpuProcesses.sort((a, b) => (b.memMb ?? 0) - (a.memMb ?? 0));
  }

  return {
    server,
    date,
    tsMs,
    topN,
    totalProcs: finite(snapshot.total_procs),
    totalThreads: finite(snapshot.total_threads),
    stateCounts: Object.keys(stateCounts).length > 0 ? stateCounts : null,
    gpuProcesses,
    processes: processes.slice(0, topN),
  };
}

async function buildCores(url: URL): Promise<CoresResponse> {
  const server = url.searchParams.get("server") ?? "";
  if (!ALIAS_RE.test(server)) throw new HttpError(400, "server 参数缺失或非法");
  const dates = await listDates();
  if (dates.length === 0) throw new HttpError(404, `数据目录为空或不可读: ${DATA_DIR}`);

  const to = url.searchParams.get("to") ?? dates[0];
  const from = url.searchParams.get("from") ?? to;
  if (!isValidDate(from) || !isValidDate(to)) {
    throw new HttpError(400, "from/to 必须是 YYYY-MM-DD 日期");
  }
  const span = daysBetween(from, to);
  if (span < 0) throw new HttpError(400, "from 不能晚于 to");
  if (span >= MAX_RANGE_DAYS) {
    throw new HttpError(400, `单次最多查询 ${MAX_RANGE_DAYS} 个日历日`);
  }
  const maxFrames = intParam(url.searchParams.get("maxFrames"), 240, 40, 800, "maxFrames");
  const files = calendarDates(from, to).map((date) => ({
    date,
    path: dataFile(date, server, "metrics"),
  }));
  const sizes = await Promise.all(files.map((f) => fileSize(f.path)));
  if (sizes.every((size) => size === null)) {
    throw new HttpError(
      404,
      `服务器 ${server} 在 ${from}..${to} 没有 metrics 文件`,
    );
  }

  const fromMs = dayStartMs(from);
  const toMs = dayStartMs(to) + DAY_MS;
  const spanMs = toMs - fromMs;
  const bucketSecs = Math.max(5, Math.ceil(spanMs / maxFrames / 1000 / 5) * 5);
  const started = performance.now();
  const result = await runCoresWorker({
    files,
    fromMs,
    toMs,
    bucketMs: bucketSecs * 1000,
    maxLines: MAX_SCAN_LINES,
    maxBytes: MAX_SCAN_BYTES,
  });
  return {
    server,
    from,
    to,
    bucketSecs,
    coreCount: result.coreCount,
    frames: result.frames,
    avg: result.avg,
    max: result.max,
    samples: result.samples,
    malformed: result.malformed,
    missingDates: result.missingDates,
    truncated: result.truncated,
    scanMs: Math.round(performance.now() - started),
    warnings: result.warnings,
    expectedBuckets: result.expectedBuckets,
    coveredBuckets: result.coveredBuckets,
    coveragePercent: result.coveragePercent,
    gaps: result.gaps,
    largestGapSecs: result.largestGapSecs,
  };
}

async function serveDist(pathname: string): Promise<Response> {
  let decoded: string;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return text("Bad request", 400);
  }
  const rel = decoded === "/" ? "/index.html" : decoded;
  const target = resolve(DIST, `.${rel}`);
  if (target !== DIST && !target.startsWith(DIST + sep)) return text("Not found", 404);
  const file = Bun.file(target);
  if (!(await file.exists())) return text("Not found", 404);
  return new Response(file, {
    headers: {
      "cache-control":
        rel === "/index.html" ? "no-cache" : "public, max-age=31536000, immutable",
      "x-content-type-options": "nosniff",
    },
  });
}

const apiRoutes = {
  "/api/meta": () => guard(buildMeta),
  "/api/overview": () => guard(buildOverview),
  "/api/series": (req: Request) => guard(() => buildSeries(new URL(req.url))),
  "/api/cores": (req: Request) => guard(() => buildCores(new URL(req.url))),
  "/api/processes": (req: Request) => guard(() => buildProcesses(new URL(req.url))),
};

const server = Bun.serve({
  port: PORT,
  hostname: HOST,
  routes: PROD
    ? { ...apiRoutes, "/": () => serveDist("/") }
    : { ...apiRoutes, "/": index },
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.startsWith("/api/")) return json({ error: "API 路径不存在" }, 404);
    if (PROD) return serveDist(url.pathname);
    return text("Not found", 404);
  },
});

console.log(
  `ivitals dashboard (${PROD ? "production" : "dev"}) listening on ${server.hostname}:${server.port}`,
);
if (server.hostname === "0.0.0.0" || server.hostname === "::") {
  console.log(`  local:   http://127.0.0.1:${server.port}`);
  for (const [name, addrs] of Object.entries(networkInterfaces())) {
    for (const addr of addrs ?? []) {
      if (addr.family === "IPv4" && !addr.internal) {
        console.log(`  ${name}: http://${addr.address}:${server.port}`);
      }
    }
  }
  console.log("  note:    no authentication; expose only on a trusted network");
}
console.log(`  project: ${PROJECT_ROOT}`);
console.log(`  config:  ${CONFIG_PATH}`);
console.log(`  data:    ${DATA_DIR} (read-only)`);
