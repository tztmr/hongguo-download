import type { NewReleaseType, SeriesItem } from "../types";

export const NEW_RELEASES_SEEN_KEY = "hongguo.new-releases.seen.v1";
export const NEW_RELEASES_ITEMS_KEY = "hongguo.new-releases.items.v1";

type SeenData = { version: 1; entries: Record<string, string[]> };

function read(storage: Storage): SeenData {
  try {
    const value = JSON.parse(storage.getItem(NEW_RELEASES_SEEN_KEY) || "");
    if (value?.version !== 1 || typeof value.entries !== "object" || !value.entries) throw new Error("invalid");
    return { version: 1, entries: value.entries };
  } catch {
    return { version: 1, entries: {} };
  }
}

function key(date: string, type: NewReleaseType, days?: number | null) {
  return `${date}|${type}${days == null ? "" : `|days=${days}`}`;
}

type ItemData = { version: 1; entries: Record<string, SeriesItem[]> };

function readItems(storage: Storage): ItemData {
  try {
    const value = JSON.parse(storage.getItem(NEW_RELEASES_ITEMS_KEY) || "");
    if (value?.version !== 1 || typeof value.entries !== "object" || !value.entries) throw new Error("invalid");
    return { version: 1, entries: value.entries };
  } catch {
    return { version: 1, entries: {} };
  }
}

function isSeriesItem(value: unknown): value is SeriesItem {
  if (!value || typeof value !== "object") return false;
  const item = value as Partial<SeriesItem>;
  return typeof item.bookId === "string" && typeof item.seriesId === "string" && typeof item.title === "string";
}

export function loadReleaseItems(storage: Storage, date: string, type: NewReleaseType, days?: number | null) {
  const values = readItems(storage).entries[key(date, type, days)];
  return Array.isArray(values) ? values.filter(isSeriesItem) : [];
}

export function saveReleaseItems(
  storage: Storage,
  date: string,
  type: NewReleaseType,
  items: SeriesItem[],
  days?: number | null,
) {
  const data = readItems(storage);
  const entries: Record<string, SeriesItem[]> = {};
  for (const [entryKey, values] of Object.entries(data.entries)) {
    if (entryKey.startsWith(`${date}|`) && Array.isArray(values)) entries[entryKey] = values.filter(isSeriesItem);
  }
  entries[key(date, type, days)] = items.filter(isSeriesItem);
  storage.setItem(NEW_RELEASES_ITEMS_KEY, JSON.stringify({ version: 1, entries } satisfies ItemData));
}

export function loadSeenReleases(storage: Storage, date: string, type: NewReleaseType, days?: number | null) {
  const values = read(storage).entries[key(date, type, days)];
  return new Set(Array.isArray(values) ? values.filter((item): item is string => typeof item === "string") : []);
}

export function markSeenReleases(storage: Storage, date: string, type: NewReleaseType, ids: string[], days?: number | null) {
  const data = read(storage);
  const currentKey = key(date, type, days);
  const existing = Array.isArray(data.entries[currentKey]) ? data.entries[currentKey] : [];
  const entries: Record<string, string[]> = {};
  for (const [entryKey, values] of Object.entries(data.entries)) {
    if (entryKey.startsWith(`${date}|`) && Array.isArray(values)) entries[entryKey] = values;
  }
  entries[currentKey] = [...new Set([...existing, ...ids])];
  storage.setItem(NEW_RELEASES_SEEN_KEY, JSON.stringify({ version: 1, entries } satisfies SeenData));
}
