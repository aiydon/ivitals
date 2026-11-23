import { useEffect, useRef, useState } from "react";
import { HeatmapChart, LineChart } from "echarts/charts";
import {
  DataZoomComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  VisualMapComponent,
} from "echarts/components";
import * as echarts from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";

import { api } from "../api";
import { DateRangeBar } from "./DateRangeBar";
import {
  formatBytes,
  formatBytesCompact,
  formatDateTime,
  formatDuration,
  formatPercent,
  formatTime,
} from "../format";
import { chartTheme, useTheme } from "../theme";
import type { GpuTrack, MetaResponse, SeriesResponse } from "../shared/types";

echarts.use([
  LineChart,
  HeatmapChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  VisualMapComponent,
  CanvasRenderer,
]);

const MAX_POINTS = ["600", "1200", "2400"] as const;

type NumericField =
  | "cpu"
  | "mem"
  | "iowait"
  | "gpuUtil"
  | "gpuMem"
  | "gpuTemp"
  | "gpuPower"
  | "diskRead"
  | "diskWrite"
  | "fsUsedMax";

function gapSeries(
  data: SeriesResponse,
  getter: (point: SeriesResponse["points"][number]) => number | null,
): Array<[number, number | null]> {
  const bucketMs = data.bucketSecs * 1000;
  const out: Array<[number, number | null]> = [];
  let previous: number | null = null;
  for (const point of data.points) {
    const value = getter(point);
    if (previous !== null && point.t - previous > bucketMs * 1.8) {
      out.push([previous + bucketMs, null]);
    }
    out.push([point.t, value]);
    previous = point.t;
  }
  return out;
}

function gapAware(
  data: SeriesResponse,
  field: NumericField,
): Array<[number, number | null]> {
  return gapSeries(data, (point) => point[field]);
}

function internalGaps(data: SeriesResponse) {
  const first = data.points[0]?.t;
  const last = data.points[data.points.length - 1]?.t;
  if (first === undefined || last === undefined) return data.gaps;
  return data.gaps.filter((gap) => gap.start >= first && gap.end <= last + data.bucketSecs * 1000);
}

