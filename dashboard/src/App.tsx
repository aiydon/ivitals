import { useCallback, useEffect, useState } from "react";
import {
  Database,
  Gauge,
  Grid3X3,
  LayoutDashboard,
  ListTree,
  Moon,
  RefreshCw,
  ShieldCheck,
  Sun,
  Timer,
} from "lucide-react";

import { api } from "./api";
import { CorePanel } from "./components/CorePanel";
import { FleetPanel } from "./components/Fleet";
import { OverviewPanel } from "./components/Overview";
import { ProcessPanel } from "./components/ProcessPanel";
import { SeriesPanel } from "./components/SeriesPanel";
import { formatBytes, formatDateTime } from "./format";
import type { MetaResponse, OverviewResponse } from "./shared/types";
import { useTheme } from "./theme";

const HOST_TABS = ["overview", "telemetry", "cores", "processes"] as const;
type HostTab = (typeof HOST_TABS)[number];

const TAB_LABEL: Record<HostTab, string> = {
  overview: "概览",
  telemetry: "时序",
  cores: "多核",
  processes: "进程",
};

interface Route {
  view: "fleet" | "host";
  alias: string;
  tab: HostTab;
}

function isHostTab(value: string | undefined): value is HostTab {
  return value !== undefined && (HOST_TABS as readonly string[]).includes(value);
}

