import { useEffect, useMemo, useState } from "react";
import { ClipboardCheck, Copy } from "lucide-react";

import { api } from "../api";
import { DateRangeBar } from "./DateRangeBar";
import {
  formatDateTime,
  formatDuration,
  formatPercent,
  formatRate,
  formatRateCompact,
  relativeTime,
} from "../format";
import type {
  MetaResponse,
  OverviewResponse,
  OverviewServer,
  SeriesPoint,
} from "../shared/types";

const MAX_POINTS = "240";
type Metric = "cpu" | "mem" | "gpuUtil" | "iowait" | "diskRead" | "diskWrite";

const STATUS_LABEL: Record<OverviewServer["status"], string> = {
  online: "在线",
  stale: "数据过期",
  "no-data": "无数据",
};

interface HostHistory {
  alias: string;
  mode: string;
  status: OverviewServer["status"];
  hostMeta: OverviewServer["hostMeta"];
  gpus: number;
  points: SeriesPoint[];
  fsMax: number | null;
  diskRead: number | null;
  diskWrite: number | null;
  cpuAvg: number | null;
  cpuP95: number | null;
  cpuMax: number | null;
  memAvg: number | null;
  gpuAvg: number | null;
  ioAvg: number | null;
  coverage: number | null;
  samples: number;
  gaps: number;
  largestGapSecs: number;
  lastPointMs: number | null;
  error: string | null;
}

function weighted(
  points: SeriesPoint[],
  field: Metric,
): { avg: number | null; p95: number | null; max: number | null } {
  const rows: Array<{ value: number; weight: number }> = [];
  let sum = 0;
  let weightSum = 0;
  let max = -Infinity;
  for (const point of points) {
    const value = point[field];
    if (value === null) continue;
    const weight = Math.max(1, point.samples);
    rows.push({ value, weight });
    sum += value * weight;
    weightSum += weight;
    max = Math.max(max, value);
  }
  if (rows.length === 0) return { avg: null, p95: null, max: null };
  rows.sort((a, b) => a.value - b.value);
  const target = weightSum * 0.95;
  let accumulated = 0;
  let p95 = rows[rows.length - 1].value;
  for (const row of rows) {
    accumulated += row.weight;
    if (accumulated >= target) {
      p95 = row.value;
      break;
    }
  }
  return { avg: sum / weightSum, p95, max: max === -Infinity ? null : max };
}

function summarize(
  server: OverviewServer,
  points: SeriesPoint[],
  coverage: number | null,
  samples: number,
  gaps: number,
  largestGapSecs: number,
): HostHistory {
  const cpu = weighted(points, "cpu");
  const mem = weighted(points, "mem");
  const gpu = weighted(points, "gpuUtil");
  const io = weighted(points, "iowait");
  const read = weighted(points, "diskRead");
  const write = weighted(points, "diskWrite");
  const lastPointMs = points.length > 0 ? Math.max(...points.map((p) => p.t)) : null;
  const effectiveCoverage = samples > 0 ? coverage : null;
  const fsMax = points.reduce<number | null>(
    (max, point) =>
      point.fsUsedMax === null
        ? max
        : max === null
          ? point.fsUsedMax
          : Math.max(max, point.fsUsedMax),
    null,
  );
  return {
    alias: server.alias,
    mode: server.mode,
    status: server.status,
    hostMeta: server.hostMeta,
    gpus: server.gpus.length,
    points,
    fsMax,
    diskRead: read.avg,
    diskWrite: write.avg,
    cpuAvg: cpu.avg,
    cpuP95: cpu.p95,
    cpuMax: cpu.max,
    memAvg: mem.avg,
    gpuAvg: gpu.avg,
    ioAvg: io.avg,
    coverage: effectiveCoverage,
    samples,
    gaps,
    largestGapSecs,
    lastPointMs,
    error: null,
  };
}

