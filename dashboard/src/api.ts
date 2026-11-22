import type {
  CoresResponse,
  MetaResponse,
  OverviewResponse,
  ProcessesResponse,
  SeriesResponse,
} from "./shared/types";

export async function apiGet<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, {
    signal,
    headers: { accept: "application/json" },
  });
  const payload = (await res.json().catch(() => null)) as { error?: string } | null;
  if (!res.ok) {
    throw new Error(payload?.error ?? `请求失败 (HTTP ${res.status})`);
  }
  return payload as T;
}

/** Browser timezone offset east of UTC, in minutes (UTC+8 → 480). */
export function tzOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

function withTz(params: Record<string, string>): Record<string, string> {
  return { ...params, tzOffset: String(tzOffsetMinutes()) };
}

export const api = {
  meta: (signal?: AbortSignal) =>
    apiGet<MetaResponse>(`/api/meta?tzOffset=${tzOffsetMinutes()}`, signal),
  overview: (signal?: AbortSignal) => apiGet<OverviewResponse>("/api/overview", signal),
  series: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<SeriesResponse>(
      `/api/series?${new URLSearchParams(withTz(params))}`,
      signal,
    ),
  cores: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<CoresResponse>(`/api/cores?${new URLSearchParams(withTz(params))}`, signal),
  processes: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<ProcessesResponse>(
      `/api/processes?${new URLSearchParams(withTz(params))}`,
      signal,
    ),
};
