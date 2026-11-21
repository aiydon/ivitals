import { Fragment, useEffect, useState } from "react";

import { api } from "../api";
import { formatBytes, formatDateTime, formatDuration, formatPercent, relativeTime } from "../format";
import type { MetaResponse, ProcessesResponse } from "../shared/types";

const TOP_CHOICES = ["10", "20", "50", "100"] as const;

function StateBadge({ state }: { state: string | null }) {
  if (!state) return <span className="muted">—</span>;
  const label: Record<string, string> = {
    R: "运行",
    S: "睡眠",
    D: "不可中断",
    Z: "僵尸",
    T: "停止",
    I: "空闲",
  };
  return (
    <span className={`state st-${state}`} title={label[state] ?? state}>
      {state}
    </span>
  );
}

export function ProcessPanel({
  meta,
  server,
  refreshTick,
}: {
  meta: MetaResponse;
  server: string;
  refreshTick: number;
}) {
  const [date, setDate] = useState("");
  const [top, setTop] = useState("20");
  const [data, setData] = useState<ProcessesResponse | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    const latest = meta.dates[0] ?? "";
    setDate((previous) =>
      previous && meta.dates.includes(previous) ? previous : latest,
    );
  }, [meta]);

  useEffect(() => {
    if (!server || !date) return;
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    setData(null);
    api
      .processes({ server, date, top }, controller.signal)
      .then((result) => setData(result))
      .catch((cause) => {
        if (!controller.signal.aborted) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [server, date, top, nonce, refreshTick]);

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>进程</h2>
          <p className="section-sub">当天最后一个完整快照，按 CPU 降序</p>
        </div>
        <span className="section-meta mono">{server} · top {top}</span>
      </div>

      <div className="controls">
        <label>
          日期 (UTC)
          <input
            aria-label="进程日期"
            type="date"
            value={date}
            min={meta.dates[meta.dates.length - 1]}
            max={meta.dates[0]}
            onChange={(event) => setDate(event.target.value)}
          />
        </label>
        <label>
          Top N
          <select aria-label="Top N" value={top} onChange={(event) => setTop(event.target.value)}>
            {TOP_CHOICES.map((value) => (
              <option key={value} value={value}>
                Top {value}
              </option>
            ))}
          </select>
        </label>
        <button onClick={() => setNonce((value) => value + 1)} disabled={loading}>
          {loading ? "读取中…" : "重新读取"}
        </button>
      </div>

      {error && (
        <div className="notice error">
          <strong>进程快照读取失败</strong>
          <p>{error}</p>
        </div>
      )}

      {data && (
        <>
          <div className="series-stats">
            <span>快照时间 {formatDateTime(data.tsMs)}</span>
            <span>
              进程总数 {data.totalProcs === null ? "—" : data.totalProcs.toLocaleString()}
            </span>
            <span>
              线程总数 {data.totalThreads === null ? "—" : data.totalThreads.toLocaleString()}
            </span>
            <span>
              显示 {data.processes.length} / Top {data.topN}
            </span>
            <span>快照 {relativeTime(data.tsMs)}</span>
          </div>
          {Date.now() - data.tsMs > Math.max(meta.processIntervalSecs * 3000, 300_000) && (
            <div className="notice warn">
              最后一个进程快照是 {relativeTime(data.tsMs)}，采集可能中断；上方时间范围仍会显示历史快照。
            </div>
          )}
          {data.stateCounts && (
            <div className="state-chips">
              {Object.entries(data.stateCounts)
                .sort((a, b) => b[1] - a[1])
                .slice(0, 12)
                .map(([state, count]) => (
                  <span className="chip" key={state} title={`进程状态 ${state}`}>
                    <span className="mono">{state}</span>
                    <b>{count.toLocaleString()}</b>
                  </span>
                ))}
            </div>
          )}
          {data.processes.length === 0 ? (
            <div className="empty-state">该快照没有可解析的进程行。</div>
          ) : (
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th className="num">PID</th>
                    <th>用户</th>
                    <th>进程 / 命令</th>
                    <th className="num">CPU %</th>
                    <th className="num">内存 %</th>
                    <th className="num">RSS</th>
                    <th className="num">线程</th>
                    <th className="num">累计 CPU</th>
                    <th className="center">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {data.processes.map((row) => {
                    const open = expanded === row.pid;
                    return (
                      <Fragment key={`${row.pid}-${row.name}`}>
                        <tr
                          className={`proc-row ${open ? "open" : ""}`}
                          onClick={() => setExpanded(open ? null : row.pid)}
                          aria-expanded={open}
                        >
                          <td className="num mono">{row.pid}</td>
                          <td className="proc-user">{row.user ?? "—"}</td>
                          <td className="proc-name">
                            <span className="proc-title">
                              <strong>{row.name}</strong>
                              <span className="proc-args mono" title={row.cmdline ?? undefined}>
                                {row.cmdline ?? "—"}
                              </span>
                            </span>
                          </td>
                          <td className="num">{formatPercent(row.cpuPercent)}</td>
                          <td className="num">{formatPercent(row.memPercent)}</td>
                          <td className="num">{formatBytes(row.rssBytes)}</td>
                          <td className="num">{row.threads ?? "—"}</td>
                          <td className="num">{formatDuration(row.cumCpuMs)}</td>
                          <td className="center">
                            <StateBadge state={row.state} />
                          </td>
                        </tr>
                        {open && (
                          <tr className="proc-detail-row">
                            <td colSpan={9}>
                              <div className="proc-detail">
                                <div><span>用户</span><strong>{row.user ?? "—"}{row.uid === null ? "" : ` (uid ${row.uid})`}</strong></div>
                                <div><span>父进程</span><strong>{row.ppid === null ? "—" : `${row.ppid} ${row.parentName ?? ""}`}</strong></div>
                                <div><span>工作目录</span><strong className="mono">{row.cwd ?? "—"}</strong></div>
                                <div><span>可执行文件</span><strong className="mono">{row.exe ?? "—"}</strong></div>
                                <div className="wide"><span>命令行</span><strong className="mono">{row.cmdline ?? "—"}</strong></div>
                              </div>
                            </td>
                          </tr>
                        )}
                      </Fragment>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}

          {data.gpuProcesses.length > 0 && (
            <div className="gpu-processes">
              <h3 className="subhead">GPU 进程</h3>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>GPU</th>
                      <th className="num">PID</th>
                      <th>进程</th>
                      <th className="num">显存</th>
                      <th>类型</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.gpuProcesses.map((row) => (
                      <tr key={`${row.gpuUuid ?? row.gpuIndex}-${row.pid}-${row.kind}`}>
                        <td className="mono" title={row.gpuUuid ?? undefined}>
                          {row.gpuUuid ? `${row.gpuUuid.slice(0, 12)}…` : `#${row.gpuIndex}`}
                        </td>
                        <td className="num mono">{row.pid}</td>
                        <td className="proc-name" title={row.name}>
                          {row.name}
                        </td>
                        <td className="num">
                          {row.memMb === null ? "—" : `${row.memMb.toLocaleString()} MiB`}
                        </td>
                        <td>{row.kind ?? "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </>
      )}

      {loading && !data && <div className="empty-state">正在读取进程快照…</div>}
    </section>
  );
}