function chartOption(data: SeriesResponse, theme: "dark" | "light"): Record<string, unknown> {
  const c = chartTheme(theme);
  const fg = c.fg;
  const muted = c.muted;
  const border = c.border;
  const tooltipBg = c.tooltipBg;
  const hasIowait = data.points.some((point) => point.iowait !== null);
  const hasMemory = data.points.some((point) => point.mem !== null);
  const hasGpu = data.points.some((point) => point.gpuUtil !== null);
  const observedMax = data.points.reduce(
    (max, point) => Math.max(max, point.cpu ?? 0, point.mem ?? 0),
    0,
  );
  const axisMax = observedMax <= 10 ? 10 : observedMax <= 20 ? 20 : observedMax <= 50 ? 50 : 100;

  const series: Record<string, unknown>[] = [
    {
      name: "CPU %",
      type: "line",
      showSymbol: false,
      symbol: "circle",
      symbolSize: 6,
      lineStyle: { width: 1.6, color: c.cpu },
      itemStyle: { color: c.cpu },
      emphasis: { focus: "series" },
      connectNulls: false,
      data: gapAware(data, "cpu"),
    },
  ];
  if (hasMemory) {
    series.push({
      name: "内存 %",
      type: "line",
      showSymbol: false,
      symbol: "circle",
      symbolSize: 6,
      lineStyle: { width: 1.5, color: c.mem },
      itemStyle: { color: c.mem },
      emphasis: { focus: "series" },
      connectNulls: false,
      data: gapAware(data, "mem"),
    });
  }
  if (hasIowait) {
    series.push({
      name: "iowait %",
      type: "line",
      showSymbol: false,
      symbol: "circle",
      symbolSize: 6,
      lineStyle: { width: 1.4, color: c.io },
      itemStyle: { color: c.io },
      emphasis: { focus: "series" },
      connectNulls: false,
      data: gapAware(data, "iowait"),
    });
  }
  if (hasGpu) {
    series.push({
      name: "GPU util %",
      type: "line",
      yAxisIndex: 1,
      showSymbol: false,
      symbol: "circle",
      symbolSize: 6,
      lineStyle: { width: 1.5, color: c.gpu },
      itemStyle: { color: c.gpu },
      emphasis: { focus: "series" },
      connectNulls: false,
      data: gapAware(data, "gpuUtil"),
    });
    series.push({
      name: "GPU mem %",
      type: "line",
      yAxisIndex: 1,
      showSymbol: false,
      symbol: "circle",
      symbolSize: 6,
      lineStyle: { width: 1.3, color: c.gpuMem, type: "dashed" },
      itemStyle: { color: c.gpuMem },
      emphasis: { focus: "series" },
      connectNulls: false,
      data: gapAware(data, "gpuMem"),
    });
  }

  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 56, right: 58, top: 36, bottom: 68 },
    legend: {
      type: "scroll",
      top: 0,
      textStyle: { color: muted, fontFamily: "IBM Plex Sans" },
      icon: "roundRect",
      itemWidth: 14,
      pageTextStyle: { color: muted },
    },
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "cross", label: { backgroundColor: border } },
      backgroundColor: tooltipBg,
      borderColor: c.tooltipBorder,
      textStyle: { color: fg, fontSize: 12 },
      formatter: (params: Array<{ dataIndex: number }>) => {
        const point = data.points[params[0]?.dataIndex ?? 0];
        if (!point) return "";
        const rows = [
          `<div class="tt-time">${formatDateTime(point.t)}</div>`,
          `<div class="tt-row"><span>CPU 平均</span><b>${formatPercent(point.cpu)}</b></div>`,
          `<div class="tt-row"><span>CPU 峰值</span><b>${formatPercent(point.cpuMax)}</b></div>`,
          `<div class="tt-row"><span>内存平均</span><b>${formatPercent(point.mem)}</b></div>`,
          `<div class="tt-row"><span>iowait</span><b>${formatPercent(point.iowait, 2)}</b></div>`,
          `<div class="tt-row"><span>load1</span><b>${point.load1?.toFixed(2) ?? "—"}</b></div>`,
          `<div class="tt-row"><span>GPU util 平均</span><b>${formatPercent(point.gpuUtil)}</b></div>`,
          `<div class="tt-row"><span>GPU 显存平均</span><b>${formatPercent(point.gpuMem)}</b></div>`,
          `<div class="tt-row"><span>GPU 最高温</span><b>${point.gpuTemp?.toFixed(1) ?? "—"} °C</b></div>`,
          `<div class="tt-row"><span>GPU 总功耗</span><b>${point.gpuPower?.toFixed(0) ?? "—"} W</b></div>`,
          `<div class="tt-row tt-muted"><span>桶内样本</span><b>${point.samples}</b></div>`,
        ];
        return rows.join("");
      },
    },
    xAxis: {
      type: "time",
      axisLabel: { color: muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: border } },
      splitLine: { show: false },
    },
    yAxis: [
      {
        type: "value",
        min: 0,
        max: axisMax,
        axisLabel: { color: muted, formatter: "{value}%", fontFamily: "IBM Plex Mono" },
        splitLine: { lineStyle: { color: border, type: "dashed" } },
      },
      {
        type: "value",
        min: 0,
        max: 100,
        axisLabel: { color: muted, formatter: "{value}%", fontFamily: "IBM Plex Mono" },
        splitLine: { show: false },
      },
    ],
    dataZoom: [
      { type: "inside", throttle: 60 },
      {
        type: "slider",
        height: 18,
        bottom: 10,
        borderColor: border,
        backgroundColor: "transparent",
        fillerColor: theme === "light" ? "rgba(166,106,31,0.12)" : "rgba(224,178,104,0.12)",
        handleStyle: { color: theme === "light" ? "#a66a1f" : "#e0b268" },
        textStyle: { color: muted },
        labelFormatter: (_value: number, valueStr: string) => valueStr,
      },
    ],
    series,
  };
}