function Sparkline({ points }: { points: SeriesPoint[] }) {
  const raw = points
    .map((point) => point.cpu)
    .filter((value): value is number => value !== null);
  if (raw.length < 2) return null;
  const step = Math.ceil(raw.length / 80);
  const samples: number[] = [];
  for (let i = 0; i < raw.length; i += step) {
    const slice = raw.slice(i, i + step);
    samples.push(slice.reduce((sum, value) => sum + value, 0) / slice.length);
  }
  const max = Math.max(...samples, 1);
  const width = 100;
  const height = 26;
  const coords = samples.map((value, index) => {
    const x = (index / Math.max(1, samples.length - 1)) * width;
    const y = height - 2 - (value / max) * (height - 4);
    return [x, y] as const;
  });
  const line = coords
    .map(([x, y], index) => `${index === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`)
    .join(" ");
  const area = `${line} L${width} ${height} L0 ${height} Z`;
  return (
    <svg
      className="sparkline"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <path d={area} className="spark-area" />
      <path d={line} className="spark-line" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function MiniMeter({ value, tone }: { value: number | null; tone?: string }) {
  return (
    <span className={`meter ${tone ?? ""}`} aria-hidden="true">
      <span
        className="meter-fill"
        style={{ width: `${value === null ? 0 : Math.max(0, Math.min(100, value))}%` }}
      />
    </span>
  );
}

function buildSummary(rows: HostHistory[], from: string, to: string): string {
  const header =
    "| 主机 | CPU 均值 | CPU p95 | 内存均值 | GPU 均值 | 磁盘峰值 | 覆盖 | 样本 | 最后数据 |\n|---|---|---|---|---|---|---|---|---|";
  const body = rows.map(
    (row) =>
      `| ${row.alias}${row.hostMeta?.hostname ? ` (${row.hostMeta.hostname})` : ""} | ${
        formatPercent(row.cpuAvg)
      } | ${formatPercent(row.cpuP95)} | ${formatPercent(row.memAvg)} | ${
        row.gpuAvg === null ? "无 GPU" : formatPercent(row.gpuAvg)
      } | ${formatPercent(row.fsMax)} | ${
        row.coverage === null ? "—" : `${row.coverage}%`
      } | ${row.samples.toLocaleString()} | ${
        row.lastPointMs === null ? "—" : formatDateTime(row.lastPointMs)
      } |`,
  );
  return [`统计范围：${from} → ${to}（本地日期）`, "", header, ...body].join("\n");
}

function Kpi({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note?: string;
}) {
  return (
    <div className="kpi">
      <span className="kpi-label">{label}</span>
      <strong className="kpi-value">{value}</strong>
      {note && <span className="kpi-note mono">{note}</span>}
    </div>
  );
}

export function FleetPanel({
  overview,
  meta,
  loading,
  reloadToken,
  onRetry,
  onSelect,
}: {
  overview: OverviewResponse | null;
  meta: MetaResponse;
  loading: boolean;
  reloadToken: number;
  onRetry: () => void;
  onSelect: (alias: string) => void;
}) {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [rows, setRows] = useState<HostHistory[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<"idle" | "ok" | "manual">("idle");

  useEffect(() => {
    const latest = meta.dates[0] ?? "";
    setFrom((previous) => (previous && meta.dates.includes(previous) ? previous : latest));
    setTo((previous) => (previous && meta.dates.includes(previous) ? previous : latest));
  }, [meta]);

  const servers = overview?.servers ?? [];
  const serverKey = servers.map((server) => server.alias).join(",");

  useEffect(() => {
    if (!from || !to || servers.length === 0) return;
    const controller = new AbortController();
    setScanning(true);
    setError(null);
    Promise.all(
      servers.map(async (server) => {
        try {
          const series = await api.series(
            { server: server.alias, from, to, maxPoints: MAX_POINTS },
            controller.signal,
          );
          return summarize(
            server,
            series.points,
            series.coveragePercent,
            series.samples,
            series.gaps.length,
            series.largestGapSecs,
          );
        } catch (cause) {
          if (controller.signal.aborted) throw cause;
          return {
            alias: server.alias,
            mode: server.mode,
            status: server.status,
            hostMeta: server.hostMeta,
            gpus: server.gpus.length,
            points: [],
            fsMax: null,
            diskRead: null,
            diskWrite: null,
            cpuAvg: null,
            cpuP95: null,
            cpuMax: null,
            memAvg: null,
            gpuAvg: null,
            ioAvg: null,
            coverage: null,
            samples: 0,
            gaps: 0,
            largestGapSecs: 0,
            lastPointMs: null,
            error: cause instanceof Error ? cause.message : String(cause),
          } satisfies HostHistory;
        }
      }),
    )
      .then((result) => setRows(result))
      .catch((cause) => {
        if (!controller.signal.aborted) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setScanning(false);
      });
    return () => controller.abort();
    // serverKey tracks host-set changes without depending on object identity.
  }, [from, to, serverKey, reloadToken]);

  const totals = useMemo(() => {
    if (!rows) return null;
    const samples = rows.reduce((sum, row) => sum + row.samples, 0);
    const covered = rows.filter((row) => row.coverage !== null && row.samples > 0);
    const coverage =
      covered.length > 0
        ? covered.reduce((sum, row) => sum + (row.coverage ?? 0), 0) / covered.length
        : null;
    const gpus = rows.reduce((sum, row) => sum + row.gpus, 0);
    const largestGap = rows.reduce((max, row) => Math.max(max, row.largestGapSecs), 0);
    const days =
      from && to
        ? Math.max(
            1,
            Math.round(
              (Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) /
                86_400_000,
            ) + 1,
          )
        : 0;
    return { samples, coverage, gpus, largestGap, days };
  }, [rows, from, to]);

  const copySummary = async () => {
    if (!rows) return;
    try {
      await navigator.clipboard.writeText(buildSummary(rows, from, to));
      setCopied("ok");
    } catch {
      setCopied("manual");
    }
    setTimeout(() => setCopied("idle"), 2500);
  };

  if (loading) return <div className="empty-state">正在读取主机清单</div>;

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>历史分析</h2>
          <p className="section-sub">
            按时间范围聚合每台主机，用于复盘；右上状态点只表示当前是否在线
          </p>
        </div>
        <div className="section-actions">
          <button className="ghost" onClick={copySummary} disabled={!rows}>
            {copied === "ok" ? (
              <ClipboardCheck size={14} strokeWidth={1.6} aria-hidden="true" />
            ) : (
              <Copy size={14} strokeWidth={1.6} aria-hidden="true" />
            )}
            {copied === "ok" ? "已复制" : "复制摘要"}
          </button>
        </div>
      </div>

      <DateRangeBar
        dates={meta.dates}
        from={from}
        to={to}
        ariaPrefix="统计"
        onChange={(nextFrom, nextTo) => {
          setFrom(nextFrom);
          setTo(nextTo);
        }}
      />

      <div className="fleet-toolbar">
        <span className={`scan-state ${scanning ? "busy" : ""}`} aria-live="polite">
          {scanning ? "正在聚合所选范围…" : rows ? "已聚合" : "等待数据"}
        </span>
        {rows && (
          <span className="muted small mono">
            {rows.reduce((sum, row) => sum + row.samples, 0).toLocaleString()} samples
          </span>
        )}
      </div>

      {error && (
        <div className="notice error">
          <strong>统计失败</strong>
          <p>{error}</p>
          <button className="ghost" onClick={onRetry}>
            重试
          </button>
        </div>
      )}

      {totals && (
        <div className="kpi-strip">
          <Kpi
            label="统计窗口"
            value={`${totals.days} 天`}
            note={`${from.slice(5)} → ${to.slice(5)}`}
          />
          <Kpi label="主机" value={String(rows?.length ?? 0)} note="参与聚合" />
          <Kpi label="总样本" value={totals.samples.toLocaleString()} note="metrics 行数" />
          <Kpi
            label="平均覆盖"
            value={totals.coverage === null ? "—" : `${totals.coverage.toFixed(1)}%`}
            note="按主机平均"
          />
          <Kpi label="GPU" value={String(totals.gpus)} note="当前识别数量" />
          <Kpi
            label="最长缺口"
            value={totals.largestGap > 0 ? formatDuration(totals.largestGap * 1000) : "无"}
            note={totals.largestGap > 0 ? "采样缺口" : "数据连续"}
          />
        </div>
      )}

      <div className="fleet-scroll">
        <div className={`fleet fleet-history ${scanning ? "is-scanning" : ""}`}>
          <div className="fleet-head" aria-hidden="true">
            <span>主机</span>
            <span>CPU 走势</span>
            <span>内存</span>
            <span>GPU</span>
            <span>磁盘</span>
            <span>覆盖</span>
            <span>最后数据</span>
          </div>
          {(rows ?? []).map((row) => (
            <button
              type="button"
              className="fleet-row fleet-row-button"
              key={row.alias}
              onClick={() => onSelect(row.alias)}
              aria-label={`查看 ${row.alias} 历史`}
            >
              <span className="fleet-host">
                <span className="fleet-title">
                  <strong>{row.alias}</strong>
                  <span className={`status ${row.status}`}>
                    <span className="status-dot" aria-hidden="true" />
                    {STATUS_LABEL[row.status]}
                  </span>
                </span>
                <span className="fleet-sub">
                  <span className="mono">{row.hostMeta?.hostname ?? "—"}</span>
                  <span className="muted"> · {row.mode}</span>
                </span>
                {row.error && <span className="fleet-error">{row.error}</span>}
              </span>

              <span className="fleet-stat cpu-stat" data-label="CPU 走势">
                <span className="cpu-line">
                  <strong className="stat-value">{formatPercent(row.cpuAvg)}</strong>
                  <span className="stat-note mono">p95 {formatPercent(row.cpuP95)}</span>
                </span>
                <Sparkline points={row.points} />
              </span>

              <span className="fleet-stat" data-label="内存">
                <strong className="stat-value">{formatPercent(row.memAvg)}</strong>
                <MiniMeter value={row.memAvg} tone="mem" />
              </span>

              <span className="fleet-stat" data-label="GPU">
                <strong className="stat-value">
                  {row.gpuAvg === null ? "无" : formatPercent(row.gpuAvg)}
                </strong>
                {row.gpus > 0 ? (
                  <MiniMeter value={row.gpuAvg} tone="gpu" />
                ) : (
                  <span className="stat-note mono">无 GPU</span>
                )}
              </span>

              <span className="fleet-stat" data-label="磁盘">
                <strong
                  className={`stat-value ${row.fsMax !== null && row.fsMax >= 90 ? "danger" : ""}`}
                >
                  {formatPercent(row.fsMax)}
                </strong>
                <MiniMeter value={row.fsMax} tone="disk" />
                <span
                  className="stat-note mono"
                  title={`读 ${formatRate(row.diskRead)} · 写 ${formatRate(row.diskWrite)}`}
                >
                  ↓{formatRateCompact(row.diskRead)} ↑{formatRateCompact(row.diskWrite)}
                </span>
              </span>

              <span className="fleet-stat" data-label="覆盖">
                <strong
                  className={`stat-value ${
                    row.coverage !== null && row.coverage < 80 ? "danger" : ""
                  }`}
                >
                  {row.coverage === null ? "—" : `${row.coverage.toFixed(1)}%`}
                </strong>
                <span className="stat-note mono">
                  {row.samples === 0
                    ? "无数据"
                    : row.gaps > 0
                      ? `${row.gaps} 段缺口`
                      : "连续"}
                </span>
              </span>

              <span className="fleet-time" data-label="最后数据">
                <strong className="stat-time">{relativeTime(row.lastPointMs)}</strong>
                <span className="stat-note mono">{formatDateTime(row.lastPointMs)}</span>
              </span>

            </button>
          ))}
        </div>
      </div>

      {!rows && scanning && (
        <div className="skeleton-list" aria-hidden="true">
          {[0, 1, 2].map((index) => (
            <div className="skeleton-row" key={index}>
              <span />
              <span />
              <span />
              <span />
            </div>
          ))}
        </div>
      )}

      {copied === "manual" && (
        <div className="notice warn">
          <strong>当前为 http 内网地址，浏览器不允许自动写剪贴板</strong>
          <p>下面是可直接选中的 Markdown 摘要：</p>
          <pre className="summary-text">{buildSummary(rows ?? [], from, to)}</pre>
        </div>
      )}

      <p className="fleet-hint muted small">
        数字是所选时间范围的平均/峰值，不是当前瞬时值。点击任意主机进入单机视图。
      </p>
    </section>
  );
}
