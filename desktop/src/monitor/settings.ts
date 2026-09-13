import type { SeriesItem } from "../types";

export const MONITOR_SETTINGS_KEY = "hongguo.new-releases.settings.v1";
export function loadMonitorDays(storage: Storage): number | null {
  try {
    const saved = JSON.parse(storage.getItem(MONITOR_SETTINGS_KEY) || "null");
    return saved?.version === 1 && Number.isInteger(saved.days) && saved.days >= 1 && saved.days <= 30 ? saved.days : null;
  } catch { return null; }
}
export function inMonitorRange(item: SeriesItem, date: string, days: number | null) {
  if (days === null) return true;
  const timestamp = item.onlineTime;
  const end = Date.parse(`${date}T00:00:00+08:00`) / 1000 + 86400;
  return timestamp !== undefined && Number.isFinite(timestamp) && timestamp >= end - days * 86400 && timestamp < end;
}
