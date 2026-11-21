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

export const api = {
  meta: (signal?: AbortSignal) => apiGet<MetaResponse>("/api/meta", signal),
  overview: (signal?: AbortSignal) => apiGet<OverviewResponse>("/api/overview", signal),
  series: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<SeriesResponse>(`/api/series?${new URLSearchParams(params)}`, signal),
  cores: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<CoresResponse>(`/api/cores?${new URLSearchParams(params)}`, signal),
  processes: (params: Record<string, string>, signal?: AbortSignal) =>
    apiGet<ProcessesResponse>(`/api/processes?${new URLSearchParams(params)}`, signal),
};
