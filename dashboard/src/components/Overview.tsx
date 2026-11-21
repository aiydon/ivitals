import {
  formatBytes,
  formatDateTime,
  formatDuration,
  formatPercent,
  formatTime,
  relativeTime,
} from "../format";
import type { OverviewServer } from "../shared/types";

const STATUS_LABEL: Record<OverviewServer["status"], string> = {
  online: "在线",
  stale: "数据过期",
  "no-data": "无数据",
};

function Status({ status }: { status: OverviewServer["status"] }) {
  return (
    <span className={`status ${status}`}>
      <span className="status-dot" aria-hidden="true" />
      {STATUS_LABEL[status]}
    </span>
  );
}

function Stat({
  label,
  value,
  note,
  meter,
}: {
  label: string;
  value: string;
  note?: string;
  meter?: number | null;
}) {
  return (
    <div className="host-stat">
      <span className="stat-label">{label}</span>
      <strong className="stat-value">{value}</strong>
      {note && <span className="stat-note mono">{note}</span>}
      {meter !== undefined && (
        <span className="meter" aria-hidden="true">
          <span
            className="meter-fill"
            style={{ width: `${meter === null ? 0 : Math.max(0, Math.min(100, meter))}%` }}
          />
        </span>
      )}
    </div>
  );
}

export function OverviewPanel({
  server,
  loading,
  error,
}: {
  server: OverviewServer | null;
  loading: boolean;
  error: string | null;
}) {
  if (loading) return <div className="empty-state">正在读取主机状态</div>;
  if (error) {
    return (
      <div className="notice error">
        <strong>主机读取失败</strong>
        <p>{error}</p>
      </div>
    );
  }
  if (!server) return <div className="empty-state">选择一台主机查看状态</div>;

  const host = server.hostMeta;
  const now = Date.now();
  const gpuUtil = server.gpus.reduce((max, gpu) => Math.max(max, gpu.utilPercent), 0);
  const gpuText =
    server.gpu?.state === "ok"
      ? `${server.gpus.length} × ${gpuUtil.toFixed(0)}%`
      : server.gpu?.state === "error"
        ? "错误"
        : "无 GPU";
  const gpuNote =
    server.gpu?.state === "ok" && server.gpus[0]
      ? `VRAM ${formatPercent(server.gpus[0].memUtilPercent)} · ${server.gpus[0].tempC.toFixed(0)}°C`
      : server.gpu?.state === "error"
        ? server.gpu.message ?? "探测失败"
        : "未检测到 NVIDIA GPU";

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>主机状态</h2>
          <p className="section-sub">
            {host?.hostname ?? "—"} · {host?.os ?? server.mode}
            {host?.kernel ? ` · ${host.kernel}` : ""}
          </p>
        </div>
        <div className="section-status">
          <Status status={server.status} />
          <span className="section-meta mono">{relativeTime(server.lastSeenMs, now)}</span>
        </div>
      </div>

      {server.error && <div className="notice error">{server.error}</div>}

      <div className="host-stats">
        <Stat
          label="CPU"
          value={formatPercent(server.cpuPercent)}
          note={`load ${server.load1?.toFixed(2) ?? "—"}`}
          meter={server.cpuPercent}
        />
        <Stat
          label="内存"
          value={formatPercent(server.memUsedPercent)}
          note={`${formatBytes(server.memUsedBytes)} / ${formatBytes(server.memTotalBytes)}`}
          meter={server.memUsedPercent}
        />
        <Stat label="GPU" value={gpuText} note={gpuNote} />
        <Stat
          label="核心"
          value={server.coreCount === null ? "—" : String(server.coreCount)}
          note={`mean ${formatPercent(server.perCoreMeanPercent)} · p95 ${formatPercent(server.perCoreP95Percent)}`}
        />
        <Stat
          label="iowait"
          value={formatPercent(server.iowaitPercent, 2)}
          note={`over 50%: ${server.coresOver50 ?? "—"}`}
        />
        <Stat
          label="运行时间"
          value={
            host?.uptimeSecs === null || host?.uptimeSecs === undefined
              ? "—"
              : formatDuration(host.uptimeSecs * 1000)
          }
          note={`最后采样 ${formatTime(server.lastSeenMs)}`}
        />
      </div>

      <div className="host-facts">
        <div><span>CPU</span><strong>{host?.cpuModel ?? "—"}</strong></div>
        <div><span>内存</span><strong>{formatBytes(host?.memTotalBytes ?? null)}</strong></div>
        <div><span>架构</span><strong>{host?.arch ?? "—"}</strong></div>
        <div><span>启动</span><strong>{formatDateTime(host?.bootTimeMs ?? null)}</strong></div>
      </div>

      <div className="detail-grid">
        <div className="inset">
          <div className="inset-head">
            <h3>加速器</h3>
            <span className="mono muted">{server.gpus.length} 张</span>
          </div>
          {server.gpus.length === 0 ? (
            <div className="inset-empty">
              {server.gpu?.state === "error"
                ? server.gpu.message ?? "GPU 探测失败"
                : "未检测到 NVIDIA GPU"}
            </div>
          ) : (
            <div className="accelerator-list">
              {server.gpus.map((gpu) => (
                <div className="accelerator-row" key={gpu.index}>
                  <div className="accelerator-name">
                    <span className="mono muted">GPU {gpu.index}</span>
                    <strong>{gpu.name}</strong>
                    <span className="mono tiny">{gpu.pstate ?? "—"}</span>
                  </div>
                  <div className="accelerator-stats">
                    <span><b>{formatPercent(gpu.utilPercent)}</b> util</span>
                    <span><b>{formatPercent(gpu.memUtilPercent)}</b> VRAM</span>
                    <span><b>{gpu.tempC.toFixed(0)}°C</b></span>
                    <span><b>{gpu.powerW === null ? "—" : `${gpu.powerW.toFixed(0)}W`}</b></span>
                    <span><b>{gpu.fanPercent === null ? "—" : `${gpu.fanPercent}%`}</b> fan</span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="inset">
          <div className="inset-head">
            <h3>存储</h3>
            <span className="mono muted">{server.filesystems.length} 个挂载点</span>
          </div>
          {server.filesystems.length === 0 ? (
            <div className="inset-empty">没有文件系统数据</div>
          ) : (
            <div className="storage-mounts storage-mounts-wide">
              {server.filesystems.slice(0, 6).map((fs) => (
                <div className="storage-mount" key={`${fs.device}-${fs.mount}`}>
                  <span className="storage-mount-name">{fs.mount}</span>
                  <span className="meter" aria-hidden="true">
                    <span
                      className="meter-fill"
                      style={{ width: `${Math.max(0, Math.min(100, fs.usedPercent))}%` }}
                    />
                  </span>
                  <span className="mono tiny">{formatPercent(fs.usedPercent)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