function gpuChartOption(data: SeriesResponse, theme: "dark" | "light"): Record<string, unknown> {
  const c = chartTheme(theme);
  const fg = c.fg;
  const muted = c.muted;
  const border = c.border;
  const powerMax = Math.max(
    100,
    ...data.points.map((point) => Math.ceil((point.gpuPower ?? 0) / 50) * 50),
  );
  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 52, right: 56, top: 32, bottom: 36 },
    legend: {
      type: "scroll",
      top: 0,
      textStyle: { color: muted, fontFamily: "IBM Plex Sans" },
      icon: "roundRect",
      itemWidth: 14,
      pageTextStyle: { color: muted },
    },
    tooltip: {
      trigger: "axis",
      backgroundColor: c.tooltipBg,
      borderColor: c.tooltipBorder,
      textStyle: { color: fg, fontSize: 12 },
      formatter: (params: Array<{ dataIndex: number }>) => {
        const point = data.points[params[0]?.dataIndex ?? 0];
        if (!point) return "";
        return [
          `<div class="tt-time">${formatDateTime(point.t)}</div>`,
          `<div class="tt-row"><span>GPU 最高温</span><b>${point.gpuTemp?.toFixed(1) ?? "—"} °C</b></div>`,
          `<div class="tt-row"><span>GPU 总功耗</span><b>${point.gpuPower?.toFixed(0) ?? "—"} W</b></div>`,
          `<div class="tt-row"><span>GPU util 平均</span><b>${formatPercent(point.gpuUtil)}</b></div>`,
        ].join("");
      },
    },
    xAxis: {
      type: "time",
      axisLabel: { color: muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: border } },
      splitLine: { show: false },
    },
    yAxis: [
      {
        type: "value",
        min: 0,
        max: 100,
        axisLabel: { color: muted, fontFamily: "IBM Plex Mono" },
        splitLine: { lineStyle: { color: border, type: "dashed" } },
      },
      {
        type: "value",
        min: 0,
        max: powerMax,
        axisLabel: { color: muted, fontFamily: "IBM Plex Mono" },
        splitLine: { show: false },
      },
    ],
    series: [
      {
        name: "GPU 最高温",
        type: "line",
        yAxisIndex: 0,
        showSymbol: false,
        lineStyle: { width: 1.5, color: c.temp },
        itemStyle: { color: c.temp },
        connectNulls: false,
        data: gapAware(data, "gpuTemp"),
      },
      {
        name: "GPU 总功耗",
        type: "line",
        yAxisIndex: 1,
        showSymbol: false,
        lineStyle: { width: 1.4, color: c.io },
        itemStyle: { color: c.io },
        connectNulls: false,
        data: gapAware(data, "gpuPower"),
      },
    ],
  };
}

function GpuChart({ data }: { data: SeriesResponse }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const keyRef = useRef("");
  const { theme } = useTheme();
  const hasData = data.points.some(
    (point) => point.gpuTemp !== null || point.gpuPower !== null,
  );

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
    if (!hasData) {
      chart.current.clear();
      return;
    }
    chart.current.setOption(gpuChartOption(data, theme), { lazyUpdate: true });
  }, [data, theme, hasData]);

  if (!hasData) return null;
  return (
    <>
      <div className="chart-caption">GPU 温度 / 功耗</div>
      <div className="chart-wrap gpu-chart-wrap">
        <div className="chart gpu-chart" ref={host} role="img" aria-label="GPU 温度与功耗图" />
      </div>
    </>
  );
}

