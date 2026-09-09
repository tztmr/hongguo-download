import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type FormEvent, type UIEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  downloadEpisode,
  fetchCatalog,
  fetchCategoryGroups,
  fetchDiscovery,
  fetchDiscoveryByCategory,
  fetchDiscoveryMore,
  fetchHealth,
  fetchRank,
  fetchSearch,
  fetchSearchAll,
  fetchSeriesMetrics,
  fetchNewReleases,
  revealPath,
} from "./api";
import { AppRail } from "./components/AppRail";
import { CategoryFilter } from "./components/CategoryFilter";
import { Cover } from "./components/Cover";
import { DownloadManagerPage } from "./components/DownloadManagerPage";
import { CheckIcon, CloseIcon, SearchIcon } from "./components/icons";
import { SeriesInspector } from "./components/SeriesInspector";
import { SeriesDialog } from "./components/SeriesDialog";
import { useDownloadManager, type DownloadAdapter, type DownloadProgress } from "./download/useDownloadManager";
import { useMediaJobs } from "./media/useMediaJobs";
import {
  createGroupedPagingState,
  fillUniqueGroup,
  type GroupedPagingState,
} from "./feed/groupedPaging";
import { NewReleasesPage } from "./monitor/NewReleasesPage";
import { useNewReleaseMonitor } from "./monitor/useNewReleaseMonitor";
import { createTauriNotificationAdapter, type NotificationTarget } from "./notifications";
import { useNotificationRouter } from "./notifications/useNotificationRouter";
import {
  createMemoryStorage,
  createPreviewDownloadState,
  previewDownloadAdapter,
  previewAIComponents,
  previewEpisodes,
  previewMediaCommands,
  previewMediaJobs,
  previewSeries,
  previewYouTubeModel,
} from "./preview";
import { SettingsPage } from "./settings/SettingsPage";
import { useAppSettings, type AppSettingsDependencies } from "./settings/useAppSettings";
import { useYouTube } from "./youtube/useYouTube";
import type {
  AppSettings,
  ContentType,
  CategoryGroup,
  DiscoveryPage,
  EpisodeItem,
  NavId,
  RankPage,
  RankReleaseType,
  SearchContentType,
  SearchPage,
  SeriesItem,
} from "./types";

import { SearchCache, searchKey } from "./search/cache";
import { combinedDiscovery, type CombinedDiscoveryPage } from "./feed/combinedDiscovery";

type SearchMode = "fuzzy" | "exact";

const appNotifications = createTauriNotificationAdapter();
const DEFAULT_RANK_BOARDS = [
  { id: "ranklist_hot_sc", name: "推荐榜" },
  { id: "ranklist_hot_play_sc", name: "热播榜" },
  { id: "ranklist_prestige", name: "臻果榜" },
  { id: "ranklist_subscribe", name: "预约榜" },
  { id: "ranklist_new_rank_sc", name: "新剧榜" },
  { id: "ranklist_hot_search_sc", name: "热搜榜" },
  { id: "ranklist_must_watch", name: "必看榜" },
  { id: "ranklist_followed", name: "收藏榜" },
];
const liveMonitorApi = { fetchNewReleases };
const previewSettings: AppSettings = {
  version: 4,
  saveDir: "/Users/edking/Downloads/红果下载",
  definition: "auto",
  notifyDownloadComplete: true,
  notifyNewReleases: true,
  notifyMediaComplete: true,
  notifyYouTubeResult: true,
  demucsModel: "htdemucs",
  whisperModel: "small",
  aiDevice: "auto",
  downloadProxy: "",
  downloadMirror: "",
};
const previewSettingsApi: AppSettingsDependencies = {
  getSettings: async () => previewSettings,
  updateSettings: async (patch) => ({ ...previewSettings, ...patch }),
  chooseSaveDir: async () => previewSettings.saveDir,
  openSaveDir: async () => undefined,
  getNotificationStatus: async () => "granted",
  getAiComponents: async () => previewAIComponents,
  installAiComponent: async (id) => {
    const item = previewAIComponents.find((component) => component.id === id);
    if (!item) throw new Error("预览组件不存在");
    return { ...item, installed: true, installedVersion: item.version, installedPath: `/Preview/components/${id}` };
  },
  removeAiComponent: async () => undefined,
};
const previewMonitorApi = {
  fetchNewReleases: async () => ({
    items: previewSeries.slice(0, 20),
    nextCursor: "",
    hasMore: false,
    date: "2026-09-02",
    refreshedAt: "2026-09-02T09:30:00+08:00",
  }),
};
const previewNotifications = { getStatus: async () => "granted" as const, send: async () => true };

