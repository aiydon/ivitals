/**
 * Multi-core scanner. Buckets per-core CPU vectors into a bounded number
 * of frames (O(frames * cores) memory) and also keeps whole-range per-core
 * average/max, so a 384-core host never forces the API to load a day of
 * JSONL at once.
 */
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createInterface } from "node:readline";
import { parentPort } from "node:worker_threads";

import type { CoreFrame, DataGap } from "./shared/types";
import type { CoresRequest, CoresWorkerMessage, CoresWorkerResult } from "./shared/cores";

function num(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function round(value: number, digits = 1): number {
  const f = 10 ** digits;
  return Math.round(value * f) / f;
}

interface Bucket {
  coresSum: number[];
  coresN: number[];
  samples: number;
}

interface GlobalCores {
  sum: number[];
  n: number[];
  max: number[];
}

function ensureLength(values: number[], length: number): void {
  while (values.length < length) values.push(0);
}

async function scanFile(
  path: string,
  req: CoresRequest,
  buckets: Map<number, Bucket>,
  global: GlobalCores,
  counters: { samples: number; malformed: number; warnings: string[]; stop: boolean },
): Promise<boolean> {
  const stream = createReadStream(path, { encoding: "utf8" });
  const rl = createInterface({ input: stream, crlfDelay: Infinity });
  let lines = 0;
  try {
    for await (const raw of rl) {
      if (!raw || counters.stop) continue;
      lines += 1;
      if (lines > req.maxLines) {
        counters.warnings.push("已达到单次扫描行数上限，结果被截断");
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
      const detail = (obj.cpu_detail ?? {}) as Record<string, unknown>;
      const rawCores = detail.per_core_percent;
      if (!Array.isArray(rawCores) || rawCores.length === 0) continue;
      const cores: number[] = [];
      for (const value of rawCores) {
        const v = num(value);
        if (v === null) break;
        cores.push(Math.max(0, Math.min(100, v)));
      }
      if (cores.length === 0) continue;

      ensureLength(global.sum, cores.length);
      ensureLength(global.n, cores.length);
      ensureLength(global.max, cores.length);
      for (let i = 0; i < cores.length; i++) {
        global.sum[i] += cores[i];
        global.n[i] += 1;
        global.max[i] = Math.max(global.max[i], cores[i]);
      }

      const idx = Math.floor((ts - req.fromMs) / req.bucketMs);
      if (idx < 0) continue;
      let bucket = buckets.get(idx);
      if (!bucket) {
        bucket = { coresSum: [], coresN: [], samples: 0 };
        buckets.set(idx, bucket);
      }
      ensureLength(bucket.coresSum, cores.length);
      ensureLength(bucket.coresN, cores.length);
      for (let i = 0; i < cores.length; i++) {
        bucket.coresSum[i] += cores[i];
        bucket.coresN[i] += 1;
      }
      bucket.samples += 1;
      counters.samples += 1;
    }
  } catch (error) {
    counters.warnings.push(
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

async function run(req: CoresRequest): Promise<CoresWorkerResult> {
  const started = performance.now();
  if (!(req.bucketMs > 0) || !(req.toMs > req.fromMs)) {
    throw new Error("非法的扫描区间");
  }
  const bucketCount = Math.ceil((req.toMs - req.fromMs) / req.bucketMs);
  if (bucketCount > 100_000) throw new Error("时间桶数量过多");

  const buckets = new Map<number, Bucket>();
  const global: GlobalCores = { sum: [], n: [], max: [] };
  const counters = {
    samples: 0,
    malformed: 0,
    warnings: [] as string[],
    stop: false,
  };
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
      counters.warnings.push("本次扫描达到字节上限，结果被截断");
      break;
    }
    bytesRead += size;
    if (await scanFile(file.path, req, buckets, global, counters)) {
      truncated = true;
    }
  }

  const frames: CoreFrame[] = [];
  for (const idx of [...buckets.keys()].sort((a, b) => a - b)) {
    const bucket = buckets.get(idx)!;
    if (idx < 0 || idx >= bucketCount) continue;
    const cores = bucket.coresSum.map((sum, i) =>
      bucket.coresN[i] > 0 ? round(sum / bucket.coresN[i]) : null,
    );
    frames.push({
      t: req.fromMs + idx * req.bucketMs,
      cores,
      samples: bucket.samples,
    });
  }
  const coreCount = Math.max(global.sum.length, ...frames.map((f) => f.cores.length), 0);
  const avg = global.sum.map((sum, i) => (global.n[i] > 0 ? round(sum / global.n[i]) : 0));
  const max = global.max.map((value) => round(value));
  if (counters.samples === 0 && !truncated) {
    counters.warnings.push(
      "所选范围没有 per_core_percent 数据（旧 schema 或该主机未开启 cpu_detail）",
    );
  }
  const effectiveEnd = Math.min(req.toMs, Date.now());
  const times = frames.map((frame) => frame.t);
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
    frames,
    coreCount,
    avg,
    max,
    samples: counters.samples,
    malformed: counters.malformed,
    missingDates: [...new Set(missingDates)].sort(),
    truncated,
    scanMs: Math.round(performance.now() - started),
    bytesRead,
    warnings: counters.warnings.slice(0, 5),
    ...coverageInfo,
  };
}

parentPort?.on("message", async (req: CoresRequest) => {
  let message: CoresWorkerMessage;
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
