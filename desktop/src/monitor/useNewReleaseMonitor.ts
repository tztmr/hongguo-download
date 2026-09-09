import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { NotificationAdapter } from "../notifications";
import type { NewReleasePage, NewReleaseType, SeriesItem } from "../types";
import { releaseCategoryNames, releaseCategoryOptions } from "./categories";
import { loadReleaseItems, loadSeenReleases, markSeenReleases, saveReleaseItems } from "./storage";

type MonitorApi = { fetchNewReleases(type: NewReleaseType, cursor?: string, limit?: number): Promise<NewReleasePage> };
export type MonitorSort = "latest" | "hot" | "collected";

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

function metric(value: number | undefined) {
  return value !== undefined && Number.isFinite(value) && value >= 0 ? value : -1;
}

function mergeItems(persisted: SeriesItem[], fresh: SeriesItem[]) {
  const merged = new Map<string, SeriesItem>();
  for (const item of persisted) merged.set(item.bookId, item);
  for (const item of fresh) if (item.bookId) merged.set(item.bookId, item);
  return [...merged.values()].sort((left, right) => metric(right.onlineTime) - metric(left.onlineTime));
}

export function useNewReleaseMonitor({ api, storage, notifications, enabled, notify, intervalMs = 300_000 }: NewReleaseMonitorOptions) {
  const [selection, setSelection] = useState<{ type: NewReleaseType }>({ type: "playlet" });
  const type = selection.type;
  const [date, setDate] = useState(shanghaiDate);
  const [dateScope, setDateScope] = useState<"today" | "latest">("today");
  const [source, setSource] = useState<"subscribe" | "rank">("subscribe");
  const [items, setItems] = useState<SeriesItem[]>(() => loadReleaseItems(storage, shanghaiDate(), "playlet"));
  const [selectedCategory, setCategory] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<MonitorSort>("latest");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [scanPages, setScanPages] = useState(0);
  const [scanComplete, setScanComplete] = useState(false);
  const [refreshedAt, setRefreshedAt] = useState("");
  const [unseenCount, setUnseenCount] = useState(0);
  const generation = useRef(0);
  const inFlight = useRef<{ generation: number; promise: Promise<void> } | null>(null);
  const baseline = useRef(new Set<string>());
  const typeRef = useRef(type);
  const dateRef = useRef(date);
  const notificationRef = useRef({ notifications, notify });

  useEffect(() => { notificationRef.current = { notifications, notify }; }, [notifications, notify]);

  // Invalidate immediately, including a quick A → B → A switch before effects run.
  const setType = useCallback((next: NewReleaseType) => {
    if (next === typeRef.current) return;
    generation.current += 1;
    inFlight.current = null;
    typeRef.current = next;
    setSelection({ type: next });
  }, []);

  const refresh = useCallback((): Promise<void> => {
    if (!enabled) return Promise.resolve();
    if (inFlight.current?.generation === generation.current) return inFlight.current.promise;
    const requestedGeneration = generation.current;
    const requestedType = typeRef.current;
    const requestedDate = shanghaiDate();
    const valid = () => generation.current === requestedGeneration && typeRef.current === requestedType;
    const switchDate = (nextDate: string) => {
      dateRef.current = nextDate;
      setDate(nextDate);
      setItems(loadReleaseItems(storage, nextDate, requestedType));
      setRefreshedAt("");
      setUnseenCount(0);
    };
    if (dateRef.current !== requestedDate) switchDate(requestedDate);
    setLoading(true);
    setError("");
    setScanPages(0);
    setScanComplete(false);

    const run = async () => {
      const fresh = new Map<string, SeriesItem>();
      const cursors = new Set<string>();
      let cursor = "";
      let scanDate = "";
      let scope: "today" | "latest" = requestedType === "playlet" ? "today" : "latest";
      let merged: SeriesItem[] = [];
      while (valid()) {
        const page = await api.fetchNewReleases(requestedType, cursor, 20);
        if (!valid()) return;
        if (shanghaiDate() !== requestedDate) {
          switchDate(shanghaiDate());
          setScanPages(0);
          throw new Error("北京时间已跨日，请刷新以检查今天的新剧");
        }
        const pageDate = page.date || requestedDate;
        if (scanDate && pageDate !== scanDate) throw new Error("分页日期发生变化，本次检查未完成，请重新刷新");
        if (!scanDate) {
          scanDate = pageDate;
          scope = page.dateScope ?? scope;
          dateRef.current = scanDate;
          setDate(scanDate);
          setDateScope(scope);
          setSource(page.source ?? (requestedType === "playlet" ? "subscribe" : "rank"));
          merged = loadReleaseItems(storage, scanDate, requestedType);
        }
        for (const item of page.items) if (item.bookId) fresh.set(item.bookId, item);
        merged = mergeItems(merged, page.items);
        saveReleaseItems(storage, scanDate, requestedType, merged);
        setItems(merged);
        setScanPages((value) => value + 1);
        if (!page.hasMore) {
          setScanComplete(true);
          setRefreshedAt(page.refreshedAt || new Date().toISOString());
          break;
        }
        if (!page.nextCursor || cursors.has(page.nextCursor)) throw new Error("监听分页状态未前进，本次检查未完成，请重新刷新");
        cursors.add(page.nextCursor);
        cursor = page.nextCursor;
      }
      if (!valid()) return;
      const baselineKey = `${scanDate}|${requestedType}`;
      const isBaseline = !baseline.current.has(baselineKey);
      const seen = loadSeenReleases(storage, scanDate, requestedType);
      const newIds = [...fresh.keys()].filter((id) => !seen.has(id));
      baseline.current.add(baselineKey);
      markSeenReleases(storage, scanDate, requestedType, merged.map((item) => item.bookId));
      if (!isBaseline && newIds.length) {
        setUnseenCount((value) => value + newIds.length);
        const currentNotifications = notificationRef.current;
        if (currentNotifications.notify) {
          // Notification delivery does not determine whether the feed scan completed.
          await currentNotifications.notifications.send({
            title: scope === "today" ? "发现今日新剧" : "发现新收录剧目",
            body: scope === "today" ? `新发现 ${newIds.length} 部今日上线短剧` : `新剧榜新增收录 ${newIds.length} 部剧目`,
            target: { kind: "monitor", id: requestedType },
          }).catch(() => false);
        }
      }
    };
    const operation = run().catch((reason) => {
      if (valid()) setError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => {
      if (inFlight.current?.generation === requestedGeneration) inFlight.current = null;
      if (valid()) setLoading(false);
    });
    inFlight.current = { generation: requestedGeneration, promise: operation };
    return operation;
  }, [api, enabled, storage]);

  useEffect(() => {
    generation.current += 1;
    inFlight.current = null;
    typeRef.current = type;
    const today = shanghaiDate();
    dateRef.current = today;
    setDate(today);
    setDateScope(type === "playlet" ? "today" : "latest");
    setSource(type === "playlet" ? "subscribe" : "rank");
    setCategory("");
    setQuery("");
    setItems(loadReleaseItems(storage, today, type));
    setError("");
    setLoading(false);
    setScanPages(0);
    setScanComplete(false);
    setRefreshedAt("");
    if (enabled) void refresh();
    return () => {
      generation.current += 1;
      inFlight.current = null;
    };
  }, [enabled, refresh, selection, storage, type]);

  useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => void refresh(), intervalMs);
    return () => window.clearInterval(timer);
  }, [enabled, intervalMs, refresh]);

  const categories = useMemo(() => releaseCategoryOptions(items), [items]);
  const filteredItems = useMemo(() => {
    const search = query.trim().toLocaleLowerCase();
    return items.filter((item) =>
      (!selectedCategory || releaseCategoryNames(item).includes(selectedCategory)) &&
      (!search || item.title.toLocaleLowerCase().includes(search)),
    ).sort((left, right) => {
      const value = (item: SeriesItem) => metric(sort === "hot" ? item.hotCount : sort === "collected" ? item.collectCount : item.onlineTime);
      return value(right) - value(left) || metric(right.onlineTime) - metric(left.onlineTime);
    });
  }, [items, query, selectedCategory, sort]);

  return {
    type, date, source, dateScope, items, filteredItems, categories, selectedCategory,
    query, sort, loading, error, scanPages, scanComplete, refreshedAt, unseenCount,
    hasMore: false, setType, setCategory, setQuery, setSort, refresh,
    loadMore: async () => undefined,
    clearUnseen: () => setUnseenCount(0),
  };
}

export type NewReleaseMonitor = ReturnType<typeof useNewReleaseMonitor>;
