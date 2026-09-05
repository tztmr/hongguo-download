import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { NotificationAdapter } from "../notifications";
import type { NewReleasePage, NewReleaseType, SeriesItem } from "../types";
import { releaseCategoryNames, releaseCategoryOptions } from "./categories";
import {
  loadReleaseItems,
  loadSeenReleases,
  markSeenReleases,
  saveReleaseItems,
} from "./storage";

type MonitorApi = { fetchNewReleases(type: NewReleaseType, cursor?: string, limit?: number): Promise<NewReleasePage> };

export type NewReleaseMonitorOptions = {
  api: MonitorApi;
  storage: Storage;
  notifications: NotificationAdapter;
  enabled: boolean;
  notify: boolean;
  intervalMs?: number;
};

function shanghaiDate() {
  return new Intl.DateTimeFormat("en-CA", { timeZone: "Asia/Shanghai", year: "numeric", month: "2-digit", day: "2-digit" }).format(new Date());
}

function mergeItems(persisted: SeriesItem[], fresh: SeriesItem[]) {
  const merged = new Map<string, SeriesItem>();
  for (const item of persisted) merged.set(item.bookId, item);
  for (const item of fresh) merged.set(item.bookId, item);
  return [...merged.values()].sort((left, right) => (right.onlineTime || 0) - (left.onlineTime || 0));
}

async function fetchCompleteType(api: MonitorApi, type: NewReleaseType) {
  const items: SeriesItem[] = [];
  const ids = new Set<string>();
  const cursors = new Set<string>();
  let cursor = "";
  let lastPage: NewReleasePage | null = null;

  while (true) {
    const page = await api.fetchNewReleases(type, cursor, 20);
    lastPage = page;
    for (const item of page.items) {
      if (!item.bookId || ids.has(item.bookId)) continue;
      ids.add(item.bookId);
      items.push(item);
    }
    if (!page.hasMore) break;
    if (!page.nextCursor || cursors.has(page.nextCursor)) throw new Error("监听分页状态未前进，请重新刷新");
    cursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }

  return {
    items,
    date: lastPage?.date || shanghaiDate(),
    refreshedAt: lastPage?.refreshedAt || new Date().toISOString(),
  };
}

export function useNewReleaseMonitor({ api, storage, notifications, enabled, notify, intervalMs = 300_000 }: NewReleaseMonitorOptions) {
  const [type, setType] = useState<NewReleaseType>("playlet");
  const [items, setItems] = useState<SeriesItem[]>(() => loadReleaseItems(storage, shanghaiDate(), "playlet"));
  const [selectedCategory, setCategory] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [refreshedAt, setRefreshedAt] = useState("");
  const [unseenCount, setUnseenCount] = useState(0);
  const inFlight = useRef<Promise<void> | null>(null);
  const baseline = useRef(new Set<string>());
  const typeRef = useRef(type);

  const refresh = useCallback(() => {
    if (!enabled) return Promise.resolve();
    if (inFlight.current) return inFlight.current;
    const requestedType = typeRef.current;
    setLoading(true);
    setError("");
    const operation = fetchCompleteType(api, requestedType).then(async (scan) => {
      if (typeRef.current !== requestedType) return;
      const persisted = loadReleaseItems(storage, scan.date, requestedType);
      const merged = mergeItems(persisted, scan.items);
      const ids = scan.items.map((item) => item.bookId);
      const baselineKey = `${scan.date}|${requestedType}`;
      const isBaseline = !baseline.current.has(baselineKey);
      const seen = loadSeenReleases(storage, scan.date, requestedType);
      const newIds = ids.filter((id) => !seen.has(id));
      baseline.current.add(baselineKey);
      markSeenReleases(storage, scan.date, requestedType, merged.map((item) => item.bookId));
      saveReleaseItems(storage, scan.date, requestedType, merged);
      setItems(merged);
      setRefreshedAt(scan.refreshedAt);
      if (!isBaseline && newIds.length) {
        setUnseenCount((value) => value + newIds.length);
        if (notify) await notifications.send({
          title: "发现今日新剧",
          body: `新发现 ${newIds.length} 部今日上线短剧`,
          target: { kind: "monitor", id: requestedType },
        });
      }
    }).catch((reason) => {
      if (typeRef.current === requestedType) setError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => {
      inFlight.current = null;
      if (typeRef.current === requestedType) setLoading(false);
    });
    inFlight.current = operation;
    return operation;
  }, [api, enabled, notifications, notify, storage]);

  useEffect(() => {
    typeRef.current = type;
    setCategory("");
    setItems(loadReleaseItems(storage, shanghaiDate(), type));
    setError("");
    if (!enabled) return;
    let cancelled = false;
    const runLatestType = async () => {
      const pending = inFlight.current;
      if (pending) await pending;
      if (!cancelled && typeRef.current === type) await refresh();
    };
    void runLatestType();
    return () => { cancelled = true; };
  }, [enabled, refresh, storage, type]);

  useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => void refresh(), intervalMs);
    return () => window.clearInterval(timer);
  }, [enabled, intervalMs, refresh]);

  const categories = useMemo(() => releaseCategoryOptions(items), [items]);
  const filteredItems = useMemo(
    () => selectedCategory
      ? items.filter((item) => releaseCategoryNames(item).includes(selectedCategory))
      : items,
    [items, selectedCategory],
  );

  return {
    type,
    items,
    filteredItems,
    categories,
    selectedCategory,
    loading,
    error,
    refreshedAt,
    unseenCount,
    hasMore: false,
    setType,
    setCategory,
    refresh,
    loadMore: async () => undefined,
    clearUnseen: () => setUnseenCount(0),
  };
}

export type NewReleaseMonitor = ReturnType<typeof useNewReleaseMonitor>;
