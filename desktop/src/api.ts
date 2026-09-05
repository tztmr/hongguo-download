import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AIComponentProgress,
  AIComponentStatus,
  AppSettings,
  CategoryGroup,
  ContentType,
  DiscoveryPage,
  EpisodeItem,
  NewReleasePage,
  NewReleaseType,
  RankPage,
  SearchPage,
  SeriesItem,
  SeriesMetrics,
} from "./types";

type RawSeries = {
  book_id?: string;
  series_id?: string;
  title?: string;
  cover?: string;
  first_vid?: string;
  content_type?: number;
  episode_count?: number;
  abstract?: string;
  score?: string;
  category?: string;
  category_tags?: string[];
  release_type?: NewReleaseType;
  author?: string;
  rank_tags?: Array<{ label?: string; schema?: string }>;
  online_time?: number | null;
  play_count?: number | null;
  hot_count?: number | null;
  collect_count?: number | null;
  like_count?: number | null;
};

function optionalNumber(value: number | null | undefined) {
  return value === null || value === undefined ? undefined : Number(value);
}

function asSeries(item: RawSeries): SeriesItem {
  const id = String(item.series_id || item.book_id || "");
  return {
    bookId: id,
    seriesId: id,
    title: item.title || "未命名短剧",
    cover: item.cover || "",
    firstVid: String(item.first_vid || ""),
    contentTypeCode: Number(item.content_type || 1),
    episodeCount: Number(item.episode_count || 0),
    abstract: item.abstract || "",
    score: item.score || "",
    category: item.category || "",
    categoryTags: Array.isArray(item.category_tags) ? item.category_tags.filter((value): value is string => typeof value === "string") : undefined,
    releaseType: item.release_type,
    author: item.author || "",
    rankTags: (item.rank_tags || [])
      .filter((tag) => tag.label)
      .map((tag) => ({ label: String(tag.label), schema: String(tag.schema || "") })),
    onlineTime: optionalNumber(item.online_time),
    playCount: optionalNumber(item.play_count),
    hotCount: optionalNumber(item.hot_count),
    collectCount: optionalNumber(item.collect_count),
    likeCount: optionalNumber(item.like_count),
  };
}

export async function fetchSeriesMetrics(
  seriesId: string,
  contentTypeCode: number,
): Promise<SeriesMetrics> {
  const query = new URLSearchParams({
    series_id: seriesId,
    content_type: String(contentTypeCode),
  });
  const data = await apiGet<{
    series_id: string;
    content_type: number;
    online_time?: number | null;
    play_count?: number | null;
    hot_count?: number | null;
    collect_count?: number | null;
    like_count?: number | null;
  }>(`/api/duanju/series-metrics?${query.toString()}`);
  return {
    seriesId: String(data.series_id || seriesId),
    contentTypeCode: Number(data.content_type || contentTypeCode),
    onlineTime: optionalNumber(data.online_time),
    playCount: optionalNumber(data.play_count),
    hotCount: optionalNumber(data.hot_count),
    collectCount: optionalNumber(data.collect_count),
    likeCount: optionalNumber(data.like_count),
  };
}

export async function fetchCategoryGroups(contentType: ContentType): Promise<CategoryGroup[]> {
  const data = await apiGet<{ groups?: CategoryGroup[] }>(
    `/api/duanju/categories?content_type=${contentType}`,
  );
  return data.groups || [];
}

export async function fetchNewReleases(
  type: NewReleaseType,
  cursor = "",
  limit = 20,
): Promise<NewReleasePage> {
  const query = new URLSearchParams({ type, limit: String(limit) });
  if (cursor) query.set("cursor", cursor);
  const data = await apiGet<{
    items?: RawSeries[];
    next_cursor?: string;
    has_more?: boolean;
    date?: string;
    refreshed_at?: string;
  }>(`/api/duanju/new-releases?${query.toString()}`);
  return {
    items: (data.items || []).map(asSeries),
    nextCursor: data.next_cursor || "",
    hasMore: Boolean(data.has_more),
    date: data.date || "",
    refreshedAt: data.refreshed_at || "",
  };
}

export async function apiGet<T>(path: string): Promise<T> {
  return invoke<T>("api_get", { path });
}

export async function fetchHealth() {
  return invoke<{ status: string; pool_size: number; active_count: number }>("health");
}

export async function fetchDiscovery(contentType: ContentType): Promise<DiscoveryPage> {
  return fetchDiscoveryPage(`/api/duanju/discovery?content_type=${contentType}`);
}

export async function fetchDiscoveryByCategory(
  contentType: ContentType,
  selectedItems: string,
): Promise<DiscoveryPage> {
  const first = await fetchDiscovery(contentType);
  if (!selectedItems) return first;
  return fetchDiscoveryMore(contentType, { ...first, selectedItems });
}

