import type { SeriesItem } from "./types";

export function seriesTypeLabel(item: SeriesItem): string {
  if (item.releaseType === "ai_playlet") return "AI剧";
  if (item.releaseType === "comic_series_rank" || [2, 1004].includes(item.contentTypeCode)) return "漫剧";
  if (item.releaseType === "playlet" || item.contentTypeCode === 1) return "真人剧";
  return "类型未知";
}

export function metricLabel(value?: number): string {
  if (value === undefined || !Number.isFinite(value) || value < 0) return "—";
  if (value >= 100_000_000) return `${Number((value / 100_000_000).toFixed(1))}亿`;
  if (value >= 10_000) return `${Number((value / 10_000).toFixed(1))}万`;
  return String(value);
}
