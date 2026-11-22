import { useMemo, useState } from "react";
import { ArrowRight, ClipboardCheck, Copy } from "lucide-react";

import {
  formatBytes,
  formatDateTime,
  formatDuration,
  formatPercent,
  relativeTime,
} from "../format";
import type { MetaResponse, OverviewResponse, OverviewServer } from "../shared/types";

const STATUS_LABEL: Record<OverviewServer["status"], string> = {
  online: "在线",
  stale: "数据过期",
  "no-data": "无数据",
};

function gpuLabel(server: OverviewServer): string {
  if (!server.gpu) return "—";
  if (server.gpu.state === "error") return "错误";
  if (server.gpu.state === "absent") return "无";
  const util = Math.max(...server.gpus.map((gpu) => gpu.utilPercent), 0);
  return `${server.gpus.length} × ${util.toFixed(0)}%`;
}

function gpuNote(server: OverviewServer): string {
  if (server.gpu?.state === "ok") {
    const vram = server.gpus.reduce((sum, gpu) => sum + gpu.memUsedMb, 0);
    const total = server.gpus.reduce((sum, gpu) => sum + gpu.memTotalMb, 0);
    return total > 0 ? `VRAM ${((vram / total) * 100).toFixed(0)}%` : "—";
  }
  if (server.gpu?.state === "error") return server.gpu.message ?? "探测失败";
  return "未检测到 NVIDIA GPU";
}

function hostAlert(server: OverviewServer): string | null {
  if (server.status === "no-data") return "没有可解析的 metrics";
  if (server.status === "stale") return "最后采样已超出过期窗口";
  if (server.gpu?.state === "error") return server.gpu.message ?? "GPU 探测失败";
  return null;
}