async function fetchDiscoveryPage(path: string): Promise<DiscoveryPage> {
  const data = await apiGet<{
    items: RawSeries[];
    cell_id: string;
    next_offset: number;
    has_more: boolean;
    session_id: string;
    plan_id: string;
    filter_ids: string;
    selected_items?: string;
    categories?: Array<{ id: string; name: string; group?: string }>;
    rank_boards?: Array<{ label: string; schema: string }>;
  }>(path);
  return {
    items: (data.items || []).map(asSeries),
    cellId: data.cell_id || "",
    nextOffset: Number(data.next_offset || 0),
    hasMore: Boolean(data.has_more),
    sessionId: data.session_id || "",
    planId: data.plan_id || "",
    filterIds: data.filter_ids || "",
    selectedItems: data.selected_items || "",
    categories: (data.categories || []).map((item) => ({
      id: item.id,
      name: item.name,
      group: item.group || "",
    })),
    rankBoards: data.rank_boards || [],
  };
}

export async function fetchDiscoveryMore(
  contentType: ContentType,
  page: DiscoveryPage,
): Promise<DiscoveryPage> {
  const query = new URLSearchParams({
    content_type: contentType,
    cell_id: page.cellId,
    offset: String(page.nextOffset),
    session_id: page.sessionId,
    plan_id: page.planId,
    filter_ids: page.filterIds,
    selected_items: page.selectedItems || "",
  });
  const data = await apiGet<{
    items: RawSeries[];
    cell_id: string;
    next_offset: number;
    has_more: boolean;
    session_id: string;
    plan_id: string;
    filter_ids: string;
    selected_items?: string;
    rank_boards?: Array<{ label: string; schema: string }>;
  }>(`/api/duanju/discovery/more?${query.toString()}`);
  return {
    items: (data.items || []).map(asSeries),
    cellId: data.cell_id || page.cellId,
    nextOffset: Number(data.next_offset || 0),
    hasMore: Boolean(data.has_more),
    sessionId: data.session_id || page.sessionId,
    planId: data.plan_id || page.planId,
    filterIds: data.filter_ids || page.filterIds,
    selectedItems: data.selected_items || page.selectedItems,
    categories: page.categories,
    rankBoards: page.rankBoards.length ? page.rankBoards : data.rank_boards || [],
  };
}

export async function fetchSearch(
  key: string,
  contentType: ContentType,
  offset = 0,
  passback = "",
): Promise<SearchPage> {
  const query = new URLSearchParams({
    key,
    content_type: contentType,
    offset: String(offset),
    passback,
  });
  const data = await apiGet<{
    items: RawSeries[];
    has_more: boolean;
    next_offset: number;
    next_passback: string;
  }>(`/api/duanju/search?${query.toString()}`);
  return {
    items: (data.items || []).map(asSeries),
    hasMore: Boolean(data.has_more),
    nextOffset: Number(data.next_offset || 0),
    nextPassback: data.next_passback || "",
  };
}

export async function fetchCatalog(bookId: string): Promise<EpisodeItem[]> {
  const data = await apiGet<{ items: Array<{ index: number; item_id: string; title: string }> }>(
    `/api/duanju/catalog?book_id=${encodeURIComponent(bookId)}`,
  );
  return (data.items || []).map((item) => ({
    index: Number(item.index || 0),
    itemId: String(item.item_id || ""),
    title: item.title || `第${item.index}集`,
  }));
}

export async function fetchRank(args: { board?: string; cursor?: string; limit?: number }): Promise<RankPage> {
  const query = new URLSearchParams({
    board: args.board || "ranklist_hot_sc",
    limit: String(args.limit || 20),
  });
  if (args.cursor) query.set("cursor", args.cursor);
  const data = await apiGet<{
    items: RawSeries[];
    next_cursor: string;
    has_more: boolean;
    board: string;
    board_name: string;
    boards?: Array<{ id: string; name: string }>;
  }>(`/api/duanju/rank?${query.toString()}`);
  return {
    items: (data.items || []).map(asSeries),
    nextCursor: data.next_cursor || "",
    hasMore: Boolean(data.has_more),
    board: data.board || "ranklist_hot_sc",
    boardName: data.board_name || "",
    boards: data.boards || [],
  };
}

export async function getSaveDir() {
  return invoke<string>("get_save_dir");
}

export async function getSettings() {
  return invoke<AppSettings>("get_settings");
}

export async function updateSettings(patch: Partial<Omit<AppSettings, "version" | "warning">>) {
  return invoke<AppSettings>("update_settings", { patch });
}

export async function chooseSaveDir() {
  return invoke<string>("choose_save_dir");
}

export async function openSaveDir() {
  return invoke<void>("open_save_dir");
}

export async function revealPath(path: string) {
  return invoke<void>("reveal_path", { path });
}

export async function downloadEpisode(args: {
  taskId: string;
  itemId: string;
  title: string;
  episodeTitle: string;
  definition: string;
}) {
  return invoke<{ taskId: string; path: string; definition: string; bytes: number }>(
    "download_episode",
    { args },
  );
}

export async function getAiComponents() {
  return invoke<AIComponentStatus[]>("get_ai_components");
}

export async function installAiComponent(id: string) {
  return invoke<AIComponentStatus>("install_ai_component", { id });
}

export async function removeAiComponent(id: string) {
  return invoke<void>("remove_ai_component", { id });
}

export async function subscribeAiComponentProgress(listener: (progress: AIComponentProgress) => void) {
  return listen<AIComponentProgress>("ai-component-progress", (event) => listener(event.payload));
}