function gpuHeatmapOption(
  tracks: GpuTrack[],
  times: number[],
  theme: "dark" | "light",
): Record<string, unknown> {
  const c = chartTheme(theme);
  const labels = times.map((time) => formatTime(time));
  const points: [number, number, number][] = [];
  tracks.forEach((track, y) => {
    track.values.forEach((value, x) => {
      if (value !== null) points.push([x, y, value]);
    });
  });
  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: c.fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 64, right: 18, top: 44, bottom: 34 },
    tooltip: {
      backgroundColor: c.tooltipBg,
      borderColor: c.tooltipBorder,
      textStyle: { color: c.fg, fontSize: 12 },
      formatter: (params: { value: [number, number, number] }) => {
        const [x, y, value] = params.value;
        return `${tracks[y]?.name ?? `GPU ${y}`}<br/>${labels[x] ?? ""}<br/><b>显存 ${value.toFixed(1)}%</b>`;
      },
    },
    visualMap: {
      type: "piecewise",
      orient: "horizontal",
      left: "center",
      top: 4,
      itemWidth: 13,
      itemHeight: 11,
      itemGap: 10,
      textStyle: { color: c.muted, fontSize: 10 },
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
      data: labels,
      axisLabel: { color: c.muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: c.border } },
      splitLine: { show: false },
    },
    yAxis: {
      type: "category",
      inverse: true,
      data: tracks.map((track) => track.name),
      axisLabel: { color: c.muted, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: c.border } },
      splitLine: { show: false },
    },
    series: [
      {
        type: "heatmap",
        data: points,
        itemStyle: {
          borderColor: theme === "light" ? "rgba(21,23,26,0.1)" : "rgba(11,12,14,0.42)",
          borderWidth: 0.5,
        },
        progressive: 4000,
      },
    ],
  };
}

function GpuHeatmap({ tracks, times }: { tracks: GpuTrack[]; times: number[] }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const { theme } = useTheme();
  const enabled = (tracks?.length ?? 0) > 0 && times.length > 0;

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
    if (!enabled) {
      chart.current.clear();
      return;
    }
    chart.current.setOption(gpuHeatmapOption(tracks, times, theme), { lazyUpdate: true });
  }, [tracks, times, theme, enabled]);

  if (!enabled) return null;
  return (
    <>
      <div className="chart-caption">GPU 显存</div>
      <div className="chart-wrap gpu-chart-wrap">
        <div
          className="chart gpu-heatmap"
        ref={host}
        style={{ height: `${70 + tracks.length * 26}px` }}
          role="img"
          aria-label="GPU 显存利用率热力图"
        />
      </div>
    </>
  );
}

function memoryChartOption(
  data: SeriesResponse,
  theme: "dark" | "light",
): Record<string, unknown> {
  const c = chartTheme(theme);
  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: c.fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 64, right: 20, top: 32, bottom: 36 },
    legend: {
      type: "scroll",
      top: 0,
      textStyle: { color: c.muted, fontFamily: "IBM Plex Sans" },
      icon: "roundRect",
      itemWidth: 14,
      pageTextStyle: { color: c.muted },
    },
    tooltip: {
      trigger: "axis",
      backgroundColor: c.tooltipBg,
      borderColor: c.tooltipBorder,
      textStyle: { color: c.fg, fontSize: 12 },
      formatter: (params: Array<{ dataIndex: number }>) => {
        const point = data.points[params[0]?.dataIndex ?? 0];
        if (!point) return "";
        const used = point.memUsedBytes;
        const cache = point.memCacheBytes;
        const apps = used !== null && cache !== null ? Math.max(0, used - cache) : null;
        return [
          `<div class="tt-time">${formatDateTime(point.t)}</div>`,
          `<div class="tt-row"><span>应用</span><b>${formatBytes(apps)}</b></div>`,
          `<div class="tt-row"><span>缓存</span><b>${formatBytes(cache)}</b></div>`,
          `<div class="tt-row"><span>Swap</span><b>${formatBytes(point.memSwapBytes)}</b></div>`,
          `<div class="tt-row"><span>总内存</span><b>${formatBytes(point.memTotalBytes)}</b></div>`,
        ].join("");
      },
    },
    xAxis: {
      type: "time",
      axisLabel: { color: c.muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: c.border } },
      splitLine: { show: false },
    },
    yAxis: {
      type: "value",
      min: 0,
      axisLabel: {
        color: c.muted,
        fontFamily: "IBM Plex Mono",
        formatter: (value: number) => formatBytesCompact(value),
      },
      splitLine: { lineStyle: { color: c.border, type: "dashed" } },
    },
    series: [
      {
        name: "应用",
        type: "line",
        stack: "mem",
        showSymbol: false,
        areaStyle: { opacity: 0.28 },
        lineStyle: { width: 1 },
        itemStyle: { color: c.cpu },
        connectNulls: false,
        data: gapSeries(data, (point) =>
          point.memUsedBytes !== null && point.memCacheBytes !== null
            ? Math.max(0, point.memUsedBytes - point.memCacheBytes)
            : null,
        ),
      },
      {
        name: "缓存",
        type: "line",
        stack: "mem",
        showSymbol: false,
        areaStyle: { opacity: 0.28 },
        lineStyle: { width: 1 },
        itemStyle: { color: c.mem },
        connectNulls: false,
        data: gapSeries(data, (point) => point.memCacheBytes),
      },
      {
        name: "Swap",
        type: "line",
        showSymbol: false,
        lineStyle: { width: 1.2, color: c.warn, type: "dashed" },
        itemStyle: { color: c.warn },
        connectNulls: false,
        data: gapSeries(data, (point) => point.memSwapBytes),
      },
    ],
  };
}