function buildSummary(servers: OverviewServer[]): string {
  const header =
    "| 主机 | 状态 | CPU | 内存 | GPU | 核数 | 最后采样 |\n|---|---|---|---|---|---|---|";
  const rows = servers.map((server) => {
    const host = server.hostMeta;
    const gpu =
      server.gpu?.state === "ok"
        ? `${server.gpus.length} × ${Math.max(...server.gpus.map((g) => g.utilPercent), 0).toFixed(0)}%`
        : server.gpu?.state === "error"
          ? "错误"
          : "无";
    return `| ${server.alias}${host?.hostname ? ` (${host.hostname})` : ""} | ${
      STATUS_LABEL[server.status]
    } | ${formatPercent(server.cpuPercent)} | ${formatPercent(server.memUsedPercent)} | ${gpu} | ${
      server.coreCount ?? "—"
    } | ${formatDateTime(server.lastSeenMs)} |`;
  });
  return [header, ...rows].join("\n");
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
  onSelect,
}: {
  overview: OverviewResponse | null;
  meta: MetaResponse;
  loading: boolean;
  onSelect: (alias: string) => void;
}) {
  const [copied, setCopied] = useState<"idle" | "ok" | "manual">("idle");
  const servers = overview?.servers ?? [];

  const totals = useMemo(() => {
    const online = servers.filter((server) => server.status === "online").length;
    const cores = servers.reduce(
      (sum, server) => sum + (server.coreCount ?? server.hostMeta?.coreCount ?? 0),
      0,
    );
    const gpus = servers.reduce((sum, server) => sum + server.gpus.length, 0);
    const memBytes = servers.reduce(
      (sum, server) => sum + (server.hostMeta?.memTotalBytes ?? server.memTotalBytes ?? 0),
      0,
    );
    const alerts = servers.filter((server) => hostAlert(server) !== null);
    return { online, cores, gpus, memBytes, alerts };
  }, [servers]);

  const copySummary = async () => {
    const text = buildSummary(servers);
    try {
      await navigator.clipboard.writeText(text);
      setCopied("ok");
    } catch {
      // Plain-http LAN origins have no Clipboard API; fall back to a
      // selection so the summary can still be copied by hand.
      setCopied("manual");
    }
    setTimeout(() => setCopied("idle"), 2500);
  };

  if (loading) return <div className="empty-state">正在汇总所有主机</div>;

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>总览</h2>
          <p className="section-sub">
            {servers.length} 台主机 · 数据 {meta.dates.length} 天
            {meta.dates.length > 0 ? `（${meta.dates[meta.dates.length - 1]} 起）` : ""}
          </p>
        </div>
        <div className="section-actions">
          <button className="ghost" onClick={copySummary} disabled={servers.length === 0}>
            {copied === "ok" ? (
              <ClipboardCheck size={14} strokeWidth={1.6} aria-hidden="true" />
            ) : (
              <Copy size={14} strokeWidth={1.6} aria-hidden="true" />
            )}
            {copied === "ok" ? "已复制" : "复制摘要"}
          </button>
        </div>
      </div>

      <div className="kpi-strip">
        <Kpi
          label="在线"
          value={`${totals.online} / ${servers.length}`}
          note={totals.alerts.length > 0 ? `${totals.alerts.length} 项异常` : "全部正常"}
        />
        <Kpi label="总核数" value={totals.cores.toLocaleString()} note="逻辑核心" />
        <Kpi label="GPU" value={String(totals.gpus)} note="已识别 NVIDIA GPU" />
        <Kpi label="内存总量" value={formatBytes(totals.memBytes)} note="所有主机合计" />
        <Kpi
          label="采样"
          value={`${meta.intervalSecs}s`}
          note={`快照 ${meta.processIntervalSecs}s`}
        />
        <Kpi
          label="保留"
          value={`${meta.retentionDays} 天`}
          note={overview ? `更新于 ${formatDateTime(overview.generatedAt)}` : "读取中"}
        />
      </div>

      {totals.alerts.length > 0 && (
        <div className="notice warn">
          <strong>需要关注</strong>
          <ul>
            {totals.alerts.map((server) => (
              <li key={server.alias}>
                <button className="inline-link" onClick={() => onSelect(server.alias)}>
                  {server.alias}
                </button>
                ：{hostAlert(server)}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="fleet">
        <div className="fleet-head" aria-hidden="true">
          <span>主机</span>
          <span>状态</span>
          <span>CPU</span>
          <span>内存</span>
          <span>GPU</span>
          <span>核数</span>
          <span>运行时间</span>
          <span>最后采样</span>
        </div>
        {servers.map((server) => (
          <button
            type="button"
            className="fleet-row fleet-row-button"
            key={server.alias}
            onClick={() => onSelect(server.alias)}
            aria-label={`查看 ${server.alias} 详情`}
          >
            <span className="fleet-host">
              <span className="fleet-title">
                <strong>{server.alias}</strong>
              </span>
              <span className="fleet-sub">
                <span className="mono">{server.hostMeta?.hostname ?? "—"}</span>
                <span className="muted"> · {server.mode}</span>
              </span>
            </span>
            <span className={`status ${server.status}`}>
              <span className="status-dot" aria-hidden="true" />
              {STATUS_LABEL[server.status]}
            </span>
            <span className="fleet-stat" data-label="CPU">
              <strong className="stat-value">{formatPercent(server.cpuPercent)}</strong>
              <span className="meter" aria-hidden="true">
                <span
                  className="meter-fill"
                  style={{ width: `${Math.max(0, Math.min(100, server.cpuPercent ?? 0))}%` }}
                />
              </span>
            </span>
            <span className="fleet-stat" data-label="内存">
              <strong className="stat-value">{formatPercent(server.memUsedPercent)}</strong>
              <span
                className="stat-note mono"
                title={`${formatBytes(server.memUsedBytes)} / ${formatBytes(server.memTotalBytes)}`}
              >
                {formatBytes(server.memUsedBytes)} / {formatBytes(server.memTotalBytes)}
              </span>
            </span>
            <span className="fleet-stat" data-label="GPU">
              <strong className="stat-value">{gpuLabel(server)}</strong>
              <span className="stat-note mono" title={gpuNote(server)}>
                {gpuNote(server)}
              </span>
            </span>
            <span className="fleet-stat" data-label="核数">
              <strong className="stat-value">{server.coreCount ?? "—"}</strong>
              <span className="stat-note mono">p95 {formatPercent(server.perCoreP95Percent)}</span>
            </span>
            <span className="fleet-stat" data-label="运行时间">
              <strong className="stat-value">
                {server.hostMeta?.uptimeSecs === null ||
                server.hostMeta?.uptimeSecs === undefined
                  ? "—"
                  : formatDuration(server.hostMeta.uptimeSecs * 1000)}
              </strong>
            </span>
            <span className="fleet-time" data-label="最后采样">
              <strong className="stat-time">{relativeTime(server.lastSeenMs)}</strong>
              <span className="stat-note mono" title={formatDateTime(server.lastSeenMs)}>
                {formatDateTime(server.lastSeenMs)}
              </span>
            </span>
            <span className="fleet-chevron" aria-hidden="true">
              <ArrowRight size={14} strokeWidth={1.6} />
            </span>
          </button>
        ))}
      </div>

      {copied === "manual" && (
        <div className="notice warn">
          <strong>当前为 http 内网地址，浏览器不允许自动写剪贴板</strong>
          <p>下面是可直接选中的 Markdown 摘要：</p>
          <pre className="summary-text">{buildSummary(servers)}</pre>
        </div>
      )}

      <p className="fleet-hint muted small">
        点击任意主机进入单机视图。汇总只读取最新的 metrics 与 host 记录，不写数据目录。
      </p>
    </section>
  );
}