function parseRoute(): Route {
  const hash = decodeURIComponent(window.location.hash.replace("#", ""));
  if (!hash || hash === "fleet") return { view: "fleet", alias: "", tab: "overview" };
  const [alias, tab] = hash.split("/");
  return { view: "host", alias, tab: isHostTab(tab) ? tab : "overview" };
}

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
  const [route, setRoute] = useState<Route>(parseRoute);
  const [selected, setSelected] = useState(() => {
    const initial = parseRoute();
    if (initial.alias) return initial.alias;
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
    api
      .overview(controller.signal)
      .then((next) => {
        setOverview(next);
        setOverviewError(null);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setOverviewError(errorText(error));
      });
    return () => controller.abort();
  }, [meta, nonce]);

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

  useEffect(() => {
    const hash =
      route.view === "fleet" ? "#fleet" : `#${route.alias}/${route.tab}`;
    window.history.replaceState(null, "", hash);
    window.scrollTo({ top: 0, behavior: "smooth" });
  }, [route]);

  useEffect(() => {
    const onHashChange = () => setRoute(parseRoute());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const openFleet = () => setRoute({ view: "fleet", alias: selected, tab: "overview" });
  const openHost = (alias: string, tab: HostTab = "overview") => {
    setSelected(alias);
    setRoute({ view: "host", alias, tab });
  };

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
  const isFleet = route.view === "fleet";

  const title = isFleet ? "集群总览" : (selectedServer?.alias ?? "主机");
  const lede = isFleet
    ? `${hostRows.length} 台主机 · 历史聚合`
    : selectedServer
      ? [
          hostMeta?.hostname,
          selectedServer.coreCount ? `${selectedServer.coreCount} 核` : null,
          hostMeta?.memTotalBytes ? formatBytes(hostMeta.memTotalBytes) : null,
          selectedServer.mode,
        ]
          .filter(Boolean)
          .join(" · ")
      : "读取主机状态";

  return (
    <div className="shell">
      <aside className="sidebar">
        <a
          className="brand"
          href="#fleet"
          onClick={(event) => {
            event.preventDefault();
            openFleet();
          }}
        >
          <span className="brand-mark" aria-hidden="true">
            <span /><span /><span />
          </span>
          <span>
            <span className="wordmark">ivitals</span>
            <span className="brand-sub">Fleet telemetry</span>
          </span>
        </a>

        <nav className="side-nav" aria-label="Global">
          <button
            type="button"
            className={`nav-item ${isFleet ? "active" : ""}`}
            onClick={openFleet}
            aria-current={isFleet ? "page" : undefined}
          >
            <LayoutDashboard size={16} strokeWidth={1.5} aria-hidden="true" />
            <span>总览</span>
          </button>
        </nav>

        <div className="side-block">
          <div className="side-label">选择主机</div>
          <div className="host-list">
            {hostRows.map((host) => (
              <button
                type="button"
                className={`host-row ${!isFleet && host.alias === selected ? "active" : ""}`}
                key={host.alias}
                onClick={() => openHost(host.alias, route.tab)}
                aria-current={!isFleet && host.alias === selected ? "true" : undefined}
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
            <span>只读 · 手动刷新</span>
          </div>
          <div className="side-path mono" title={meta.dataDir}>
            <Database size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>…/{meta.dataDir.split("/").slice(-2).join("/")}</span>
          </div>
          <div
            className="mono tiny"
            title="按本地日期统计，来自 data/ 下的 UTC 分区"
          >
            数据 {meta.dates.length} 天
          </div>
        </div>
      </aside>

      <main className="main">
        <header className="page-header">
          <div>
            {!isFleet && (
              <button type="button" className="breadcrumb" onClick={openFleet}>
                <LayoutDashboard size={13} strokeWidth={1.6} aria-hidden="true" />
                总览
                <span className="breadcrumb-sep" aria-hidden="true">/</span>
                <span className="breadcrumb-current">{selectedServer?.alias ?? "主机"}</span>
              </button>
            )}
            <h1>{title}</h1>
            <p className="lede">{lede}</p>
          </div>
          <div className="header-actions">
            <div className="mono updated" title="仅手动刷新，不会自动重扫">
              <span>{formatDateTime(generated)}</span>
              <span className="muted">手动刷新</span>
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

        {!isFleet && (
          <div className="host-tabs" role="tablist" aria-label="主机视图">
            {HOST_TABS.map((tab) => {
              const Icon =
                tab === "overview"
                  ? Gauge
                  : tab === "telemetry"
                    ? Timer
                    : tab === "cores"
                      ? Grid3X3
                      : ListTree;
              return (
                <button
                  type="button"
                  role="tab"
                  key={tab}
                  className={`host-tab ${route.tab === tab ? "active" : ""}`}
                  aria-selected={route.tab === tab}
                  onClick={() => openHost(selected, tab)}
                >
                  <Icon size={14} strokeWidth={1.5} aria-hidden="true" />
                  {TAB_LABEL[tab]}
                </button>
              );
            })}
          </div>
        )}

        {meta.warnings.length > 0 && !isFleet && (
          <div className="notice warn">
            <strong>配置提示</strong>
            <ul>
              {meta.warnings.map((warning, index) => (
                <li key={`${index}-${warning}`}>{warning}</li>
              ))}
            </ul>
          </div>
        )}

        {(metaError || overviewError) && (
          <div className="notice error">
            <strong>看板服务未连接或读取失败</strong>
            <p>{metaError ?? overviewError}</p>

            <button className="ghost" onClick={refresh} disabled={busy}>
              立即重试
            </button>
          </div>
        )}

        {isFleet && (
          <FleetPanel
            overview={overview}
            meta={meta}
            loading={!overview && !overviewError}
            reloadToken={nonce}
            onRetry={refresh}
            onSelect={(alias) => openHost(alias, "overview")}
          />
        )}

        {!isFleet && route.tab === "overview" && (
          <OverviewPanel
            server={selectedServer}
            loading={!overview && !overviewError}
            error={null}
          />
        )}
        {!isFleet && route.tab === "telemetry" && (
          <SeriesPanel meta={meta} server={selected} reloadToken={nonce} />
        )}
        {!isFleet && route.tab === "cores" && (
          <CorePanel meta={meta} server={selected} reloadToken={nonce} />
        )}
        {!isFleet && route.tab === "processes" && (
          <ProcessPanel meta={meta} server={selected} reloadToken={nonce} />
        )}

        <footer className="page-footer">
          <span>ivitals · 数据目录只读</span>
          <span className="mono">{meta.configPath}</span>
        </footer>
      </main>
    </div>
  );
}
