import { useEffect, useMemo, useRef, useState } from "react";
import { HeatmapChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  VisualMapComponent,
} from "echarts/components";
import * as echarts from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";

import { api } from "../api";
import { DateRangeBar } from "./DateRangeBar";
import { formatDuration, formatTime } from "../format";
import { chartTheme, useTheme } from "../theme";
import type { CoresResponse, MetaResponse } from "../shared/types";

echarts.use([HeatmapChart, GridComponent, TooltipComponent, VisualMapComponent, CanvasRenderer]);

const FRAME_CHOICES = ["120", "240", "480"] as const;

function internalGaps(data: CoresResponse) {
  const first = data.frames[0]?.t;
  const last = data.frames[data.frames.length - 1]?.t;
  if (first === undefined || last === undefined) return data.gaps;
  return data.gaps.filter((gap) => gap.start >= first && gap.end <= last + data.bucketSecs * 1000);
}

function heatmapOption(data: CoresResponse, theme: "dark" | "light"): Record<string, unknown> {
  const c = chartTheme(theme);
  const fg = c.fg;
  const muted = c.muted;
  const border = c.border;
  const tooltipBg = c.tooltipBg;
  const frameLabels = data.frames.map((frame) => formatTime(frame.t));
  const coreLabels = Array.from({ length: data.coreCount }, (_, index) => String(index));
  const points: [number, number, number][] = [];
  data.frames.forEach((frame, x) => {
    frame.cores.forEach((value, y) => {
      if (value !== null) points.push([x, y, value]);
    });
  });
  const coreLabelInterval = Math.max(0, Math.floor(data.coreCount / 14));

  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 58, right: 18, top: 46, bottom: 42 },
    tooltip: {
      position: "top",
      backgroundColor: tooltipBg,
      borderColor: border,
      textStyle: { color: fg, fontSize: 12 },
      formatter: (params: { value: [number, number, number] }) => {
        const [x, y, value] = params.value;
        return `core ${y}<br/>${frameLabels[x] ?? ""}<br/><b>${value.toFixed(1)}%</b>`;
      },
    },
    visualMap: {
      type: "piecewise",
      orient: "horizontal",
      left: "center",
      top: 4,
      itemWidth: 14,
      itemHeight: 12,
      itemGap: 12,
      textStyle: { color: muted, fontSize: 10 },
      pieces: [
        { min: 0, max: 5, label: "0–5", color: c.heatmap[0] },
        { min: 5, max: 15, label: "5–15", color: c.heatmap[1] },
        { min: 15, max: 35, label: "15–35", color: c.heatmap[2] },
        { min: 35, max: 60, label: "35–60", color: c.heatmap[3] },
        { min: 60, max: 85, label: "60–85", color: c.heatmap[4] },
        { min: 85, max: 100, label: "85–100", color: c.heatmap[5] },
      ],
    },
    xAxis: {
      type: "category",
      data: frameLabels,
      axisLabel: { color: muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: border } },
      splitLine: { show: false },
    },
    yAxis: {
      type: "category",
      data: coreLabels,
      axisLabel: { color: muted, interval: coreLabelInterval, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: border } },
      splitLine: { show: false },
    },
    series: [
      {
        name: "core busy %",
        type: "heatmap",
        data: points,
        itemStyle: {
          borderColor: theme === "light" ? "rgba(21,23,26,0.10)" : "rgba(11,12,14,0.42)",
          borderWidth: 0.5,
        },
        progressive: 4000,
        progressiveThreshold: 8000,
        emphasis: {
          itemStyle: { borderColor: fg, borderWidth: 1 },
        },
      },
    ],
  };
}