function MemoryChart({ data }: { data: SeriesResponse }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const { theme } = useTheme();
  const enabled = data.points.some((point) => point.memUsedBytes !== null);

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
    if (!enabled) {
      chart.current.clear();
      return;
    }
    chart.current.setOption(memoryChartOption(data, theme), { lazyUpdate: true });
  }, [data, theme, enabled]);

  if (!enabled) return null;
  return (
    <>
      <div className="chart-caption">内存构成</div>
      <div className="chart-wrap gpu-chart-wrap">
        <div
          className="chart mem-chart"
        ref={host}
          role="img"
          aria-label="内存构成图"
        />
      </div>
    </>
  );
}

function diskChartOption(data: SeriesResponse, theme: "dark" | "light"): Record<string, unknown> {
  const c = chartTheme(theme);
  return {
    animation: false,
    backgroundColor: "transparent",
    textStyle: { color: c.fg, fontFamily: "IBM Plex Sans" },
    grid: { left: 62, right: 52, top: 32, bottom: 36 },
    legend: {
      type: "scroll",
      top: 0,
      textStyle: { color: c.muted, fontFamily: "IBM Plex Sans" },
      icon: "roundRect",
      itemWidth: 14,
      pageTextStyle: { color: c.muted },
    },
    tooltip: {
      trigger: "axis",
      backgroundColor: c.tooltipBg,
      borderColor: c.tooltipBorder,
      textStyle: { color: c.fg, fontSize: 12 },
      formatter: (params: Array<{ dataIndex: number }>) => {
        const point = data.points[params[0]?.dataIndex ?? 0];
        if (!point) return "";
        return [
          `<div class="tt-time">${formatDateTime(point.t)}</div>`,
          `<div class="tt-row"><span>读</span><b>${point.diskRead === null ? "—" : `${formatBytes(point.diskRead)}/s`}</b></div>`,
          `<div class="tt-row"><span>写</span><b>${point.diskWrite === null ? "—" : `${formatBytes(point.diskWrite)}/s`}</b></div>`,
          `<div class="tt-row"><span>最高盘使用率</span><b>${formatPercent(point.fsUsedMax)}</b></div>`,
        ].join("");
      },
    },
    xAxis: {
      type: "time",
      axisLabel: { color: c.muted, hideOverlap: true, fontFamily: "IBM Plex Mono" },
      axisLine: { lineStyle: { color: c.border } },
      splitLine: { show: false },
    },
    yAxis: [
      {
        type: "value",
        min: 0,
        axisLabel: {
          color: c.muted,
          fontFamily: "IBM Plex Mono",
          formatter: (value: number) =>
            value >= 1e9
              ? `${(value / 1e9).toFixed(1)} GB/s`
              : value >= 1e6
                ? `${(value / 1e6).toFixed(0)} MB/s`
                : `${(value / 1e3).toFixed(0)} KB/s`,
        },
        splitLine: { lineStyle: { color: c.border, type: "dashed" } },
      },
      {
        type: "value",
        min: 0,
        max: 100,
        axisLabel: { color: c.muted, formatter: "{value}%", fontFamily: "IBM Plex Mono" },
        splitLine: { show: false },
      },
    ],
    series: [
      {
        name: "读",
        type: "line",
        yAxisIndex: 0,
        showSymbol: false,
        lineStyle: { width: 1.4, color: c.cpu },
        itemStyle: { color: c.cpu },
        connectNulls: false,
        data: gapAware(data, "diskRead"),
      },
      {
        name: "写",
        type: "line",
        yAxisIndex: 0,
        showSymbol: false,
        lineStyle: { width: 1.4, color: c.mem },
        itemStyle: { color: c.mem },
        connectNulls: false,
        data: gapAware(data, "diskWrite"),
      },
      {
        name: "最高盘使用率",
        type: "line",
        yAxisIndex: 1,
        showSymbol: false,
        lineStyle: { width: 1.2, color: c.warn, type: "dashed" },
        itemStyle: { color: c.warn },
        connectNulls: false,
        data: gapAware(data, "fsUsedMax"),
      },
    ],
  };
}