function normalizeSearchTitle(value: string) {
  return value.trim().replace(/\s+/g, " ").toLocaleLowerCase();
}

function filterSearchItems(items: SeriesItem[], keyword: string, mode: SearchMode) {
  if (mode === "fuzzy") return items;
  const expected = normalizeSearchTitle(keyword);
  return items.filter((item) => normalizeSearchTitle(item.title) === expected);
}

function rankTypeLabel(type: RankReleaseType) {
  if (type === "comic_series_rank") return "漫剧";
  if (type === "ai_playlet") return "AI剧";
  return "真人剧";
}

export default function App() {
  const previewMode = new URLSearchParams(window.location.search).get("preview");
  const isPreview = previewMode === "library" || previewMode === "downloads";
  const [nav, setNav] = useState<NavId>(previewMode === "downloads" ? "queue" : "discover");
  const [queueVisited, setQueueVisited] = useState(previewMode === "downloads");
  useEffect(() => { if (nav === "queue") setQueueVisited(true); }, [nav]);
  const [managerFocus, setManagerFocus] = useState<NotificationTarget | null>(null);
  const [contentType, setContentType] = useState<ContentType>("drama");
  const [searchContentType, setSearchContentType] = useState<SearchContentType>("all");
  const [query, setQuery] = useState("");
  const [submittedQuery, setSubmittedQuery] = useState("");
  const [searchRevision, setSearchRevision] = useState(0);
  const searchInput = useRef<HTMLInputElement>(null);
  const searchCache = useRef(new SearchCache<GroupedPagingState<SeriesItem, SearchPage | null>>(20, 5 * 60_000, Date.now, (state) => state.allItems.length <= 500));
  const [searchMode, setSearchMode] = useState<SearchMode>("fuzzy");
  const [items, setItems] = useState<SeriesItem[]>(isPreview ? previewSeries.slice(0, 20) : []);
  const [selected, setSelected] = useState<SeriesItem | null>(isPreview ? previewSeries[0] : null);
  const [episodes, setEpisodes] = useState<EpisodeItem[]>(isPreview ? previewEpisodes : []);
  const [selectedEpisodeIds, setSelectedEpisodeIds] = useState<string[]>(isPreview ? [previewEpisodes[0].itemId] : []);
  const [loading, setLoading] = useState(false);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogError, setCatalogError] = useState("");
  const [monitorDetailOpen, setMonitorDetailOpen] = useState(false);
  const [metricsLoading, setMetricsLoading] = useState(false);
  const [metricsError, setMetricsError] = useState("");
  const [error, setError] = useState("");
  const [toast, setToast] = useState("");
  const [healthOk, setHealthOk] = useState(isPreview);
  const [backgroundMonitorStarted, setBackgroundMonitorStarted] = useState(isPreview);
  const [discovery, setDiscovery] = useState<DiscoveryPage | null>(null);
  const [selectedCategory, setSelectedCategory] = useState("");
  const [categoryGroups, setCategoryGroups] = useState<CategoryGroup[]>([]);
  const [rankPage, setRankPage] = useState<RankPage | null>(null);
  const [rankBoard, setRankBoard] = useState("ranklist_hot_sc");
  const [rankType, setRankType] = useState<RankReleaseType>("all");
  const pageRequestRef = useRef(0);
  const catalogRequestRef = useRef(0);
  const loadMoreInFlightRef = useRef(false);
  const discoveryPagingRef = useRef<GroupedPagingState<SeriesItem, DiscoveryPage | null>>(
    isPreview
      ? { allItems: previewSeries, visibleCount: 20, cursor: null, hasMore: false }
      : createGroupedPagingState(null),
  );
  const rankPagingRef = useRef<GroupedPagingState<SeriesItem, RankPage | null>>(
    isPreview
      ? { allItems: previewSeries, visibleCount: 20, cursor: null, hasMore: false }
      : createGroupedPagingState(null),
  );
  const searchPagingRef = useRef<GroupedPagingState<SeriesItem, SearchPage | null>>(
    createGroupedPagingState(null),
  );

  const storage = useMemo(() => (isPreview ? createMemoryStorage() : window.localStorage), [isPreview]);
  const settingsModel = useAppSettings(isPreview ? previewSettingsApi : undefined);
  const activeSettings = settingsModel.settings || previewSettings;
  const monitor = useNewReleaseMonitor({
    api: isPreview ? previewMonitorApi : liveMonitorApi,
    storage,
    notifications: isPreview ? previewNotifications : appNotifications,
    enabled: backgroundMonitorStarted || nav === "monitor",
    notify: activeSettings.notifyNewReleases,
  });
  const adapter = useMemo<DownloadAdapter>(() => {
    if (isPreview) return previewDownloadAdapter;
    return {
      download: downloadEpisode,
      subscribeProgress: async (listener) =>
        listen<DownloadProgress>("download-progress", (event) => listener(event.payload)),
    };
  }, [isPreview]);
  const manager = useDownloadManager({
    adapter,
    storage,
    initialState: isPreview ? createPreviewDownloadState() : undefined,
    enabled: !isPreview,
    definition: activeSettings.definition,
    onBatchCompleted: async (batch) => {
      if (activeSettings.notifyDownloadComplete) {
        await appNotifications.send({
          title: "下载完成",
          body: `《${batch.title}》共 ${batch.items.length} 集下载完成`,
          target: { kind: "downloadBatch", id: batch.id },
        });
      }
    },
  });
  const media = useMediaJobs({
    enabled: !isPreview,
    commands: isPreview ? previewMediaCommands : undefined,
    initialJobs: isPreview ? previewMediaJobs : [],
  });
  const youtube = useYouTube(undefined, !isPreview);
  useNotificationRouter({
    adapter: appNotifications,
    media,
    youtube,
    notifyMedia: activeSettings.notifyMediaComplete ?? true,
    notifyYouTube: activeSettings.notifyYouTubeResult ?? true,
    enabled: !isPreview,
    onTarget: (target) => {
      if (target.kind === "monitor") {
        setNav("monitor");
      } else {
        setManagerFocus(target);
        setNav("queue");
      }
      const window = getCurrentWindow();
      void window.show().then(() => window.setFocus()).catch(() => undefined);
    },
  });

  useEffect(() => {
    if (isPreview) return;
    fetchHealth().then(() => setHealthOk(true)).catch(() => setHealthOk(false));
  }, [isPreview]);

  useEffect(() => {
    if (!healthOk || backgroundMonitorStarted) return;
    // Give the first visible page priority over the full background scan.
    const timer = window.setTimeout(() => setBackgroundMonitorStarted(true), 5000);
    return () => window.clearTimeout(timer);
  }, [healthOk, backgroundMonitorStarted]);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(""), 4200);
    return () => window.clearTimeout(timer);
  }, [toast]);

  async function selectSeries(item: SeriesItem) {
    const requestId = ++catalogRequestRef.current;
    setSelected(item);
    setCatalogLoading(true);
    setCatalogError("");
    setMetricsLoading(!isPreview);
    setMetricsError("");
    setSelectedEpisodeIds([]);
    if (isPreview) {
      const nextEpisodes = previewEpisodes.slice(0, Math.min(item.episodeCount || previewEpisodes.length, previewEpisodes.length));
      if (requestId !== catalogRequestRef.current) return;
      setEpisodes(nextEpisodes);
      setSelectedEpisodeIds(nextEpisodes[0] ? [nextEpisodes[0].itemId] : []);
      setCatalogLoading(false);
      setMetricsLoading(false);
      return;
    }
    setEpisodes([]);
    // Catalog and counters can finish independently; slow counters must not
    // prevent choosing episodes or adding them to the queue.
    const metricsRequest = fetchSeriesMetrics(item.seriesId, item.contentTypeCode).then((metrics) => {
      if (requestId !== catalogRequestRef.current) return;
      setSelected((current) =>
        current?.seriesId === item.seriesId && current.contentTypeCode === item.contentTypeCode
          ? { ...current, ...metrics }
          : current,
      );
    }).catch((reason) => {
      if (requestId === catalogRequestRef.current) setMetricsError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => {
      if (requestId === catalogRequestRef.current) setMetricsLoading(false);
    });
    const catalogRequest = fetchCatalog(item.bookId).then((catalog) => {
      if (requestId !== catalogRequestRef.current) return;
      setEpisodes(catalog);
      if (catalog[0]) setSelectedEpisodeIds([catalog[0].itemId]);
    }).catch((reason) => {
      if (requestId === catalogRequestRef.current) setCatalogError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => {
      if (requestId === catalogRequestRef.current) setCatalogLoading(false);
    });
    await Promise.all([catalogRequest, metricsRequest]);
  }

  async function loadDiscover() {
    const requestId = ++pageRequestRef.current;
    setLoading(true);
    setError("");
    try {
      if (searchContentType === "all") setCategoryGroups([]);
      else void fetchCategoryGroups(contentType).then((groups) => {
        if (requestId === pageRequestRef.current) setCategoryGroups(groups);
      }).catch(() => undefined);
      const initial = createGroupedPagingState<SeriesItem, DiscoveryPage | null>(null);
      const result = await fillUniqueGroup(initial, async (cursor) => {
        const page = searchContentType === "all"
          ? await combinedDiscovery(cursor, fetchDiscovery, fetchDiscoveryMore)
          : cursor ? await fetchDiscoveryMore(contentType, cursor)
          : selectedCategory
            ? await fetchDiscoveryByCategory(contentType, selectedCategory)
            : await fetchDiscovery(contentType);
        return { items: page.items, nextCursor: page, hasMore: page.hasMore };
      }, { maxRequests: 1 });
      if (requestId !== pageRequestRef.current) return;
      discoveryPagingRef.current = result.state;
      const page = result.state.cursor;
      setDiscovery(page ? { ...page, items: result.state.allItems } : null);
      setItems(result.visible);
      if (result.visible[0]) void selectSeries(result.visible[0]);
      setHealthOk(true);
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(nextError instanceof Error ? nextError.message : String(nextError));
      setHealthOk(false);
    } finally {
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  async function loadMoreDiscover() {
    const current = discoveryPagingRef.current;
    if ((!current.hasMore && current.visibleCount >= current.allItems.length) || loadMoreInFlightRef.current) return;
    loadMoreInFlightRef.current = true;
    const requestId = ++pageRequestRef.current;
    setLoading(true);
    try {
      const result = await fillUniqueGroup(current, async (cursor) => {
        if (!cursor) throw new Error("发现页分页状态丢失");
        const page = searchContentType === "all"
          ? await combinedDiscovery(cursor as CombinedDiscoveryPage, fetchDiscovery, fetchDiscoveryMore)
          : await fetchDiscoveryMore(contentType, cursor);
        return { items: page.items, nextCursor: page, hasMore: page.hasMore };
      });
      if (requestId !== pageRequestRef.current) return;
      discoveryPagingRef.current = result.state;
      setItems(result.visible);
      const page = result.state.cursor;
      setDiscovery(page ? { ...page, items: result.state.allItems } : null);
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      loadMoreInFlightRef.current = false;
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  async function loadRank(append = false) {
    if (append && loadMoreInFlightRef.current) return;
    if (append) loadMoreInFlightRef.current = true;
    const requestId = ++pageRequestRef.current;
    setLoading(true);
    setError("");
    try {
      const initial = append
        ? rankPagingRef.current
        : createGroupedPagingState<SeriesItem, RankPage | null>(null);
      const result = await fillUniqueGroup(initial, async (cursor) => {
        const page = await fetchRank({
          board: rankBoard,
          type: rankType,
          cursor: cursor?.nextCursor || "",
          limit: 20,
        });
        const mergedPage = cursor ? {
          ...page,
          boards: page.boards.length ? page.boards : cursor.boards,
        } : page;
        return { items: page.items, nextCursor: mergedPage, hasMore: page.hasMore };
      });
      if (requestId !== pageRequestRef.current) return;
      rankPagingRef.current = result.state;
      const page = result.state.cursor;
      setRankPage(page ? { ...page, items: result.state.allItems } : null);
      setItems(result.visible);
      if (!append && result.visible[0]) void selectSeries(result.visible[0]);
      setHealthOk(true);
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(nextError instanceof Error ? nextError.message : String(nextError));
      setHealthOk(false);
    } finally {
      if (append) loadMoreInFlightRef.current = false;
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  async function runSearch(key: string, append = false) {
    const keyword = key.trim().replace(/\s+/g, " ");
    if (!keyword || (append && loadMoreInFlightRef.current)) return;
    if (append) loadMoreInFlightRef.current = true;
    const requestId = ++pageRequestRef.current;
    const cacheKey = searchKey(keyword, searchContentType, searchMode);
    setLoading(true);
    setError("");
    if (!append) {
      setItems([]);
      setSelected(null);
      setEpisodes([]);
      catalogRequestRef.current += 1;
    }
    try {
      const load = async () => {
        const initial = append ? searchPagingRef.current : createGroupedPagingState<SeriesItem, SearchPage | null>(null);
        if (isPreview) {
          const matches = filterSearchItems(previewSeries.filter((item) => item.title.includes(keyword)
            && (searchContentType === "all" || item.contentTypeCode === (searchContentType === "drama" ? 1 : 2))), keyword, searchMode);
          const results = matches;
          return { allItems: results, visibleCount: Math.min(initial.visibleCount + 20, results.length), cursor: null, hasMore: false };
        }
        const result = await fillUniqueGroup(initial, async (cursor) => {
          const page = searchContentType === "all" ? await fetchSearchAll(keyword, cursor?.nextPassback || "")
            : await fetchSearch(keyword, searchContentType, cursor?.nextOffset || 0, cursor?.nextPassback || "");
          return { items: page.items, nextCursor: page, hasMore: page.hasMore };
        }, { include: (item) => filterSearchItems([item], keyword, searchMode).length === 1 });
        return result.state;
      };
      const result = append ? await load() : await searchCache.current.getOrLoad(cacheKey, load);
      if (requestId !== pageRequestRef.current) return;
      if (append) searchCache.current.set(cacheKey, result);
      searchPagingRef.current = result;
      setDiscovery(null);
      const visible = result.allItems.slice(0, result.visibleCount);
      setItems(visible);
      if (!append && visible[0]) void selectSeries(visible[0]);
    } catch (nextError) {
      if (requestId === pageRequestRef.current) setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      if (append) loadMoreInFlightRef.current = false;
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  useEffect(() => {
    loadMoreInFlightRef.current = false;
    setLoading(false);
    if (nav === "search") {
      if (submittedQuery) void runSearch(submittedQuery);
      else { setItems([]); setSelected(null); setEpisodes([]); setError(""); }
    } else if (!isPreview) {
      if (nav === "discover") void loadDiscover();
      if (nav === "rank") void loadRank();
    } else {
      if (nav === "discover") setItems(discoveryPagingRef.current.allItems.slice(0, discoveryPagingRef.current.visibleCount));
      if (nav === "rank") setItems(rankPagingRef.current.allItems.slice(0, rankPagingRef.current.visibleCount));
    }
    return () => { pageRequestRef.current += 1; };
    // Load from the submitted keyword, never from an unsubmitted input draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nav, contentType, searchContentType, selectedCategory, rankBoard, rankType, isPreview, submittedQuery, searchMode, searchRevision]);

  useEffect(() => {
    if (nav === "search") searchInput.current?.focus();
  }, [nav]);

  function onSearchSubmit(event: FormEvent) {
    event.preventDefault();
    if (!query.trim()) return;
    setMonitorDetailOpen(false);
    setSubmittedQuery(query.trim());
    setSearchRevision((value) => value + 1);
    setNav("search");
  }

  function selectContentType(type: SearchContentType) {
    setSearchContentType(type);
    if (type !== "all") setContentType(type);
    setSelectedCategory("");
    setCategoryGroups([]);
  }

  function onLibraryScroll(event: UIEvent<HTMLDivElement>) {
    if (loading || loadMoreInFlightRef.current) return;
    const scroller = event.currentTarget;
    if (scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight > 160) return;
    const discoveryState = discoveryPagingRef.current;
    const rankState = rankPagingRef.current;
    const searchState = searchPagingRef.current;
    if (nav === "discover" && (discoveryState.hasMore || discoveryState.visibleCount < discoveryState.allItems.length)) void loadMoreDiscover();
    if (nav === "rank" && (rankState.hasMore || rankState.visibleCount < rankState.allItems.length)) void loadRank(true);
    if (nav === "search" && (searchState.hasMore || searchState.visibleCount < searchState.allItems.length)) void runSearch(submittedQuery, true);
  }

  function enqueueSelection() {
    if (!selected) return;
    const chosen = episodes.filter((episode) => selectedEpisodeIds.includes(episode.itemId));
    const result = manager.enqueue(selected, chosen);
    setToast(`已加入 ${result.added} 集，跳过 ${result.skipped} 个重复项`);
  }

  const pendingCount = manager.stats.running + manager.stats.queued;
  const rankBoards = discovery?.rankBoards || [];
  const pageTitle = nav === "rank" ? "本周榜单" : nav === "search" ? "搜索结果" : "首页推荐";
  const navigate = (next: NavId) => {
    setMonitorDetailOpen(false);
    setNav(next);
    if (next === "monitor") monitor.clearUnseen();
  };

  return (
    <div className={`app-shell ${nav === "queue" || nav === "monitor" || nav === "settings" ? "queue-mode" : "library-mode"}`}>
      <AppRail nav={nav} pendingCount={pendingCount} unseenReleases={monitor.unseenCount} healthOk={healthOk} onNavigate={navigate} />
      {nav === "queue" || queueVisited ? (
        <DownloadManagerPage
          hidden={nav !== "queue"}
          manager={manager}
          media={media}
          saveDir={activeSettings.saveDir}
          demucsModel={activeSettings.demucsModel}
          whisperModel={activeSettings.whisperModel}
          aiComponents={isPreview ? undefined : settingsModel.components}
          onInstallComponent={isPreview ? undefined : settingsModel.installComponent}
          youtube={isPreview ? previewYouTubeModel : youtube}
          focusTarget={nav === "queue" ? managerFocus : null}
          onChooseDir={() => {
            void settingsModel.chooseDirectory();
          }}
          onOpenDir={() => {
            void settingsModel.openDirectory();
          }}
          onRevealPath={(path) => {
            if (!isPreview) void revealPath(path).catch((nextError) => setError(String(nextError)));
          }}
        />
      ) : null}
      {nav === "queue" ? null : nav === "monitor" ? (
        <NewReleasesPage model={monitor} onSelect={(item) => { setMonitorDetailOpen(true); void selectSeries(item); }} />
      ) : nav === "settings" ? (
        <SettingsPage model={settingsModel} youtube={isPreview ? previewYouTubeModel : youtube} />
      ) : (
        <main className="library-page">
          <header className="library-header">
            <div className="library-title"><span>红果下载</span><h1>{pageTitle}</h1></div>
            <form className="search-form" onSubmit={onSearchSubmit}>
              <SearchIcon />
              <input ref={searchInput} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索短剧或漫剧" aria-label="搜索短剧或漫剧" />
              <div className="search-mode-segment" role="group" aria-label="搜索识别方式">
                <button type="button" className={searchMode === "fuzzy" ? "active" : ""} aria-pressed={searchMode === "fuzzy"} onClick={() => setSearchMode("fuzzy")}>模糊识别</button>
                <button type="button" className={searchMode === "exact" ? "active" : ""} aria-pressed={searchMode === "exact"} onClick={() => setSearchMode("exact")}>匹配识别</button>
              </div>
              <button type="submit" className="search-submit" disabled={!query.trim()}>搜索</button>
            </form>
            <div className="content-segment" aria-label="内容类型">
              {nav === "rank" ? (
                <>
                  <button type="button" className={rankType === "all" ? "active" : ""} onClick={() => setRankType("all")}>全部</button>
                  <button type="button" className={rankType === "playlet" ? "active" : ""} onClick={() => setRankType("playlet")}>真人剧</button>
                  <button type="button" className={rankType === "comic_series_rank" ? "active" : ""} onClick={() => setRankType("comic_series_rank")}>漫剧</button>
                  <button type="button" className={rankType === "ai_playlet" ? "active" : ""} onClick={() => setRankType("ai_playlet")}>AI剧</button>
                </>
              ) : (
                <>
                  <button type="button" className={searchContentType === "all" ? "active" : ""} aria-pressed={searchContentType === "all"} onClick={() => selectContentType("all")} title="全部真人剧和漫剧">全部</button>
                  <button type="button" className={searchContentType === "drama" ? "active" : ""} aria-pressed={searchContentType === "drama"} onClick={() => selectContentType("drama")}>真人剧</button>
                  <button type="button" className={searchContentType === "manju" ? "active" : ""} aria-pressed={searchContentType === "manju"} onClick={() => selectContentType("manju")}>漫剧</button>
                  <button type="button" className="content-type-unavailable" disabled title="AI剧暂不支持关键词搜索">AI剧</button>
                </>
              )}
            </div>
          </header>

          {nav === "search" && submittedQuery ? <div className="search-result-summary"><span>“{submittedQuery}” · 已显示 {items.length} 项</span><button type="button" className="text-action" disabled={loading} onClick={() => { searchCache.current.clear(); setSearchRevision((value) => value + 1); }}>刷新结果</button></div> : null}
          {nav === "discover" && (categoryGroups.length || rankBoards.length) ? (
            <section className="filter-strip">
              {rankBoards.length ? <div className="filter-row"><span>榜单</span>{rankBoards.slice(0, 8).map((board) => <span className="filter-chip passive" key={board.schema || board.label}>{board.label}</span>)}</div> : null}
              <CategoryFilter groups={categoryGroups} selectedId={selectedCategory || "all"} onSelect={(id) => setSelectedCategory(id === "all" ? "" : id)} />
            </section>
          ) : null}

          {nav === "rank" ? (
            <section className="filter-strip">
              <div className="filter-row"><span>榜单</span>{(rankPage?.boards.length ? rankPage.boards : DEFAULT_RANK_BOARDS).map((board) => <button type="button" className={`filter-chip ${rankBoard === board.id ? "active" : ""}`} key={board.id} onClick={() => setRankBoard(board.id)}>{board.name}</button>)}</div>
            </section>
          ) : null}

          <section className="library-workspace">
            <div className="library-main" onScroll={onLibraryScroll}>
              {error ? <div className="inline-error">{error}</div> : null}
              {catalogError ? <div className="inline-error">{catalogError}</div> : null}
              {loading ? <div className="library-loading-overlay" role="status" aria-label="正在加载内容"><span className="loading-spinner" aria-hidden="true" />正在加载内容…</div> : null}
              {!loading && !items.length ? <div className="empty-library"><h2>{nav === "search" && !submittedQuery ? "搜索你想看的剧" : "没有找到短剧"}</h2><p>{nav === "search" && !submittedQuery ? "输入剧名，默认搜索全部真人剧和漫剧" : "换一个关键词或分类试试"}</p></div> : null}
              <div className="poster-grid">
                {items.map((item, index) => (
                  <button type="button" className={`poster-card ${selected?.bookId === item.bookId ? "selected" : ""}`} key={item.bookId} onClick={() => void selectSeries(item)}>
                    <div className="poster-image"><Cover src={item.cover} title={item.title} />{nav === "rank" ? <span className="rank-index">NO.{index + 1}</span> : null}{item.rankTags[0]?.label ? <span className="rank-label">{item.rankTags[0].label}</span> : null}</div>
                    <div className="poster-copy"><h2>{item.title}</h2><p>{item.episodeCount || "--"} 集 · {item.category || (nav === "rank" ? rankTypeLabel(rankType) : item.contentTypeCode === 2 ? "漫剧" : "真人剧")}{item.score ? ` · ${item.score}分` : ""}</p></div>
                  </button>
                ))}
              </div>
              {(nav === "discover"
                ? discoveryPagingRef.current.hasMore || discoveryPagingRef.current.visibleCount < discoveryPagingRef.current.allItems.length
                : nav === "search"
                  ? Boolean(submittedQuery) && (searchPagingRef.current.hasMore || searchPagingRef.current.visibleCount < searchPagingRef.current.allItems.length)
                  : rankPagingRef.current.hasMore || rankPagingRef.current.visibleCount < rankPagingRef.current.allItems.length) ? (
                <div className="load-more-row">{nav === "search" || nav === "discover" ? <button type="button" className="secondary-button" onClick={() => void (nav === "discover" ? loadMoreDiscover() : runSearch(submittedQuery, true))} disabled={loading}>{loading ? "加载中…" : "加载更多"}</button> : <span role="status">{loading ? "正在加载更多…" : "继续下拉加载更多"}</span>}</div>
              ) : null}
            </div>
            <SeriesInspector
              series={selected}
              definition={activeSettings.definition}
              episodes={episodes}
              selectedIds={selectedEpisodeIds}
              loading={catalogLoading}
              metricsLoading={metricsLoading}
              metricsError={metricsError}
              onSelectionChange={setSelectedEpisodeIds}
              onEnqueue={enqueueSelection}
            />
          </section>
        </main>
      )}
      {nav === "monitor" && monitorDetailOpen ? (
        <SeriesDialog onClose={() => setMonitorDetailOpen(false)}>
          {catalogError ? <p className="inline-error" role="alert">{catalogError}</p> : null}
          <SeriesInspector series={selected} definition={activeSettings.definition} episodes={episodes} selectedIds={selectedEpisodeIds} loading={catalogLoading} metricsLoading={metricsLoading} metricsError={metricsError} onSelectionChange={setSelectedEpisodeIds} onEnqueue={enqueueSelection} />
          {toast ? <p className="series-dialog-status" role="status">{toast}</p> : null}
        </SeriesDialog>
      ) : null}
      {toast ? <div className="toast" role="status"><span><CheckIcon size={14} /></span>{toast}<button type="button" aria-label="关闭提示" onClick={() => setToast("")}><CloseIcon size={16} /></button></div> : null}
    </div>
  );
}