function CoreHeatmap({ data }: { data: CoresResponse | null }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const { theme } = useTheme();
  const option = useMemo(() => (data ? heatmapOption(data, theme) : null), [data, theme]);

  useEffect(() => {
    if (!host.current) return;
    const instance = echarts.init(host.current, undefined, { renderer: "canvas" });
    chart.current = instance;
    const observer = new ResizeObserver(() => instance.resize());
    observer.observe(host.current);
    return () => {
      observer.disconnect();
      instance.dispose();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    if (!chart.current) return;
    if (!option) {
      chart.current.clear();
      return;
    }
    chart.current.setOption(option, { lazyUpdate: true });
  }, [option]);

  return (
    <div className="chart-wrap">
      <div
        className="chart heatmap"
        ref={host}
        role="img"
        aria-label="每核心 CPU 利用率热力图"
      />
      {data && data.frames.length === 0 && (
        <div className="chart-empty empty-state">
          所选范围没有 `per_core_percent` 数据。请选择新 schema 采集的日期。
        </div>
      )}
    </div>
  );
}

export function CorePanel({
  meta,
  server,
  refreshTick,
}: {
  meta: MetaResponse;
  server: string;
  refreshTick: number;
}) {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [maxFrames, setMaxFrames] = useState<string>("240");
  const [data, setData] = useState<CoresResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [nonce, setNonce] = useState(0);
  const queryRef = useRef("");

  useEffect(() => {
    const latest = meta.dates[0] ?? "";
    setFrom((previous) => (previous && meta.dates.includes(previous) ? previous : latest));
    setTo((previous) => (previous && meta.dates.includes(previous) ? previous : latest));
  }, [meta]);

  useEffect(() => {
    if (!server || !from || !to) return;
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    const queryKey = `${server}|${from}|${to}|${maxFrames}`;
    if (queryRef.current !== queryKey) {
      queryRef.current = queryKey;
      setData(null);
    }
    api
      .cores({ server, from, to, maxFrames }, controller.signal)
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
  }, [server, from, to, maxFrames, nonce, refreshTick]);

  const gaps = data ? internalGaps(data) : [];

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>多核</h2>
          <p className="section-sub">每核心平均利用率</p>
        </div>
        <span className="section-meta mono">{server} · per-core heatmap</span>
      </div>

      <DateRangeBar
        dates={meta.dates}
        from={from}
        to={to}
        ariaPrefix="多核"
        onChange={(nextFrom, nextTo) => {
          setFrom(nextFrom);
          setTo(nextTo);
        }}
      />

      <div className="control-row">
        <label className="inline-field">
          帧数上限
          <select
            aria-label="多核帧数上限"
            value={maxFrames}
            onChange={(event) => setMaxFrames(event.target.value)}
          >
            {FRAME_CHOICES.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
        <button onClick={() => setNonce((value) => value + 1)} disabled={loading}>
          {loading ? "扫描中…" : "重新扫描"}
        </button>
      </div>

      {error && (
        <div className="notice error">
          <strong>多核读取失败</strong>
          <p>{error}</p>
        </div>
      )}

      {data && (
        <div className="series-stats">
          <span>{data.coreCount} 核</span>
          <span>{data.frames.length} 帧</span>
          <span>{data.samples.toLocaleString()} 条 per-core 样本</span>
          <span>{data.bucketSecs}s / 桶</span>
          <span>覆盖 {data.coveragePercent}%</span>
          <span>扫描 {data.scanMs}ms</span>
          {gaps.length > 0 && (
            <span className="warn-text">
              内部缺口 {gaps.length} 段 · 最大{" "}
              {formatDuration(
                Math.max(...gaps.map((gap) => gap.seconds)) * 1000,
              )}
            </span>
          )}
          {data.malformed > 0 && (
            <span className="warn-text">解析失败 {data.malformed.toLocaleString()} 行</span>
          )}
          {data.missingDates.length > 0 && (
            <span className="warn-text">缺失 {data.missingDates.length} 天</span>
          )}
        </div>
      )}

      {data?.warnings.map((warning, index) => (
        <div className="notice warn" key={`${index}-${warning}`}>
          {warning}
        </div>
      ))}

      <CoreHeatmap data={data} />
    </section>
  );
}
