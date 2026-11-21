import type { CoreFrame, DataGap } from "./types";
import type { SeriesFile } from "./series";

export interface CoresRequest {
  files: SeriesFile[];
  fromMs: number;
  toMs: number;
  bucketMs: number;
  maxLines: number;
  maxBytes: number;
}

export interface CoresWorkerResult {
  frames: CoreFrame[];
  coreCount: number;
  avg: number[];
  max: number[];
  samples: number;
  malformed: number;
  missingDates: string[];
  truncated: boolean;
  scanMs: number;
  bytesRead: number;
  warnings: string[];
  expectedBuckets: number;
  coveredBuckets: number;
  coveragePercent: number;
  gaps: DataGap[];
  largestGapSecs: number;
}

export type CoresWorkerMessage =
  | { ok: true; result: CoresWorkerResult }
  | { ok: false; error: string };
