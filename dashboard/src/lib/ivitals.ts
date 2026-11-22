/**
 * Read-only access to the ivitals runtime: config.toml plus data/.
 * Every filesystem operation here is a read; the dashboard never writes
 * under the project root.
 */
import { open, readdir, stat } from "node:fs/promises";
import { isAbsolute, join, resolve } from "node:path";

export const DASHBOARD_ROOT = resolve(import.meta.dir, "../..");
export const PROJECT_ROOT = resolve(DASHBOARD_ROOT, "..");

function envPath(name: string, fallback: string): string {
  const value = process.env[name];
  if (!value) return fallback;
  return isAbsolute(value) ? value : resolve(PROJECT_ROOT, value);
}

export const CONFIG_PATH = envPath(
  "IVITALS_CONFIG",
  join(PROJECT_ROOT, "config.toml"),
);
export const DATA_DIR = envPath("IVITALS_DATA_DIR", join(PROJECT_ROOT, "data"));

export const ALIAS_RE = /^[A-Za-z0-9_-]{1,64}$/;
export const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const FILE_RE = /^([A-Za-z0-9_-]{1,64})-(metrics|processes)\.jsonl$/;

export interface ConfigServer {
  alias: string;
  mode: string;
  host?: string;
}

export interface ConfigInfo {
  path: string;
  servers: ConfigServer[];
  intervalSecs: number;
  processIntervalSecs: number;
  retentionDays: number;
  topN: number;
  warnings: string[];
  error?: string;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object"
    ? (value as Record<string, unknown>)
    : {};
}

function positiveInt(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.floor(value)
    : fallback;
}

/** Parses the same subset of config.toml the dashboard needs. */
export async function loadConfig(): Promise<ConfigInfo> {
  const fallback: ConfigInfo = {
    path: CONFIG_PATH,
    servers: [],
    intervalSecs: 5,
    processIntervalSecs: 60,
    retentionDays: 30,
    topN: 20,
    warnings: [],
  };
  let text: string;
  try {
    text = await Bun.file(CONFIG_PATH).text();
  } catch (error) {
    return {
      ...fallback,
      error: `无法读取配置 ${CONFIG_PATH}: ${
        error instanceof Error ? error.message : String(error)
      }`,
    };
  }
  let raw: Record<string, unknown>;
  try {
    raw = record(Bun.TOML.parse(text));
  } catch (error) {
    return {
      ...fallback,
      error: `配置文件解析失败 ${CONFIG_PATH}: ${
        error instanceof Error ? error.message : String(error)
      }`,
    };
  }

  const global = record(raw.global);
  const warnings: string[] = [];
  const servers: ConfigServer[] = [];
  const seen = new Set<string>();
  const rawServers = Array.isArray(raw.servers) ? raw.servers : [];
  for (const item of rawServers) {
    const s = record(item);
    const alias = typeof s.alias === "string" ? s.alias.trim() : "";
    if (!ALIAS_RE.test(alias)) {
      warnings.push(`忽略非法服务器别名: ${JSON.stringify(s.alias)}`);
      continue;
    }
    if (seen.has(alias)) {
      warnings.push(`忽略重复服务器别名: ${alias}`);
      continue;
    }
    seen.add(alias);
    const host = typeof s.ssh_host === "string" && s.ssh_host ? s.ssh_host : undefined;
    servers.push({
      alias,
      mode: typeof s.mode === "string" ? s.mode : "local",
      host,
    });
  }

  return {
    path: CONFIG_PATH,
    servers,
    intervalSecs: positiveInt(global.interval_secs, 5),
    processIntervalSecs: positiveInt(global.process_interval_secs, 60),
    retentionDays: positiveInt(global.retention_days, 30),
    topN: positiveInt(global.top_n_processes, 20),
    warnings,
    error: servers.length === 0 ? "配置中没有可用的服务器条目" : undefined,
  };
}

export function isValidDate(value: string): boolean {
  if (!DATE_RE.test(value)) return false;
  const [year, month, day] = value.split("-").map(Number);
  const dt = new Date(Date.UTC(year, month - 1, day));
  return (
    dt.getUTCFullYear() === year &&
    dt.getUTCMonth() === month - 1 &&
    dt.getUTCDate() === day
  );
}

/** UTC date directories, newest first. Ignores everything that is not YYYY-MM-DD. */
export async function listDates(): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(DATA_DIR, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((e) => e.isDirectory() && isValidDate(e.name))
    .map((e) => e.name)
    .sort((a, b) => (a < b ? 1 : -1));
}

