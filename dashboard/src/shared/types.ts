/** Wire types shared by the Bun API and the React client. */

export type GpuState = "absent" | "ok" | "error";

export interface ServerInfo {
  alias: string;
  mode: string;
  host?: string;
  /** true when the alias comes from config.toml, false when only data exists. */
  configured: boolean;
}

export interface MetaResponse {
  generatedAt: number;
  dataDir: string;
  configPath: string;
  intervalSecs: number;
  processIntervalSecs: number;
  retentionDays: number;
  dates: string[];
  servers: ServerInfo[];
  warnings: string[];
}

export interface GpuInfo {
  index: number;
  name: string;
  utilPercent: number;
  memUsedMb: number;
  memTotalMb: number;
  memUtilPercent: number | null;
  tempC: number;
  powerW: number | null;
  powerLimitW: number | null;
  fanPercent: number | null;
  smClockMhz: number | null;
  memClockMhz: number | null;
  pstate: string | null;
}

export interface FsInfo {
  device: string;
  mount: string;
  fstype: string;
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  usedPercent: number;
}

export interface HostMeta {
  hostname: string | null;
  os: string | null;
  kernel: string | null;
  arch: string | null;
  cpuModel: string | null;
  coreCount: number | null;
  memTotalBytes: number | null;
  bootTimeMs: number | null;
  uptimeSecs: number | null;
  gpuModels: string[];
}

export interface OverviewServer {
  alias: string;
  mode: string;
  host?: string;
  status: "online" | "stale" | "no-data";
  lastSeenMs: number | null;
  lastFileMtimeMs: number | null;
  date: string | null;
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
  gpu: {
    state: GpuState;
    count: number;
    message?: string;
  } | null;
  gpus: GpuInfo[];
  filesystems: FsInfo[];
  diskReadBytesPerSec: number | null;
  diskWriteBytesPerSec: number | null;
  diskUtilPercent: number | null;
  hostMeta: HostMeta | null;
  error?: string;
}

export interface OverviewResponse {
  generatedAt: number;
  staleAfterMs: number;
  servers: OverviewServer[];
  warnings: string[];
}

export interface DataGap {
  start: number;
  end: number;
  seconds: number;
}

export interface SeriesPoint {
  /** Bucket start, Unix ms (UTC). */
  t: number;
  cpu: number | null;
  cpuMax: number | null;
  mem: number | null;
  memMax: number | null;
  iowait: number | null;
  load1: number | null;
  gpuUtil: number | null;
  gpuMem: number | null;
  gpuTemp: number | null;
  gpuPower: number | null;
  gpuCount: number;
  diskRead: number | null;
  diskWrite: number | null;
  diskUtil: number | null;
  fsUsedMax: number | null;
  fsUsedBytes: number | null;
  fsTotalBytes: number | null;
  memUsedBytes: number | null;
  memTotalBytes: number | null;
  samples: number;
}

export interface SeriesResponse {
  server: string;
  from: string;
  to: string;
  bucketSecs: number;
  points: SeriesPoint[];
  samples: number;
  malformed: number;
  missingDates: string[];
  truncated: boolean;
  scanMs: number;
  warnings: string[];
  gpuCount: number;
  latestGpus: GpuInfo[];
  expectedBuckets: number;
  coveredBuckets: number;
  coveragePercent: number;
  gaps: DataGap[];
  largestGapSecs: number;
}

export interface CoreFrame {
  t: number;
  /** Average busy percent per core for this time bucket; null = no sample. */
  cores: (number | null)[];
  samples: number;
}

export interface CoresResponse {
  server: string;
  from: string;
  to: string;
  bucketSecs: number;
  coreCount: number;
  frames: CoreFrame[];
  /** Per-core average/max over the whole range, indexed by core number. */
  avg: number[];
  max: number[];
  samples: number;
  malformed: number;
  missingDates: string[];
  truncated: boolean;
  scanMs: number;
  warnings: string[];
  expectedBuckets: number;
  coveredBuckets: number;
  coveragePercent: number;
  gaps: DataGap[];
  largestGapSecs: number;
}

export interface ProcRow {
  pid: number;
  name: string;
  user: string | null;
  uid: number | null;
  gid: number | null;
  ppid: number | null;
  parentName: string | null;
  cmdline: string | null;
  cwd: string | null;
  exe: string | null;
  cpuPercent: number;
  memPercent: number;
  rssBytes: number | null;
  threads: number | null;
  cumCpuMs: number | null;
  state: string | null;
}

export interface GpuProcess {
  gpuIndex: number;
  gpuUuid: string | null;
  pid: number;
  name: string;
  memMb: number | null;
  kind: string | null;
}

export interface ProcessesResponse {
  server: string;
  date: string;
  tsMs: number;
  topN: number;
  totalProcs: number | null;
  totalThreads: number | null;
  stateCounts: Record<string, number> | null;
  gpuProcesses: GpuProcess[];
  processes: ProcRow[];
}

export interface ApiError {
  error: string;
}
