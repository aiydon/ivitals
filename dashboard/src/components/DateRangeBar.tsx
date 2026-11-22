import { useMemo, useState } from "react";
import { CalendarDays, ChevronDown } from "lucide-react";

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** `YYYY-MM-DD` → `11-22 周六`. Parsed as UTC because partitions are UTC. */
export function shortDay(ymd: string): string {
  const [year, month, day] = ymd.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  return `${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")} ${
    WEEKDAYS[date.getUTCDay()]
  }`;
}

interface Preset {
  key: string;
  label: string;
  from: string;
  to: string;
}

function buildPresets(dates: string[]): Preset[] {
  if (dates.length === 0) return [];
  const latest = dates[0];
  const take = (count: number) => dates[Math.min(count, dates.length) - 1] ?? latest;
  const presets: Preset[] = [{ key: "1d", label: "最新一天", from: latest, to: latest }];
  if (dates.length >= 3) {
    presets.push({ key: "3d", label: "最新 3 天", from: take(3), to: latest });
  }
  if (dates.length >= 7) {
    presets.push({ key: "7d", label: "最新 7 天", from: take(7), to: latest });
  }
  if (dates.length > 1) {
    presets.push({
      key: "all",
      label: "全部",
      from: dates[dates.length - 1],
      to: latest,
    });
  }
  return presets;
}

/**
 * Date scope control: preset buttons first, raw inputs only behind "自定义".
 * Keeping the common cases one click away is the whole point.
 */
export function DateRangeBar({
  dates,
  from,
  to,
  onChange,
  ariaPrefix = "日期",
}: {
  dates: string[];
  from: string;
  to: string;
  onChange: (from: string, to: string) => void;
  ariaPrefix?: string;
}) {
  const [custom, setCustom] = useState(false);
  const presets = useMemo(() => buildPresets(dates), [dates]);
  const active = presets.find((preset) => preset.from === from && preset.to === to);
  const earliest = dates[dates.length - 1] ?? "";
  const latest = dates[0] ?? "";


  if (dates.length === 0) {
    return <div className="date-bar muted small">没有可用的日期目录</div>;
  }

  return (
    <div className="date-bar">
      <div className="date-presets" role="group" aria-label={`${ariaPrefix}范围`}>
        {presets.map((preset) => (
          <button
            key={preset.key}
            type="button"
            className={`chip-button ${
              !custom && active?.key === preset.key ? "active" : ""
            }`}
            onClick={() => {
              setCustom(false);
              onChange(preset.from, preset.to);
            }}
            aria-pressed={!custom && active?.key === preset.key}
          >
            {preset.label}
          </button>
        ))}
        <button
          type="button"
          className={`chip-button ${custom ? "active" : ""}`}
          onClick={() => setCustom((value) => !value)}
          aria-expanded={custom}
        >
          <CalendarDays size={13} strokeWidth={1.6} aria-hidden="true" />
          自定义
          <ChevronDown
            size={12}
            strokeWidth={1.6}
            aria-hidden="true"
            className={custom ? "flip" : undefined}
          />
        </button>
      </div>

      <span className="date-summary mono">
        {from === to
          ? shortDay(from)
          : `${shortDay(from)} → ${shortDay(to)}`}
        <span className="muted">
          {" "}
          ·{" "}
          {Math.max(
            1,
            Math.round(
              (Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) /
                86_400_000,
            ) + 1,
          )}{" "}
          天
        </span>
      </span>

      {custom && (
        <div className="date-custom">
          <label>
            起
            <input
              aria-label={`${ariaPrefix}开始日期`}
              type="date"
              value={from}
              min={earliest}
              max={latest}
              onChange={(event) => {
                const value = event.target.value;
                if (!value) return;
                onChange(value, value > to ? value : to);
                if (value > to) onChange(value, value);
              }}
            />
          </label>
          <label>
            止
            <input
              aria-label={`${ariaPrefix}结束日期`}
              type="date"
              value={to}
              min={earliest}
              max={latest}
              onChange={(event) => {
                const value = event.target.value;
                if (!value) return;
                onChange(value < from ? value : from, value);
              }}
            />
          </label>
          <span className="muted tiny">按 UTC 日期分区</span>
        </div>
      )}
    </div>
  );
}

/** Single-day selector for snapshots, labelled with weekday and recency. */
export function DayPicker({
  dates,
  value,
  onChange,
  ariaLabel = "选择日期",
}: {
  dates: string[];
  value: string;
  onChange: (date: string) => void;
  ariaLabel?: string;
}) {
  if (dates.length === 0) {
    return <span className="muted small">没有可用日期</span>;
  }
  return (
    <label className="day-picker">
      日期
      <select
        aria-label={ariaLabel}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        {dates.map((date, index) => (
          <option key={date} value={date}>
            {shortDay(date)}
            {index === 0 ? " · 最新" : ""}
          </option>
        ))}
      </select>
    </label>
  );
}