/** Aliases that have data files, including aliases no longer in config.toml. */
export async function discoverAliases(dates: string[]): Promise<string[]> {
  const found = new Set<string>();
  for (const date of dates.slice(0, 62)) {
    let entries;
    try {
      entries = await readdir(join(DATA_DIR, date), { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (!entry.isFile()) continue;
      const match = FILE_RE.exec(entry.name);
      if (match) found.add(match[1]);
    }
  }
  return [...found].sort();
}

export function dataFile(
  date: string,
  alias: string,
  kind: "metrics" | "processes" | "host",
): string {
  if (!isValidDate(date)) throw new Error(`非法日期: ${date}`);
  if (!ALIAS_RE.test(alias)) throw new Error(`非法服务器别名: ${alias}`);
  return join(DATA_DIR, date, `${alias}-${kind}.jsonl`);
}

export async function fileSize(path: string): Promise<number | null> {
  try {
    const st = await stat(path);
    return st.isFile() ? st.size : null;
  } catch {
    return null;
  }
}

export interface TailRecord {
  value: unknown;
  line: string;
  mtimeMs: number;
  size: number;
}

/**
 * Reads the tail of a JSONL file and returns the last parseable line.
 * A partially-flushed final line (the daemon buffers writes) is skipped
 * instead of being reported as corruption.
 */
export async function tailJsonRecord(
  path: string,
  maxBytes = 256 * 1024,
): Promise<TailRecord | null> {
  let st;
  try {
    st = await stat(path);
  } catch {
    return null;
  }
  if (!st.isFile() || st.size === 0) return null;

  const start = Math.max(0, st.size - maxBytes);
  const length = st.size - start;
  const fh = await open(path, "r");
  try {
    const buf = Buffer.alloc(length);
    const { bytesRead } = await fh.read(buf, 0, length, start);
    const text = buf.toString("utf8", 0, bytesRead);
    const lines = text.split("\n");
    // When the window starts mid-file, its first segment is a partial line.
    const first = start > 0 ? 1 : 0;
    for (let i = lines.length - 1; i >= first; i--) {
      const line = lines[i].trim();
      if (!line) continue;
      try {
        return { value: JSON.parse(line), line, mtimeMs: st.mtimeMs, size: st.size };
      } catch {
        // Partial or corrupt line: keep walking backwards.
      }
    }
    return null;
  } finally {
    await fh.close();
  }
}

/**
 * Reads backwards from the end of a JSONL file and returns the newest record
 * whose `ts_ms` falls inside `[startMs, endMs)`.
 *
 * Needed because a local calendar day straddles two UTC partitions: the tail
 * of the later file is usually *after* the local day ends, so the day's last
 * snapshot sits earlier in that same file.
 */
export async function lastJsonRecordWithin(
  path: string,
  startMs: number,
  endMs: number,
  maxBytes = 16 * 1024 * 1024,
): Promise<TailRecord | null> {
  let st;
  try {
    st = await stat(path);
  } catch {
    return null;
  }
  if (!st.isFile() || st.size === 0) return null;

  const start = Math.max(0, st.size - maxBytes);
  const length = st.size - start;
  const fh = await open(path, "r");
  try {
    const buf = Buffer.alloc(length);
    const { bytesRead } = await fh.read(buf, 0, length, start);
    const lines = buf.toString("utf8", 0, bytesRead).split("\n");
    const first = start > 0 ? 1 : 0;
    for (let i = lines.length - 1; i >= first; i--) {
      const line = lines[i].trim();
      if (!line) continue;
      let value: unknown;
      try {
        value = JSON.parse(line);
      } catch {
        continue;
      }
      const record = value as { ts_ms?: unknown };
      const ts = typeof record.ts_ms === "number" ? record.ts_ms : null;
      if (ts === null) continue;
      if (ts >= startMs && ts < endMs) {
        return { value, line, mtimeMs: st.mtimeMs, size: st.size };
      }
      if (ts < startMs) break;
    }
    return null;
  } finally {
    await fh.close();
  }
}

export function dayStartMs(date: string): number {
  const [year, month, day] = date.split("-").map(Number);
  return Date.UTC(year, month - 1, day);
}

export function daysBetween(from: string, to: string): number {
  return Math.round((dayStartMs(to) - dayStartMs(from)) / 86_400_000);
}