function DiskChart({ data }: { data: SeriesResponse }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const { theme } = useTheme();
  const hasData = data.points.some(
    (point) =>
      point.diskRead !== null || point.diskWrite !== null || point.fsUsedMax !== null,
  );

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
    if (!hasData) {
      chart.current.clear();
      return;
    }
    chart.current.setOption(diskChartOption(data, theme), { lazyUpdate: true });
  }, [data, theme, hasData]);

  if (!hasData) return null;
  return (
    <>
      <div className="chart-caption">磁盘吞吐 / 使用率</div>
      <div className="chart-wrap gpu-chart-wrap">
        <div className="chart disk-chart" ref={host} role="img" aria-label="磁盘吞吐与使用率图" />
      </div>
    </>
  );
}

function SeriesChart({ data }: { data: SeriesResponse | null }) {
  const host = useRef<HTMLDivElement | null>(null);
  const chart = useRef<echarts.EChartsType | null>(null);
  const keyRef = useRef("");
  const { theme } = useTheme();

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
    if (!data) {
      chart.current.clear();
      keyRef.current = "";
      return;
    }
    const key = `${data.server}|${data.from}|${data.to}`;
    const reset = keyRef.current !== key;
    keyRef.current = key;
    chart.current.setOption(chartOption(data, theme), {
      notMerge: reset,
      lazyUpdate: true,
    });
  }, [data, theme]);

  return (
    <div className="chart-wrap">
      <div
        className="chart"
        ref={host}
        role="img"
        aria-label="CPU、内存与 iowait 时间序列图"
      />
      {data && data.points.length === 0 && (
        <div className="chart-empty empty-state">所选范围没有可绘制的数据</div>
      )}
    </div>
  );
}

