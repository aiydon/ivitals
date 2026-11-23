/**
 * Time-series scanner. Runs in a dedicated Worker so JSON parsing a large
 * JSONL file never blocks the API event loop. Streams files line by line and
 * aggregates into fixed time buckets: memory use is O(buckets), not O(lines).
 */
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createInterface } from "node:readline";
import { parentPort } from "node:worker_threads";

import type { DataGap, GpuInfo, GpuTrack, SeriesPoint } from "./shared/types";
import type {
  SeriesRequest,
  SeriesWorkerMessage,
  SeriesWorkerResult,
} from "./shared/series";

function num(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

interface Bucket {
  samples: number;
  cpuSum: number;
  cpuN: number;
  cpuMax: number | null;
  memSum: number;
  memN: number;
  memMax: number | null;
  ioSum: number;
  ioN: number;
  loadSum: number;
  loadN: number;
  gpuUtilSum: number;
  gpuUtilN: number;
  gpuMemSum: number;
  gpuMemN: number;
  gpuTempMax: number | null;
  gpuPowerSum: number;
  gpuPowerN: number;
  gpuCount: number;
  diskReadSum: number;
  diskReadN: number;
  diskWriteSum: number;
  diskWriteN: number;
  diskUtilMax: number | null;
  fsUsedMax: number | null;
  fsUsedBytes: number | null;
  fsTotalBytes: number | null;
  memUsedBytes: number | null;
  memTotalBytes: number | null;
  memCacheBytes: number | null;
  memSwapBytes: number | null;
  gpuTrackSum: number[];
  gpuTrackN: number[];
}

interface ScanMeta {
  latestGpus: GpuInfo[];
  gpuCount: number;
}

function parseGpuList(value: unknown, detailValue?: unknown): GpuInfo[] {
  const detailMap = new Map<number, Record<string, unknown>>();
  if (Array.isArray(detailValue)) {
    for (const item of detailValue) {
      if (item === null || typeof item !== "object") continue;
      const detail = item as Record<string, unknown>;
      const index = num(detail.index);
      if (index !== null) detailMap.set(index, detail);
    }
  }
  if (!Array.isArray(value)) return [];
  const out: GpuInfo[] = [];
  for (const item of value) {
    if (item === null || typeof item !== "object") continue;
    const g = item as Record<string, unknown>;
    const index = num(g.index);
    if (index === null) continue;
    const memUsedMb = num(g.mem_used_mb) ?? 0;
    const memTotalMb = num(g.mem_total_mb) ?? 0;
    const memUtil =
      num(g.mem_util_percent) ??
      (memTotalMb > 0 ? (memUsedMb / memTotalMb) * 100 : null);
    const detail = detailMap.get(index) ?? {};
    out.push({
      index,
      name: typeof g.name === "string" ? g.name : `GPU ${index}`,
      utilPercent: num(g.util_percent) ?? 0,
      memUsedMb,
      memTotalMb,
      memUtilPercent: memUtil,
      tempC: num(g.temp_c) ?? 0,
      powerW: num(g.power_w) ?? num(detail.power_w),
      powerLimitW: num(g.power_limit_w),
      fanPercent: num(g.fan_percent) ?? num(detail.fan_percent),
      smClockMhz: num(g.sm_clock_mhz) ?? num(detail.sm_clock_mhz),
      memClockMhz: num(g.mem_clock_mhz) ?? num(detail.mem_clock_mhz),
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

function round(value: number, digits = 2): number {
  const f = 10 ** digits;
  return Math.round(value * f) / f;
}

async function scanFile(
  path: string,
  req: SeriesRequest,
  buckets: Map<number, Bucket>,
  counters: {
    samples: number;
    malformed: number;
    scanErrors: string[];
    stop: boolean;
  },
  meta: ScanMeta,
): Promise<boolean> {
  const stream = createReadStream(path, { encoding: "utf8" });
  const rl = createInterface({ input: stream, crlfDelay: Infinity });
  let lines = 0;
  try {
    for await (const raw of rl) {
      if (!raw) continue;
      if (counters.stop) break;
      lines += 1;
      if (lines > req.maxLines) {
        counters.scanErrors.push(
          `${path.split("/").slice(-2).join("/")}: 已达到单次扫描行数上限，结果被截断`,
        );
        counters.stop = true;
        break;
      }
      let obj: Record<string, unknown>;
      try {
        obj = JSON.parse(raw) as Record<string, unknown>;
      } catch {
        counters.malformed += 1;
        continue;
      }
      const ts = num(obj.ts_ms);
      if (ts === null || ts < req.fromMs || ts >= req.toMs) continue;

      const cpuRoot = (obj.cpu ?? {}) as Record<string, unknown>;
      const memRoot = (obj.memory ?? {}) as Record<string, unknown>;
      const detail = (obj.cpu_detail ?? {}) as Record<string, unknown>;

      const cpu = num(cpuRoot.global_percent);
      const used = num(memRoot.used_bytes);
      const total = num(memRoot.total_bytes);
      const mem = used !== null && total !== null && total > 0 ? (used / total) * 100 : null;
      const iowait = num(detail.iowait_percent);
      const load1 = num(cpuRoot.load_avg_1m);
      const gpuRoot = (obj.gpu ?? {}) as Record<string, unknown>;
      const gpus = parseGpuList(gpuRoot.gpus, obj.gpu_detail);
      if (gpuRoot.state !== "ok") {
        meta.latestGpus = [];
      } else if (gpus.length > 0) {
        meta.latestGpus = gpus;
        meta.gpuCount = Math.max(meta.gpuCount, gpus.length);
      }
      let gpuUtil: number | null = null;
      let gpuMem: number | null = null;
      let gpuTemp: number | null = null;
      let gpuPower: number | null = null;
      if (gpus.length > 0) {
        let utilSum = 0;
        let memSum = 0;
        let memN = 0;
        let powerSum = 0;
        let powerN = 0;
        let tempMax = 0;
        for (const gpu of gpus) {
          utilSum += gpu.utilPercent;
          if (gpu.memUtilPercent !== null) {
            memSum += gpu.memUtilPercent;
            memN += 1;
          }
          tempMax = Math.max(tempMax, gpu.tempC);
          if (gpu.powerW !== null) {
            powerSum += gpu.powerW;
            powerN += 1;
          }
        }
        gpuUtil = utilSum / gpus.length;
        gpuMem = memN > 0 ? memSum / memN : null;
        gpuTemp = tempMax;
        gpuPower = powerN > 0 ? powerSum : null;
      }

      if (cpu === null && mem === null && iowait === null && load1 === null && gpuUtil === null) {
        counters.malformed += 1;
        continue;
      }

      const idx = Math.floor((ts - req.fromMs) / req.bucketMs);
      if (idx < 0) continue;
      let b = buckets.get(idx);
      if (!b) {
        b = {
          samples: 0,
          cpuSum: 0,
          cpuN: 0,
          cpuMax: null,
          memSum: 0,
          memN: 0,
          memMax: null,
          ioSum: 0,
          ioN: 0,
          loadSum: 0,
          loadN: 0,
          gpuUtilSum: 0,
          gpuUtilN: 0,
          gpuMemSum: 0,
          gpuMemN: 0,
          gpuTempMax: null,
          gpuPowerSum: 0,
          gpuPowerN: 0,
          gpuCount: 0,
          diskReadSum: 0,
          diskReadN: 0,
          diskWriteSum: 0,
          diskWriteN: 0,
          diskUtilMax: null,
          fsUsedMax: null,
          fsUsedBytes: null,
          fsTotalBytes: null,
          memUsedBytes: null,
          memTotalBytes: null,
          memCacheBytes: null,
          memSwapBytes: null,
          gpuTrackSum: [],
          gpuTrackN: [],
        };
        buckets.set(idx, b);
      }
      b.samples += 1;
      counters.samples += 1;
      if (cpu !== null) {
        b.cpuSum += cpu;
        b.cpuN += 1;
        b.cpuMax = b.cpuMax === null ? cpu : Math.max(b.cpuMax, cpu);
      }
      if (mem !== null) {
        b.memSum += mem;
        b.memN += 1;
        b.memMax = b.memMax === null ? mem : Math.max(b.memMax, mem);
      }
      if (used !== null) b.memUsedBytes = used;
      if (total !== null) b.memTotalBytes = total;
      const memDetail = (obj.memory_detail ?? {}) as Record<string, unknown>;
      const cache = num(memDetail.cache_bytes);
      const swap = num(memDetail.swap_used_bytes);
      if (cache !== null) b.memCacheBytes = cache;
      if (swap !== null) b.memSwapBytes = swap;
      if (iowait !== null) {
        b.ioSum += iowait;
        b.ioN += 1;
      }
      if (load1 !== null) {
        b.loadSum += load1;
        b.loadN += 1;
      }
      if (gpuUtil !== null) {
        b.gpuUtilSum += gpuUtil;
        b.gpuUtilN += 1;
        b.gpuCount = Math.max(b.gpuCount, gpus.length);
      }
      if (gpuMem !== null) {
        b.gpuMemSum += gpuMem;
        b.gpuMemN += 1;
      }
      for (const gpu of gpus) {
        if (gpu.memUtilPercent === null) continue;
        while (b.gpuTrackSum.length <= gpu.index) {
          b.gpuTrackSum.push(0);
          b.gpuTrackN.push(0);
        }
        b.gpuTrackSum[gpu.index] += gpu.memUtilPercent;
        b.gpuTrackN[gpu.index] += 1;
      }
      if (gpuTemp !== null) {
        b.gpuTempMax = b.gpuTempMax === null ? gpuTemp : Math.max(b.gpuTempMax, gpuTemp);
      }
      if (gpuPower !== null) {
        b.gpuPowerSum += gpuPower;
        b.gpuPowerN += 1;
      }

      if (Array.isArray(obj.disk_io)) {
        let read = 0;
        let write = 0;
        let util = 0;
        let count = 0;
        for (const item of obj.disk_io) {
          if (item === null || typeof item !== "object") continue;
          const disk = item as Record<string, unknown>;
          const r = num(disk.read_bytes_per_s);
          const w = num(disk.write_bytes_per_s);
          const u = num(disk.util_percent);
          if (r !== null) read += r;
          if (w !== null) write += w;
          if (u !== null) util = Math.max(util, u);
          count += 1;
        }
        if (count > 0) {
          b.diskReadSum += read;
          b.diskReadN += 1;
          b.diskWriteSum += write;
          b.diskWriteN += 1;
          b.diskUtilMax = b.diskUtilMax === null ? util : Math.max(b.diskUtilMax, util);
        }
      }
      if (Array.isArray(obj.filesystems)) {
        // Track the fullest mount of this sample; keep its absolute bytes so
        // the latest bucket can show "used / total" without a second read.
        let bestPct: number | null = null;
        let bestUsed: number | null = null;
        let bestTotal: number | null = null;
        for (const item of obj.filesystems) {
          if (item === null || typeof item !== "object") continue;
          const fs = item as Record<string, unknown>;
          const pct = num(fs.used_percent);
          if (pct === null) continue;
          if (bestPct === null || pct > bestPct) {
            bestPct = pct;
            bestUsed = num(fs.used_bytes);
            bestTotal = num(fs.total_bytes);
          }
        }
        if (bestPct !== null) {
          b.fsUsedMax = b.fsUsedMax === null ? bestPct : Math.max(b.fsUsedMax, bestPct);
          b.fsUsedBytes = bestUsed;
          b.fsTotalBytes = bestTotal;
        }
      }
    }
  } catch (error) {
    counters.scanErrors.push(
      `${path.split("/").slice(-2).join("/")}: 读取失败 (${
        error instanceof Error ? error.message : String(error)
      })`,
    );
  } finally {
    rl.close();
    stream.destroy();
  }
  return lines > req.maxLines;
}

function buildGpuTracks(
  points: SeriesPoint[],
  buckets: Map<number, Bucket>,
): GpuTrack[] {
  let maxIndex = -1;
  for (const bucket of buckets.values()) {
    maxIndex = Math.max(maxIndex, bucket.gpuTrackSum.length - 1);
  }
  if (maxIndex < 0) return [];
  const tracks: GpuTrack[] = [];
  for (let index = 0; index <= maxIndex; index++) {
    tracks.push({ index, name: `GPU ${index}`, values: [] });
  }
  const ordered = [...buckets.entries()].sort((a, b) => a[0] - b[0]);
  for (const [, bucket] of ordered) {
    for (const track of tracks) {
      const n = bucket.gpuTrackN[track.index] ?? 0;
      track.values.push(
        n > 0 ? round(bucket.gpuTrackSum[track.index] / n) : null,
      );
    }
  }
  // Trim values to the same points array length (defensive).
  for (const track of tracks) track.values.length = points.length;
  return tracks;
}

function coverage(
  times: number[],
  fromMs: number,
  toMs: number,
  bucketMs: number,
  expectedBuckets: number,
): {
  expectedBuckets: number;
  coveredBuckets: number;
  coveragePercent: number;
  gaps: DataGap[];
  largestGapSecs: number;
} {
  const gaps: DataGap[] = [];
  let largest = 0;
  const push = (start: number, end: number) => {
    const seconds = Math.max(0, Math.round((end - start) / 1000));
    if (seconds * 1000 <= bucketMs * 1.5) return;
    largest = Math.max(largest, seconds);
    gaps.push({ start, end, seconds });
  };
  if (times.length === 0) {
    push(fromMs, toMs);
  } else {
    push(fromMs, times[0]);
    for (let i = 1; i < times.length; i++) {
      push(times[i - 1] + bucketMs, times[i]);
    }
    push(times[times.length - 1] + bucketMs, toMs);
  }
  return {
    expectedBuckets,
    coveredBuckets: times.length,
    coveragePercent:
      expectedBuckets > 0 ? Math.round((times.length / expectedBuckets) * 1000) / 10 : 0,
    gaps,
    largestGapSecs: largest,
  };
}

async function run(req: SeriesRequest): Promise<SeriesWorkerResult> {
  const started = performance.now();
  if (!(req.bucketMs > 0) || !(req.toMs > req.fromMs)) {
    throw new Error("非法的扫描区间");
  }
  const bucketCount = Math.ceil((req.toMs - req.fromMs) / req.bucketMs);
  if (bucketCount > 100_000) throw new Error("时间桶数量过多，请扩大时间范围或降低点数上限");

  const buckets = new Map<number, Bucket>();
  const counters = {
    samples: 0,
    malformed: 0,
    scanErrors: [] as string[],
    stop: false,
  };
  const meta: ScanMeta = { latestGpus: [], gpuCount: 0 };
  const missingDates: string[] = [];
  let bytesRead = 0;
  let truncated = false;

  for (const file of req.files) {
    if (counters.stop) break;
    let size = 0;
    try {
      const st = await stat(file.path);
      if (!st.isFile()) {
        missingDates.push(file.date);
        continue;
      }
      size = st.size;
    } catch {
      missingDates.push(file.date);
      continue;
    }
    if (bytesRead + size > req.maxBytes) {
      truncated = true;
      counters.scanErrors.push("本次扫描达到字节上限，结果被截断；请缩小日期范围");
      break;
    }
    bytesRead += size;
    const hitLineCap = await scanFile(file.path, req, buckets, counters, meta);
    if (hitLineCap) truncated = true;
  }

  const points: SeriesPoint[] = [];
  for (const idx of [...buckets.keys()].sort((a, b) => a - b)) {
    const b = buckets.get(idx)!;
    if (idx < 0 || idx >= bucketCount) continue;
    points.push({
      t: req.fromMs + idx * req.bucketMs,
      cpu: b.cpuN > 0 ? round(b.cpuSum / b.cpuN) : null,
      cpuMax: b.cpuMax === null ? null : round(b.cpuMax),
      mem: b.memN > 0 ? round(b.memSum / b.memN) : null,
      memMax: b.memMax === null ? null : round(b.memMax),
      iowait: b.ioN > 0 ? round(b.ioSum / b.ioN, 3) : null,
      load1: b.loadN > 0 ? round(b.loadSum / b.loadN, 3) : null,
      gpuUtil: b.gpuUtilN > 0 ? round(b.gpuUtilSum / b.gpuUtilN) : null,
      gpuMem: b.gpuMemN > 0 ? round(b.gpuMemSum / b.gpuMemN) : null,
      gpuTemp: b.gpuTempMax === null ? null : round(b.gpuTempMax, 1),
      gpuPower: b.gpuPowerN > 0 ? round(b.gpuPowerSum / b.gpuPowerN, 1) : null,
      gpuCount: b.gpuCount,
      diskRead: b.diskReadN > 0 ? Math.round(b.diskReadSum / b.diskReadN) : null,
      diskWrite: b.diskWriteN > 0 ? Math.round(b.diskWriteSum / b.diskWriteN) : null,
      diskUtil: b.diskUtilMax === null ? null : round(b.diskUtilMax, 2),
      fsUsedMax: b.fsUsedMax === null ? null : round(b.fsUsedMax),
      fsUsedBytes: b.fsUsedBytes,
      fsTotalBytes: b.fsTotalBytes,
      memUsedBytes: b.memUsedBytes,
      memTotalBytes: b.memTotalBytes,
      memCacheBytes: b.memCacheBytes,
      memSwapBytes: b.memSwapBytes,
      samples: b.samples,
    });
  }

  // Reject buckets that somehow fall outside [from, to) (defensive).
  const filtered = points.filter((p) => p.t >= req.fromMs && p.t < req.toMs);
  const effectiveEnd = Math.min(req.toMs, Date.now());
  const times = filtered.map((point) => point.t);
  // Coverage starts at the first observed sample: hours before monitoring
  // began are "not monitored", not lost data.
  const effectiveStart = times.length > 0 ? times[0] : req.fromMs;
  const expected = Math.max(
    0,
    Math.ceil((effectiveEnd - effectiveStart) / req.bucketMs),
  );
  const coverageInfo = coverage(
    times,
    effectiveStart,
    effectiveEnd,
    req.bucketMs,
    expected,
  );
  return {
    points: filtered,
    samples: counters.samples,
    malformed: counters.malformed,
    missingDates: [...new Set(missingDates)].sort(),
    truncated,
    scanMs: Math.round(performance.now() - started),
    bytesRead,
    warnings: counters.scanErrors.slice(0, 5),
    gpuCount: meta.gpuCount,
    latestGpus: meta.latestGpus,
    gpuTracks: buildGpuTracks(filtered, buckets),
    ...coverageInfo,
  };
}

parentPort?.on("message", async (req: SeriesRequest) => {
  let message: SeriesWorkerMessage;
  try {
    message = { ok: true, result: await run(req) };
  } catch (error) {
    message = {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
  parentPort?.postMessage(message);
});
