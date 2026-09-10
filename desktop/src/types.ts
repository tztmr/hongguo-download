export type ContentType = "drama" | "manju";
export type SearchContentType = "all" | ContentType;
export type RankReleaseType = "all" | NewReleaseType;
export type NavId = "discover" | "search" | "rank" | "monitor" | "queue" | "platformVideos" | "analytics" | "settings";
export type DefinitionPreference = "auto" | "1080p" | "720p";
export type DemucsModel = "htdemucs" | "htdemucs_ft";
export type WhisperModel = "small" | "medium";
export type AIDevicePreference = "auto" | "cpu" | "cuda";
export type NewReleaseType = "playlet" | "comic_series_rank" | "ai_playlet";

export type SeriesMetrics = {
  seriesId: string;
  contentTypeCode: number;
  onlineTime?: number;
  playCount?: number;
  hotCount?: number;
  collectCount?: number;
  likeCount?: number;
  commentCount?: number;
};

export type SeriesItem = {
  bookId: string;
  seriesId: string;
  title: string;
  cover: string;
  firstVid: string;
  contentTypeCode: number;
  episodeCount: number;
  durationSeconds?: number;
  abstract: string;
  score: string;
  category: string;
  categoryTags?: string[];
  releaseType?: NewReleaseType;
  author: string;
  rankTags: Array<{ label: string; schema: string }>;
  onlineTime?: number;
  playCount?: number;
  hotCount?: number;
  collectCount?: number;
  likeCount?: number;
  commentCount?: number;
};

export type CategoryGroup = {
  id: string;
  name: string;
  items: Array<{ id: string; name: string }>;
};

export type CategoryFilters = {
  background: string;
  topic: string;
  setting: string;
  gender: string;
  time: string;
  sort_type: string;
};

export type WebCategoryPage = {
  items: SeriesItem[];
  nextPage: number;
  hasMore: boolean;
  total?: number;
};

export type NewReleasePage = {
  items: SeriesItem[];
  nextCursor: string;
  hasMore: boolean;
  date: string;
  refreshedAt: string;
  source?: "subscribe" | "rank";
  dateScope?: "today" | "latest";
};

export type AppSettings = {
  version: 1 | 2 | 3 | 4;
  saveDir: string;
  definition: DefinitionPreference;
  notifyDownloadComplete: boolean;
  notifyNewReleases: boolean;
  notifyMediaComplete?: boolean;
  notifyYouTubeResult?: boolean;
  demucsModel: DemucsModel;
  whisperModel: WhisperModel;
  aiDevice: AIDevicePreference;
  aiConcurrency?: number;
  downloadProxy?: string;
  downloadMirror?: string;
  warning?: string;
};

export type AIComponentStatus = {
  id: string;
  version: string;
  installed: boolean;
  installedVersion: string | null;
  installedPath: string | null;
  downloadBytes: number;
  installedBytes: number;
  inUse: boolean;
  stage?: string;
  percent?: number;
};

export type AIComponentProgress = {
  id: string;
  stage: string;
  percent: number;
};

export type EpisodeItem = {
  index: number;
  itemId: string;
  title: string;
};

export type DiscoveryPage = {
  items: SeriesItem[];
  cellId: string;
  nextOffset: number;
  hasMore: boolean;
  sessionId: string;
  planId: string;
  filterIds: string;
  selectedItems: string;
  categories: Array<{ id: string; name: string; group: string }>;
  rankBoards: Array<{ label: string; schema: string }>;
};

export type RankPage = {
  sourceNote?: string;
  items: SeriesItem[];
  nextCursor: string;
  hasMore: boolean;
  board: string;
  boardName: string;
  releaseType: RankReleaseType;
  boards: Array<{ id: string; name: string }>;
};

export type SearchPage = {
  items: SeriesItem[];
  hasMore: boolean;
  nextOffset: number;
  nextPassback: string;
};