export function SeriesPanel({
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
  const [maxPoints, setMaxPoints] = useState<string>("1200");
  const [data, setData] = useState<SeriesResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [tab, setTab] = useState<"mem" | "disk" | "gpuMem" | "gpuTemp">("mem");
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
    const queryKey = `${server}|${from}|${to}|${maxPoints}`;
    if (queryRef.current !== queryKey) {
      queryRef.current = queryKey;
      setData(null);
    }
    api
      .series({ server, from, to, maxPoints }, controller.signal)
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
  }, [server, from, to, maxPoints, nonce, refreshTick]);

  const gaps = data ? internalGaps(data) : [];
  const points = data?.points ?? [];
  const tabs = [
    { key: "mem" as const, label: "内存", enabled: points.some((p) => p.memUsedBytes !== null) },
    { key: "disk" as const, label: "磁盘", enabled: points.some((p) => p.diskRead !== null) },
    {
      key: "gpuMem" as const,
      label: "GPU 显存",
      enabled: (data?.gpuTracks?.length ?? 0) > 0,
    },
    {
      key: "gpuTemp" as const,
      label: "GPU 温度 / 功耗",
      enabled: points.some((p) => p.gpuTemp !== null),
    },
  ].filter((item) => item.enabled);

  useEffect(() => {
    if (tabs.length > 0 && !tabs.some((item) => item.key === tab)) {
      setTab(tabs[0].key);
    }
  }, [tabs, tab]);

  return (
    <section className="panel">
      <div className="section-head">
        <div>
          <h2>时序</h2>
          <p className="section-sub">所选范围的时间序列</p>
        </div>
        <span className="section-meta mono">{server} · CPU · 内存 · GPU</span>
      </div>

      <DateRangeBar
        dates={meta.dates}
        from={from}
        to={to}
        ariaPrefix="时序"
        onChange={(nextFrom, nextTo) => {
          setFrom(nextFrom);
          setTo(nextTo);
        }}
      />

      <div className="control-row">
        <label className="inline-field">
          点数上限
          <select
            aria-label="点数上限"
            value={maxPoints}
            onChange={(event) => setMaxPoints(event.target.value)}
          >
            {MAX_POINTS.map((value) => (
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
          <strong>时序读取失败</strong>
          <p>{error}</p>
        </div>
      )}

      {data && (
        <div className="series-stats">
          <span>{data.points.length.toLocaleString()} 个点</span>
          <span>{data.samples.toLocaleString()} 条样本</span>
          <span>{data.bucketSecs}s / 桶</span>
          <span>覆盖 {data.coveragePercent}%</span>
          <span>扫描 {data.scanMs}ms</span>
          {gaps.length > 0 && (
            <span className="warn-text">
              内部缺口 {gaps.length} 段 · 最大{" "}
              {formatDuration(Math.max(...gaps.map((gap) => gap.seconds)) * 1000)}
            </span>
          )}
          {data.gpuCount > 0 && <span>{data.gpuCount} GPU</span>}
          {(() => {
            const peak = data.points.reduce(
              (max, point) =>
                point.fsUsedMax === null ? max : Math.max(max, point.fsUsedMax),
              0,
            );
            return peak > 0 ? <span>盘峰值 {formatPercent(peak)}</span> : null;
          })()}
          {data.malformed > 0 && (
            <span className="warn-text">解析失败 {data.malformed.toLocaleString()} 行</span>
          )}
          {data.missingDates.length > 0 && (
            <span className="warn-text">缺失 {data.missingDates.length} 天</span>
          )}
        </div>
      )}

      {data && gaps.length > 0 && (
        <div className="notice warn">
          <strong>{gaps.length} 段采集中断</strong>
          <p>
            最长 {formatDuration(Math.max(...gaps.map((gap) => gap.seconds)) * 1000)}
            ，折线在缺口处断开。
          </p>
        </div>
      )}
      {data?.truncated && (
        <div className="banner warn">
          <strong>结果被截断</strong>
          <p>请缩小日期范围。</p>
        </div>
      )}
      {data && data.warnings.length > 0 && (
        <div className="banner warn">
          <strong>扫描提示</strong>
          <ul>
            {data.warnings.map((warning, index) => (
              <li key={`${index}-${warning}`}>{warning}</li>
            ))}
          </ul>
        </div>
      )}
      {data && data.missingDates.length > 0 && (
        <p className="muted small">
          缺失日期（该服务器无 metrics 文件）: {data.missingDates.slice(0, 10).join(", ")}
          {data.missingDates.length > 10 ? " …" : ""}
        </p>
      )}

      <SeriesChart data={data} />
      {data && tabs.length > 0 && (
        <div className="chart-tabs" role="tablist" aria-label="详细图表">
          {tabs.map((item) => (
            <button
              key={item.key}
              type="button"
              role="tab"
              className={`chart-tab ${tab === item.key ? "active" : ""}`}
              aria-selected={tab === item.key}
              onClick={() => setTab(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
      {data && tab === "mem" && <MemoryChart data={data} />}
      {data && tab === "disk" && <DiskChart data={data} />}
      {data && tab === "gpuMem" && (
        <GpuHeatmap
          tracks={data.gpuTracks ?? []}
          times={data.points.map((point) => point.t)}
        />
      )}
      {data && tab === "gpuTemp" && <GpuChart data={data} />}
    </section>
  );
}
