import { HeatMetric } from "./components/HeatMetric";
import { seriesTypeLabel, seriesHeatKey } from "./seriesPresentation";
import { AIRecommendations } from "./components/AIRecommendations";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type ComponentProps, type FormEvent, type UIEvent, useEffect, useMemo, useRef, useState } from "react";
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
import { CategoryBrowser } from "./components/CategoryBrowser";
import { Cover } from "./components/Cover";
import { VideoOrientationBadge } from "./components/VideoOrientationBadge";
import { deferPage } from "./components/DeferredPage";
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
  previewManagementCommands,
  previewAnalyticsCommands,
  previewSeries,
  previewYouTubeModel,
} from "./preview";
import { errorMessage, useAppSettings, type AppSettingsDependencies } from "./settings/useAppSettings";
import { useYouTube } from "./youtube/useYouTube";
import type {
  AppSettings,
  AIComponentProgress,
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

const DownloadManagerPage = deferPage<ComponentProps<typeof import("./components/DownloadManagerPage").DownloadManagerPage>>("下载管理", () => import("./components/DownloadManagerPage").then(module => ({ default: module.DownloadManagerPage })));
const PlatformVideosPage = deferPage<ComponentProps<typeof import("./components/PlatformVideosPage").PlatformVideosPage>>("视频管理", () => import("./components/PlatformVideosPage").then(module => ({ default: module.PlatformVideosPage })));
const DataAnalyticsPage = deferPage<ComponentProps<typeof import("./components/DataAnalyticsPage").DataAnalyticsPage>>("数据分析", () => import("./components/DataAnalyticsPage").then(module => ({ default: module.DataAnalyticsPage })));
const AutomationPage = deferPage<ComponentProps<typeof import("./monitor/AutomationPage").AutomationPage>>("自动追剧", () => import("./monitor/AutomationPage").then(module => ({ default: module.AutomationPage })));
const SettingsPage = deferPage<ComponentProps<typeof import("./settings/SettingsPage").SettingsPage>>("设置", () => import("./settings/SettingsPage").then(module => ({ default: module.SettingsPage })));

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
const previewInstallListeners = new Set<(progress: AIComponentProgress) => void>();
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
    // Exercise the same progress UI without downloading files in preview mode.
    for (const [stage, percent] of [["downloading", 20], ["downloading", 35], ["verifying", 50], ["extracting", 70], ["selfTesting", 90]] as const) {
      await new Promise(resolve => window.setTimeout(resolve, 1500));
      previewInstallListeners.forEach(listener => listener({ id, stage, percent }));
    }
    await new Promise(resolve => window.setTimeout(resolve, 1500));
    return { ...item, installed: true, installedVersion: item.version, installedPath: `/Preview/components/${id}` };
  },
  subscribeAiComponentProgress: async (listener) => {
    previewInstallListeners.add(listener);
    return () => { previewInstallListeners.delete(listener); };
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



export default function App() {
  const previewMode = new URLSearchParams(window.location.search).get("preview");
  const isPreview = previewMode === "library" || previewMode === "downloads" || previewMode === "automation";
  const [nav, setNav] = useState<NavId>(previewMode === "automation" ? "automation" : previewMode === "downloads" ? "queue" : "discover");
  const [platformVisited, setPlatformVisited] = useState(false);
  const [analyticsVisited, setAnalyticsVisited] = useState(false);
  const [automationVisited, setAutomationVisited] = useState(previewMode === "automation");
  const [settingsVisited, setSettingsVisited] = useState(false);
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
  const [knownHeat, setKnownHeat] = useState<Record<string, number>>({});
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
  const [operationError, setOperationError] = useState("");
  const [dismissedSettingsWarning, setDismissedSettingsWarning] = useState("");
  const [healthOk, setHealthOk] = useState(isPreview);
  const [backgroundMonitorStarted, setBackgroundMonitorStarted] = useState(isPreview);
  const [selectedCategory, setSelectedCategory] = useState("");
  const [categoryGroups, setCategoryGroups] = useState<CategoryGroup[]>([]);
  const [browseCategories, setBrowseCategories] = useState(false);
  const [homeAI, setHomeAI] = useState(false);
  const webCategories = browseCategories && searchContentType === "drama" && !homeAI;
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
  const activeSettings = settingsModel.settings || { ...previewSettings, saveDir: isPreview ? previewSettings.saveDir : "" };
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
    enabled: !isPreview && !!settingsModel.settings,
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
        if (target.id === "playlet" || target.id === "comic_series_rank" || target.id === "ai_playlet") monitor.setType(target.id);
        monitor.clearUnseen();
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
    let active = true, checking = false;
    async function check() {
      if (checking) return;
      checking = true;
      try { const health = await fetchHealth(); if (active) setHealthOk(health.status === "ok"); }
      catch { if (active) setHealthOk(false); }
      finally { checking = false; }
    }
    void check();
    const timer = window.setInterval(() => void check(), 30_000);
    return () => { active = false; window.clearInterval(timer); };
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
      if (metrics.hotCount !== undefined && Number.isFinite(metrics.hotCount) && metrics.hotCount >= 0) {
        const heat = metrics.hotCount;
        setKnownHeat(current => ({ ...current, [seriesHeatKey(item)]: heat }));
      }
      setSelected((current) =>
        current?.seriesId === item.seriesId && current.contentTypeCode === item.contentTypeCode
          ? { ...current, ...Object.fromEntries(Object.entries(metrics).filter(([, value]) => value !== undefined)) }
          : current,
      );
    }).catch((reason) => {
      if (requestId === catalogRequestRef.current) setMetricsError(errorMessage(reason));
    }).finally(() => {
      if (requestId === catalogRequestRef.current) setMetricsLoading(false);
    });
    const catalogRequest = fetchCatalog(item.bookId).then((catalog) => {
      if (requestId !== catalogRequestRef.current) return;
      setEpisodes(catalog);
      if (catalog[0]) setSelectedEpisodeIds([catalog[0].itemId]);
    }).catch((reason) => {
      if (requestId === catalogRequestRef.current) setCatalogError(errorMessage(reason));
    }).finally(() => {
      if (requestId === catalogRequestRef.current) setCatalogLoading(false);
    });
    await Promise.all([catalogRequest, metricsRequest]);
  }

  function resetLibrarySelection() {
    catalogRequestRef.current += 1;
    setSelected(null); setEpisodes([]); setSelectedEpisodeIds([]);
    setCatalogError(""); setMetricsError(""); setCatalogLoading(false); setMetricsLoading(false);
  }

  async function loadDiscover() {
    const requestId = ++pageRequestRef.current;
    resetLibrarySelection(); setItems([]);
    discoveryPagingRef.current = createGroupedPagingState(null);
    setLoading(true);
    setError("");
    try {
      if (searchContentType !== "manju") setCategoryGroups([]);
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
      setItems(result.visible);
      if (result.visible[0]) void selectSeries(result.visible[0]);
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(errorMessage(nextError));
    } finally {
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  async function loadMoreDiscover() {
    const current = discoveryPagingRef.current;
    if ((!current.hasMore && current.visibleCount >= current.allItems.length) || loadMoreInFlightRef.current) return;
    loadMoreInFlightRef.current = true;
    const requestId = ++pageRequestRef.current;
    setLoading(true); setError("");
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
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(errorMessage(nextError));
    } finally {
      if (requestId === pageRequestRef.current) loadMoreInFlightRef.current = false;
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  async function loadRank(append = false) {
    if (append && loadMoreInFlightRef.current) return;
    if (append) loadMoreInFlightRef.current = true;
    const requestId = ++pageRequestRef.current;
    if (!append) { resetLibrarySelection(); setItems([]); setRankPage(null); rankPagingRef.current = createGroupedPagingState(null); }
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
    } catch (nextError) {
      if (requestId !== pageRequestRef.current) return;
      setError(errorMessage(nextError));
    } finally {
      if (append && requestId === pageRequestRef.current) loadMoreInFlightRef.current = false;
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
      resetLibrarySelection();
      searchPagingRef.current = createGroupedPagingState(null);
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
      const visible = result.allItems.slice(0, result.visibleCount);
      setItems(visible);
      if (!append && visible[0]) void selectSeries(visible[0]);
    } catch (nextError) {
      if (requestId === pageRequestRef.current) setError(errorMessage(nextError));
    } finally {
      if (append && requestId === pageRequestRef.current) loadMoreInFlightRef.current = false;
      if (requestId === pageRequestRef.current) setLoading(false);
    }
  }

  useEffect(() => {
    loadMoreInFlightRef.current = false;
    setLoading(false);
    if (nav === "discover" && (webCategories || homeAI)) {
      resetLibrarySelection();
      return () => { pageRequestRef.current += 1; catalogRequestRef.current += 1; };
    }
    if (nav === "search") {
      if (submittedQuery) void runSearch(submittedQuery);
      else { setItems([]); resetLibrarySelection(); setError(""); }
    } else if (!isPreview) {
      if (nav === "discover") void loadDiscover();
      if (nav === "rank") void loadRank();
    } else {
      if (nav === "discover") setItems(discoveryPagingRef.current.allItems.slice(0, discoveryPagingRef.current.visibleCount));
      if (nav === "rank") setItems(rankPagingRef.current.allItems.slice(0, rankPagingRef.current.visibleCount));
    }
    return () => { pageRequestRef.current += 1; catalogRequestRef.current += 1; };
    // Load from the submitted keyword, never from an unsubmitted input draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nav, contentType, searchContentType, selectedCategory, rankBoard, rankType, isPreview, submittedQuery, searchMode, searchRevision, browseCategories, homeAI, webCategories]);

  useEffect(() => {
    if (nav === "search") searchInput.current?.focus();
  }, [nav]);

  function onSearchSubmit(event: FormEvent) {
    event.preventDefault();
    if (!query.trim()) return;
    setMonitorDetailOpen(false);
    setHomeAI(false);
    setSubmittedQuery(query.trim());
    setSearchRevision((value) => value + 1);
    setNav("search");
  }

  function selectContentType(type: SearchContentType) {
    setHomeAI(false);
    if (type === "all") setBrowseCategories(false);
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
    if (!isPreview && !settingsModel.settings) { setOperationError("下载设置尚未读取成功，请前往设置重试后再加入队列"); return; }
    const chosen = episodes.filter((episode) => selectedEpisodeIds.includes(episode.itemId));
    const result = manager.enqueue(selected, chosen);
    setToast(`已加入 ${result.added} 集，跳过 ${result.skipped} 个重复项`);
  }

  const pendingCount = manager.stats.running + manager.stats.queued;
  const pageTitle = nav === "rank" ? "短剧榜单" : nav === "search" ? "搜索结果" : browseCategories ? "分类浏览" : "首页推荐";
  const navigate = (next: NavId) => {
    setMonitorDetailOpen(false);
    setNav(next);
    if (next === "platformVideos") setPlatformVisited(true);
    if (next === "analytics") setAnalyticsVisited(true);
    if (next === "automation") setAutomationVisited(true);
    if (next === "settings") setSettingsVisited(true);
    if (next === "monitor") monitor.clearUnseen();
  };

  return (
    <div className={`app-shell ${nav === "queue" || nav === "monitor" || nav === "automation" || nav === "settings" || nav === "platformVideos" || nav === "analytics" ? "queue-mode" : "library-mode"}`}>
      <AppRail nav={nav} pendingCount={pendingCount} unseenReleases={monitor.unseenCount} healthOk={healthOk} onNavigate={navigate} />
      {nav === "queue" || queueVisited ? (
        <DownloadManagerPage
          hidden={nav !== "queue"}
          manager={manager}
          media={media}
          saveDir={activeSettings.saveDir}
          aiConcurrency={activeSettings.aiConcurrency}
          onAIConcurrencyChange={async value => { setDismissedSettingsWarning(""); await settingsModel.update({ aiConcurrency: value }); }}
          demucsModel={activeSettings.demucsModel}
          whisperModel={activeSettings.whisperModel}
          aiComponents={settingsModel.components}
          onInstallComponent={settingsModel.installComponent}
          youtube={isPreview ? previewYouTubeModel : youtube}
          focusTarget={nav === "queue" ? managerFocus : null}
          onChooseDir={() => {
            setDismissedSettingsWarning("");
            void settingsModel.chooseDirectory();
          }}
          onOpenDir={() => {
            setDismissedSettingsWarning("");
            void settingsModel.openDirectory();
          }}
          onRevealPath={(path) => {
            if (!isPreview) void revealPath(path).catch((nextError) => setOperationError(errorMessage(nextError)));
          }}
        />
      ) : null}
      {platformVisited && <PlatformVideosPage hidden={nav !== "platformVideos"} youtube={isPreview ? previewYouTubeModel : youtube} commands={isPreview ? previewManagementCommands : undefined} />}
      {analyticsVisited && <DataAnalyticsPage hidden={nav !== "analytics"} youtube={isPreview ? previewYouTubeModel : youtube} commands={isPreview ? previewAnalyticsCommands : undefined} />}
      {automationVisited && <AutomationPage hidden={nav !== "automation"} aiConcurrency={activeSettings.aiConcurrency} onAIConcurrencyChange={async value => { setDismissedSettingsWarning(""); await settingsModel.update({ aiConcurrency: value }); }} runtimeEnabled={!isPreview} saveDir={activeSettings.saveDir} channels={(isPreview ? previewYouTubeModel : youtube).channels} onOpenSettings={() => navigate("settings")} />}
      {settingsVisited && <SettingsPage hidden={nav !== "settings"} model={settingsModel} youtube={isPreview ? previewYouTubeModel : youtube} />}
      {nav === "queue" || nav === "platformVideos" || nav === "analytics" || nav === "automation" || nav === "settings" ? null : nav === "monitor" ? (
        <NewReleasesPage model={monitor} detectOrientation={!isPreview} onSelect={(item) => { setMonitorDetailOpen(true); void selectSeries(item); }} />
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
                  <button type="button" className={(nav !== "discover" || !homeAI) && searchContentType === "all" ? "active" : ""} aria-pressed={(nav !== "discover" || !homeAI) && searchContentType === "all"} onClick={() => selectContentType("all")} title="全部真人剧和漫剧">全部</button>
                  <button type="button" className={(nav !== "discover" || !homeAI) && searchContentType === "drama" ? "active" : ""} aria-pressed={(nav !== "discover" || !homeAI) && searchContentType === "drama"} onClick={() => selectContentType("drama")}>真人剧</button>
                  <button type="button" className={(nav !== "discover" || !homeAI) && searchContentType === "manju" ? "active" : ""} aria-pressed={(nav !== "discover" || !homeAI) && searchContentType === "manju"} onClick={() => selectContentType("manju")}>漫剧</button>
                  <button type="button" className={nav === "discover" && homeAI ? "active" : ""} aria-pressed={nav === "discover" && homeAI} disabled={nav === "search"} title={nav === "search" ? "AI剧暂不支持关键词搜索" : "AI剧推荐与题材浏览"} onClick={() => { setHomeAI(true); setSelectedCategory(""); }}>AI剧</button>
                </>
              )}
            </div>
          </header>

          {nav === "search" && submittedQuery ? <div className="search-result-summary"><span>“{submittedQuery}” · 已显示 {items.length} 项</span><button type="button" className="text-action" disabled={loading} onClick={() => { searchCache.current.clear(); setSearchRevision((value) => value + 1); }}>刷新结果</button></div> : null}
          {nav === "discover" ? (
            <section className="filter-strip">
              <div className="home-source-row" role="group" aria-label="首页浏览方式">
                <button type="button" className={`filter-chip ${!browseCategories ? "active" : ""}`} aria-pressed={!browseCategories} onClick={() => setBrowseCategories(false)}>推荐</button>
                <button type="button" className={`filter-chip ${browseCategories ? "active" : ""}`} aria-pressed={browseCategories} onClick={() => { setSelectedCategory(""); if (searchContentType === "all" && !homeAI) { setSearchContentType("drama"); setContentType("drama"); } setBrowseCategories(true); }}>分类浏览</button>
                <span>{homeAI ? "AI剧 · 推荐与题材浏览" : webCategories ? "真人剧 · 多个条件可组合" : searchContentType === "manju" ? "漫剧视频 · 单选分类" : "发现剧目，也可按分类筛选"}</span>
              </div>
              {(nav !== "discover" || !homeAI) && searchContentType === "manju" ? <CategoryFilter groups={categoryGroups} selectedId={selectedCategory || "all"} onSelect={(id) => setSelectedCategory(id === "all" ? "" : id)} /> : null}
            </section>
          ) : null}

          {nav === "rank" ? (
            <section className="filter-strip">
              <div className="filter-row"><span>榜单</span>{(rankPage?.boards.length ? rankPage.boards : DEFAULT_RANK_BOARDS).map((board) => <button type="button" className={`filter-chip ${rankBoard === board.id ? "active" : ""}`} key={board.id} onClick={() => setRankBoard(board.id)}>{board.name}</button>)}</div>
            </section>
          ) : null}

          <section className="library-workspace">
            <div className="library-main" onScroll={nav === "discover" && (webCategories || homeAI) ? undefined : onLibraryScroll}>
              {catalogError ? <div className="inline-error">{catalogError}</div> : null}
              {nav === "discover" && homeAI ? <AIRecommendations knownHeat={knownHeat} categoryMode={browseCategories} selectedId={selected?.bookId} detectOrientation={!isPreview} onSelect={(item) => void selectSeries(item)} /> : nav === "discover" && webCategories ? <CategoryBrowser knownHeat={knownHeat} selectedId={selected?.bookId} detectOrientation={!isPreview} onSelect={(item) => void selectSeries(item)} onResetSelection={() => {
                catalogRequestRef.current += 1;
                setSelected(null); setEpisodes([]); setSelectedEpisodeIds([]);
                setCatalogError(""); setMetricsError(""); setCatalogLoading(false); setMetricsLoading(false);
              }} /> : <>
              {error ? <div className="inline-error" role="alert">{error}<button type="button" className="text-action" disabled={loading} onClick={() => { const append = items.length > 0; void (nav === "rank" ? loadRank(append) : nav === "search" ? runSearch(submittedQuery, append) : append ? loadMoreDiscover() : loadDiscover()); }}>重试加载</button></div> : null}
              {nav === "rank" && rankPage?.sourceNote ? <p className="monitor-refreshed">{rankPage.sourceNote}</p> : null}
              {loading ? <div className="library-loading-overlay" role="status" aria-label="正在加载内容"><span className="loading-spinner" aria-hidden="true" />正在加载内容…</div> : null}
              {!loading && !error && !items.length ? <div className="empty-library"><h2>{nav === "search" && !submittedQuery ? "搜索你想看的剧" : "没有找到短剧"}</h2><p>{nav === "search" && !submittedQuery ? "输入剧名，默认搜索全部真人剧和漫剧" : "换一个关键词或分类试试"}</p></div> : null}
              <div className="poster-grid">
                {items.map((item, index) => (
                  <button type="button" className={`poster-card ${selected?.bookId === item.bookId ? "selected" : ""}`} key={item.bookId} onClick={() => void selectSeries(item)}>
                    <div className="poster-image">
                      <Cover src={item.cover} title={item.title} />
                      <div className="poster-badges">
                        {nav === "rank" ? <span className="rank-index">NO.{index + 1}</span> : null}
                        <VideoOrientationBadge seriesId={item.seriesId} firstVid={item.firstVid} enabled={!isPreview} />
                      </div>
                      {item.rankTags[0]?.label ? <span className="rank-label">{item.rankTags[0].label}</span> : null}
                    </div>
                    <div className="poster-copy"><h2>{item.title}</h2><p>{item.episodeCount || "--"} 集 · {seriesTypeLabel(item)}{item.category && !["真人剧", "漫剧", "AI剧"].includes(item.category) ? ` · ${item.category}` : ""}{item.score ? ` · ${item.score}分` : ""}</p><HeatMetric value={item.hotCount ?? knownHeat[seriesHeatKey(item)]} /></div>
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
              </>}
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
      {(operationError || (nav !== "settings" && settingsModel.warning !== dismissedSettingsWarning && settingsModel.warning)) && <div className="app-operation-error" role="alert"><span>{operationError || settingsModel.warning}</span><button type="button" className="text-action" onClick={() => navigate("settings")}>查看设置</button><button type="button" className="icon-button" aria-label="关闭操作错误" onClick={() => { setOperationError(""); setDismissedSettingsWarning(settingsModel.warning); }}>×</button></div>}
    </div>
  );
}
