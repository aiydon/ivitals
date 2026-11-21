import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  Database,
  Grid3X3,
  LayoutDashboard,
  ListTree,
  Moon,
  RefreshCw,
  ShieldCheck,
  Sun,
} from "lucide-react";

import { api } from "./api";
import { CorePanel } from "./components/CorePanel";
import { OverviewPanel } from "./components/Overview";
import { ProcessPanel } from "./components/ProcessPanel";
import { SeriesPanel } from "./components/SeriesPanel";
import { formatBytes, formatDateTime, formatTime } from "./format";
import type { MetaResponse, OverviewResponse } from "./shared/types";
import { useTheme } from "./theme";

const NAV = [
  { id: "overview", label: "主机", icon: LayoutDashboard },
  { id: "telemetry", label: "时序", icon: Activity },
  { id: "cores", label: "多核", icon: Grid3X3 },
  { id: "processes", label: "进程", icon: ListTree },
] as const;

const AUTO_REFRESH_SECS = 60;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function StatusDot({ status }: { status: string }) {
  return <span className={`status-dot ${status}`} aria-hidden="true" />;
}

export function App() {
  const [meta, setMeta] = useState<MetaResponse | null>(null);
  const [metaError, setMetaError] = useState<string | null>(null);
  const [overview, setOverview] = useState<OverviewResponse | null>(null);
  const [overviewError, setOverviewError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [refreshTick, setRefreshTick] = useState(0);
  const [selected, setSelected] = useState(() => {
    try {
      return localStorage.getItem("ivitals-host") ?? "";
    } catch {
      return "";
    }
  });
  const { theme, toggle } = useTheme();

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      const next = await api.meta();
      setMeta(next);
      setMetaError(null);
      setNonce((value) => value + 1);
    } catch (error) {
      setMetaError(errorText(error));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    api
      .meta(controller.signal)
      .then((next) => {
        setMeta(next);
        setMetaError(null);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setMetaError(errorText(error));
      });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    if (!meta) return;
    const controller = new AbortController();
    const load = async () => {
      try {
        const next = await api.overview(controller.signal);
        setOverview(next);
        setOverviewError(null);
      } catch (error) {
        if (!controller.signal.aborted) setOverviewError(errorText(error));
      }
    };
    void load();
    const timer = setInterval(load, 30_000);
    return () => {
      controller.abort();
      clearInterval(timer);
    };
  }, [meta, nonce]);

  useEffect(() => {
    const timer = setInterval(() => setRefreshTick((value) => value + 1), AUTO_REFRESH_SECS * 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!meta) return;
    const valid = meta.servers.some((server) => server.alias === selected);
    if (!valid) setSelected(meta.servers[0]?.alias ?? "");
  }, [meta, selected]);

  useEffect(() => {
    if (!selected) return;
    try {
      localStorage.setItem("ivitals-host", selected);
    } catch {
      // Ignore storage failures.
    }
  }, [selected]);

  if (!meta && metaError) {
    return (
      <main className="app-loading">
        <div className="notice error">
          <strong>看板初始化失败</strong>
          <p>{metaError}</p>
          <button onClick={refresh} disabled={busy}>重试</button>
        </div>
      </main>
    );
  }

  if (!meta) {
    return <main className="app-loading"><div className="empty-state">正在读取配置与数据目录</div></main>;
  }

  const hostRows =
    overview?.servers.map((server) => ({
      alias: server.alias,
      status: server.status,
      hostMeta: server.hostMeta,
    })) ??
    meta.servers.map((server) => ({
      alias: server.alias,
      status: "no-data",
      hostMeta: null,
    }));
  const selectedServer = overview?.servers.find((server) => server.alias === selected) ?? null;
  const hostMeta = selectedServer?.hostMeta ?? null;
  const generated = overview?.generatedAt ?? meta.generatedAt;
  const title = selectedServer?.alias ?? "集群遥测";
  const lede = selectedServer
    ? [
        hostMeta?.hostname,
        selectedServer.coreCount ? `${selectedServer.coreCount} 核` : null,
        hostMeta?.memTotalBytes ? formatBytes(hostMeta.memTotalBytes) : null,
        selectedServer.mode,
        `自动刷新 ${AUTO_REFRESH_SECS}s`,
      ]
        .filter(Boolean)
        .join(" · ")
    : `${hostRows.length} 台主机`;

  return (
    <div className="shell">
      <aside className="sidebar">
        <a className="brand" href="#overview">
          <span className="brand-mark" aria-hidden="true">
            <span /><span /><span />
          </span>
          <span>
            <span className="wordmark">ivitals</span>
            <span className="brand-sub">Fleet telemetry</span>
          </span>
        </a>

        <nav className="side-nav" aria-label="Sections">
          {NAV.map(({ id, label, icon: Icon }) => (
            <a key={id} href={`#${id}`} className="nav-item">
              <Icon size={16} strokeWidth={1.5} aria-hidden="true" />
              <span>{label}</span>
            </a>
          ))}
        </nav>

        <div className="side-block">
          <div className="side-label">主机</div>
          <div className="host-list">
            {hostRows.map((host) => (
              <button
                type="button"
                className={`host-row ${host.alias === selected ? "active" : ""}`}
                key={host.alias}
                onClick={() => setSelected(host.alias)}
                aria-pressed={host.alias === selected}
                aria-label={`选择主机 ${host.alias}`}
              >
                <span className="host-id">
                  <StatusDot status={host.status} />
                  <span>{host.alias}</span>
                </span>
                <span className="host-meta mono">
                  {host.hostMeta?.hostname ?? "—"}
                </span>
              </button>
            ))}
          </div>
        </div>

        <div className="side-footer">
          <div className="readonly">
            <ShieldCheck size={14} strokeWidth={1.5} aria-hidden="true" />
            <span>只读 · 自动刷新 {AUTO_REFRESH_SECS}s</span>
          </div>
          <div className="side-path mono" title={meta.dataDir}>
            <Database size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>{meta.dataDir}</span>
          </div>
          <div className="mono tiny">{meta.dates.length} 个日期目录</div>
        </div>
      </aside>

      <main className="main">
        <header className="page-header">
          <div>
            <h1>{title}</h1>
            <p className="lede">{lede}</p>
          </div>
          <div className="header-actions">
            <div className="mono updated">
              <span>更新于 {formatTime(generated)}</span>
              <span className="muted">{formatDateTime(generated)}</span>
            </div>
            <button
              className="ghost icon-button"
              onClick={toggle}
              aria-label={theme === "dark" ? "切换到浅色模式" : "切换到深色模式"}
              title={theme === "dark" ? "浅色模式" : "深色模式"}
            >
              {theme === "dark" ? (
                <Sun size={15} strokeWidth={1.6} aria-hidden="true" />
              ) : (
                <Moon size={15} strokeWidth={1.6} aria-hidden="true" />
              )}
            </button>
            <button onClick={refresh} disabled={busy}>
              <RefreshCw size={15} strokeWidth={1.6} aria-hidden="true" />
              {busy ? "刷新中" : "刷新"}
            </button>
          </div>
        </header>

        {meta.warnings.length > 0 && (
          <div className="notice warn">
            <strong>配置提示</strong>
            <ul>
              {meta.warnings.map((warning, index) => (
                <li key={`${index}-${warning}`}>{warning}</li>
              ))}
            </ul>
          </div>
        )}

        {overviewError && (
          <div className="notice error">
            <strong>总览读取失败</strong>
            <p>{overviewError}</p>
          </div>
        )}

        <div id="overview" className="section">
          <OverviewPanel
            server={selectedServer}
            loading={!overview && !overviewError}
            error={null}
          />
        </div>
        <div id="telemetry" className="section">
          <SeriesPanel meta={meta} server={selected} refreshTick={refreshTick} />
        </div>
        <div id="cores" className="section">
          <CorePanel meta={meta} server={selected} refreshTick={refreshTick} />
        </div>
        <div id="processes" className="section">
          <ProcessPanel meta={meta} server={selected} refreshTick={refreshTick} />
        </div>

        <footer className="page-footer">
          <span>ivitals · 数据目录只读</span>
          <span className="mono">{meta.configPath}</span>
        </footer>
      </main>
    </div>
  );
}
