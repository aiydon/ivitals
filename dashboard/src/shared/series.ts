import type { DataGap, GpuInfo, SeriesPoint } from "./types";

export interface SeriesFile {
  date: string;
  path: string;
}

export interface SeriesRequest {
  files: SeriesFile[];
  fromMs: number;
  toMs: number;
  bucketMs: number;
  maxLines: number;
  maxBytes: number;
}

export interface SeriesWorkerResult {
  points: SeriesPoint[];
  samples: number;
  malformed: number;
  missingDates: string[];
  truncated: boolean;
  scanMs: number;
  bytesRead: number;
  warnings: string[];
  gpuCount: number;
  latestGpus: GpuInfo[];
  expectedBuckets: number;
  coveredBuckets: number;
  coveragePercent: number;
  gaps: DataGap[];
  largestGapSecs: number;
}

export type SeriesWorkerMessage =
  | { ok: true; result: SeriesWorkerResult }
  | { ok: false; error: string };
