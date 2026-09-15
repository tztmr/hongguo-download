import type { DownloadAdapter } from "./download/useDownloadManager";
import type { DownloadBatch, DownloadItem, DownloadItemStatus, DownloadManagerState } from "./download/model";
import type { MediaCommands, MediaJob } from "./media/types";
import type { AIComponentStatus, EpisodeItem, SeriesItem } from "./types";
import type { YouTubeModel } from "./youtube/types";
import type { AnalyticsCommands, AnalyticsMetrics, AnalyticsRow, BreakdownRow } from "./youtube/analyticsCommands";
import type { ManagedVideo, ManagementCommands } from "./youtube/managementCommands";

const names = [
  "天下第一纨绔",
  "女子爱财，取之有道",
  "系统逼我当暴君",
  "闪婚后大佬每天都在追",
  "今日宜偏爱",
  "重生后我成了首富",
  "穿成反派师尊的白月光",
  "二嫁矿霸有点甜",
];

export const previewSeries: SeriesItem[] = Array.from({ length: 40 }, (_, index) => ({
  bookId: `preview-book-${index + 1}`,
  seriesId: `preview-book-${index + 1}`,
  title: index < names.length ? names[index] : `${names[index % names.length]} · ${Math.floor(index / names.length) + 1}`,
  cover: "",
  firstVid: "",
  contentTypeCode: 1,
  episodeCount: [59, 76, 80, 80, 68, 80, 72, 71][index % names.length],
  abstract: index === 0 ? "纨绔少爷逆势归来，在乱世中守护家人与信念。" : "热门真人短剧，剧情紧凑，全集可下载。",
  score: ["9.2", "8.9", "9.0", "8.7", "8.8", "8.6", "8.9", "8.3"][index % names.length],
  category: "真人剧",
  categoryTags: [["校园", "青春"], ["古风", "古装"], ["都市"]][index % 3],
  releaseType: "playlet",
  author: "红果短剧",
  rankTags: index < 3 ? [{ label: index === 0 ? "热播榜 TOP 1" : "本周热播", schema: "" }] : [],
  onlineTime: Math.floor(Date.parse(`2026-09-02T${String(8 + (index % 10)).padStart(2, "0")}:00:00+08:00`) / 1000),
  playCount: index === 0 ? 0 : 12_000 + index * 937,
  hotCount: 25_000 + index * 113,
  collectCount: index % 5 === 0 ? undefined : 800 + index * 17,
  likeCount: 1_200 + index * 23,
  commentCount: 320 + index * 17,
}));

export const previewEpisodes: EpisodeItem[] = Array.from({ length: 59 }, (_, index) => ({
  index: index + 1,
  itemId: `preview-episode-${index + 1}`,
  title: `第 ${index + 1} 集`,
}));

function item(batchId: string, index: number, status: DownloadItemStatus): DownloadItem {
  const percent = status === "done" ? 100 : status === "running" ? 36 + (index % 4) * 13 : 0;
  return {
    id: `${batchId}-item-${index}`,
    itemId: `${batchId}-source-${index}`,
    episodeIndex: index,
    episodeTitle: `第 ${index} 集`,
    definition: "1080p",
    status,
    percent,
    received: Math.round(percent * 1024 * 128),
    total: status === "queued" || status === "error" ? undefined : 100 * 1024 * 128,
    path: status === "done" ? `/Users/edking/Downloads/红果下载/${batchId}/第 ${index} 集_1080p.mp4` : undefined,
    error: status === "error" ? "上游连接中断，请重试" : undefined,
    addedAt: 1788320000000 + index,
    completedAt: status === "done" ? 1788322000000 + index : undefined,
  };
}

function batch(
  id: string,
  series: SeriesItem,
  counts: { running: number; queued: number; done: number; error: number },
): DownloadBatch {
  const statuses: DownloadItemStatus[] = [];
  const remaining = { ...counts };
  for (const status of ["running", "queued", "error", "done"] as DownloadItemStatus[]) {
    if (remaining[status] > 0) {
      statuses.push(status);
      remaining[status] -= 1;
    }
  }
  for (const status of ["done", "running", "queued", "error"] as DownloadItemStatus[]) {
    statuses.push(...Array.from({ length: remaining[status] }, () => status));
  }
  return {
    id,
    bookId: series.bookId,
    title: series.title,
    cover: series.cover,
    series: {
      bookId: series.bookId,
      seriesId: series.seriesId,
      title: series.title,
      cover: series.cover,
      abstract: series.abstract,
      category: series.category,
      categoryTags: series.categoryTags,
      contentTypeCode: series.contentTypeCode,
    },
    paused: false,
    items: statuses.map((status, index) => item(id, index + 1, status)),
    createdAt: 1788320000000,
    updatedAt: 1788323000000,
  };
}

export function createPreviewDownloadState(): DownloadManagerState {
  return {
    version: 2,
    concurrency: 5,
    globallyPaused: false,
    batches: [
      batch("preview-batch-a", previewSeries[0], { running: 2, queued: 5, done: 51, error: 1 }),
      batch("preview-batch-b", previewSeries[1], { running: 2, queued: 8, done: 65, error: 1 }),
      batch("preview-batch-c", previewSeries[2], { running: 1, queued: 3, done: 26, error: 0 }),
      batch("preview-batch-complete", previewSeries[3], { running: 0, queued: 0, done: 8, error: 0 }),
    ],
  };
}

export const previewDownloadAdapter: DownloadAdapter = {
  download: async (args) => ({ taskId: args.taskId, path: `/Downloads/${args.episodeTitle}.mp4`, definition: args.definition, bytes: 12 * 1024 * 1024 }),
  subscribeProgress: async () => () => undefined,
};

export function createMemoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

export const previewMediaJobs: MediaJob[] = [
  {
    id: "preview-media-queued",
    dedupeKey: "preview-queued",
    mergeRequest: { title: "系统逼我当暴君" },
    kind: "merge",
    status: "queued",
    stage: "queued",
    percent: 0,
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-c/第 1 集_1080p.mp4", sizeBytes: 12 }],
    outputPath: null,
    errorCode: null,
    errorMessage: null,
  },
  {
    id: "preview-media-running",
    dedupeKey: "preview-running",
    mergeRequest: { title: "系统逼我当暴君" },
    kind: "merge",
    status: "running",
    stage: "merging",
    percent: 58,
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-c/第 1 集_1080p.mp4", sizeBytes: 12 }],
    outputPath: null,
    errorCode: null,
    errorMessage: null,
  },
  {
    id: "preview-media-completed",
    dedupeKey: "preview-completed",
    mergeRequest: { title: "系统逼我当暴君" },
    kind: "merge",
    status: "completed",
    stage: "completed",
    percent: 100,
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-c/第 1 集_1080p.mp4", sizeBytes: 12 }],
    outputPath: "/Users/edking/Downloads/红果下载/preview-batch-c/合并视频/系统逼我当暴君.mp4",
    errorCode: null,
    errorMessage: null,
  },
  {
    id: "preview-media-failed",
    dedupeKey: "preview-failed",
    kind: "merge",
    status: "failed",
    stage: "failed",
    percent: 0,
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-complete/第 1 集_1080p.mp4", sizeBytes: 12 }],
    outputPath: null,
    errorCode: "FFMPEG_FAILED",
    errorMessage: "合并失败，可重试",
  },
  {
    id: "preview-media-upload-source",
    dedupeKey: "preview-upload-source",
    kind: "merge",
    status: "completed",
    stage: "completed",
    percent: 100,
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-complete/第 1 集_1080p.mp4", sizeBytes: 12 }],
    outputPath: "/Users/edking/Downloads/红果下载/preview-batch-complete/合并视频/闪婚后大佬每天都在追.mp4",
    errorCode: null,
    errorMessage: null,
    completionNotifiedAt: 1,
  },
  {
    id: "preview-media-clean-source", dedupeKey: "preview-clean-source", kind: "separateBackgroundMusic",
    status: "completed", stage: "completed", percent: 100,
    aiRequest: { title: "闪婚后大佬每天都在追", scope: "merged", model: "htdemucs" },
    inputs: [{ path: "/Users/edking/Downloads/红果下载/preview-batch-complete/合并视频/闪婚后大佬每天都在追.mp4", sizeBytes: 12 }],
    outputPath: "/Users/edking/Downloads/红果下载/preview-batch-complete/音频分离/去背景音乐.mp4",
    outputs: [{ episodeIndex: 1, kind: "noBackgroundMusicVideo", path: "/Users/edking/Downloads/红果下载/preview-batch-complete/音频分离/去背景音乐.mp4" }],
    errorCode: null, errorMessage: null, completionNotifiedAt: 1,
  },
];

export const previewYouTubeModel: YouTubeModel = {
  credential: { configured: true, clientIdSuffix: "…PREVIEW" },
  channels: [{ channelId: "UC_PREVIEW", title: "预览频道", authorizedAt: "2026-09-04T00:00:00Z" }],
  activeChannelId: "UC_PREVIEW",
  loading: false,
  busy: false,
  jobs: [
    {
      id: "preview-upload-running", title: "系统逼我当暴君", channelId: "UC_PREVIEW", sourcePath: "/Preview/merged.mp4",
      status: "uploading", uploadedBytes: 4.2 * 1024 ** 3, totalBytes: 10 * 1024 ** 3, percent: 42, errorCode: null, errorMessage: null,
      videoId: null, youtubeUrl: null, actualPrivacyStatus: null, thumbnailState: "pending",
    },
    {
      id: "preview-upload-private", title: "闪婚后大佬每天都在追", channelId: "UC_PREVIEW", sourcePath: "/Preview/private.mp4",
      status: "completed", uploadedBytes: 8 * 1024 ** 3, totalBytes: 8 * 1024 ** 3, percent: 100, errorCode: null, errorMessage: null,
      videoId: "preview-private", youtubeUrl: "https://www.youtube.com/watch?v=preview-private", actualPrivacyStatus: "private", thumbnailState: "succeeded", completionNotifiedAt: 1,
    },
    {
      id: "preview-thumbnail-failed", title: "今日宜偏爱", channelId: "UC_PREVIEW", sourcePath: "/Preview/partial.mp4",
      status: "videoUploadedThumbnailFailed", uploadedBytes: 8 * 1024 ** 3, totalBytes: 8 * 1024 ** 3, percent: 100, errorCode: "THUMBNAIL_FORBIDDEN", errorMessage: "频道暂无自定义封面权限",
      videoId: "preview-partial", youtubeUrl: "https://www.youtube.com/watch?v=preview-partial", actualPrivacyStatus: "private", thumbnailState: "failed", failureNotifiedAt: 1,
    },
  ],
  importCredential: async () => undefined,
  authorize: async () => undefined,
  setChannel: async () => undefined,
  revoke: async () => undefined,
  removeCredential: async () => undefined,
  startUpload: async () => previewYouTubeModel.jobs[0],
  cancel: async () => undefined,
  pause: async () => undefined,
  resume: async () => undefined,
  retry: async () => undefined,
  retryThumbnail: async () => undefined,
  uploadSubtitle: async () => undefined,
  removeJob: async () => undefined,
  markNotified: async () => undefined,
};

// Synthetic channel data for the browser preview; no remote mutation is made.
let previewManagedVideos: ManagedVideo[] = names.slice(0, 5).map((title, index) => ({
  id: `demoVideo0${index}`, etag: `preview-${index}`, title: `${title}${index % 2 ? " · 首集 Shorts" : " · 全集"}`,
  description: "演示频道视频，用于检查管理界面。", privacyStatus: index === 4 ? "private" : "public", thumbnailUrl: "", publishedAt: "2026-09-15T00:00:00Z",
  videoFormat: index % 2 ? "shorts" : "standard", durationSeconds: index % 2 ? 90 : 3600,
  restriction: { kind: index === 1 ? "global" : index === 2 ? "region" : "noneReported", reason: index === 1 ? "全球封锁" : index === 2 ? "地区限制" : "API 未返回封锁信息", allowedRegions: index === 1 ? [] : null, blockedRegions: index === 2 ? ["US", "CA"] : [] },
}));
export const previewManagementCommands: ManagementCommands = {
  detail: async (_channelId, videoId) => { const video = previewManagedVideos.find((row) => row.id === videoId); if (!video) throw new Error("演示视频不存在"); return video; },
  list: async () => ({ items: [...previewManagedVideos], nextPageToken: null }),
  lookup: async (_channelId, videoIds) => ({ items: previewManagedVideos.filter((video) => videoIds.includes(video.id)), failures: videoIds.filter((id) => !previewManagedVideos.some((video) => video.id === id)).map((videoId) => ({ videoId, message: "演示数据中没有此视频" })) }),
  deleteVideo: async (_channelId, videoId) => { previewManagedVideos = previewManagedVideos.filter((video) => video.id !== videoId); },
  update: async (request) => { const index = previewManagedVideos.findIndex((video) => video.id === request.videoId); if (index < 0) throw new Error("演示视频不存在"); previewManagedVideos[index] = { ...previewManagedVideos[index], ...request }; return previewManagedVideos[index]; },
  thumbnail: async () => undefined,
  playlists: async () => [],
  membership: async () => undefined,
  createPlaylist: async () => ({ id: "", title: "", privacyStatus: "private", itemIds: [] }),
};

const emptyAnalytics: AnalyticsMetrics = {
  views: null, engagedViews: null, estimatedMinutesWatched: null, averageViewDuration: null,
  averageViewPercentage: null, likes: null, comments: null, shares: null, subscribersGained: null, subscribersLost: null,
};
const previewAnalyticsVideos = names.slice(0, 5).map((title, index) => ({
  key: `demoVideo0${index}`, title: `${title}${index % 2 ? " · 首集 Shorts" : " · 全集"}`,
  contentType: index % 2 ? "SHORTS" : "VIDEO_ON_DEMAND", thumbnailUrl: null,
}));
function previewDate(date: string, days: number): string {
  return new Date(Date.parse(`${date}T00:00:00Z`) + days * 86400000).toISOString().slice(0, 10);
}
function previewDaily(startDate: string, endDate: string, videoId?: string): AnalyticsRow[] {
  const count = Math.min(367, (Date.parse(endDate) - Date.parse(startDate)) / 86400000 + 1);
  return Array.from({ length: Math.max(0, count) }, (_, index) => {
    const date = previewDate(startDate, index), day = Date.parse(date) / 86400000;
    const factor = videoId ? (Number(videoId.slice(-1)) + 1) / 15 : 1;
    const views = Math.round((1800 + (day % 11) * 190 + Math.sin(day) * 380) * factor);
    const engagedViews = Math.round(views * 0.7), averageViewDuration = 65 + day % 30;
    return { date, views, engagedViews, averageViewDuration, estimatedMinutesWatched: engagedViews * averageViewDuration / 60,
      averageViewPercentage: videoId && Number(videoId.slice(-1)) % 2 ? 108.5 : 42.8,
      likes: Math.round(views * 0.035), comments: Math.round(views * 0.004), shares: Math.round(views * 0.008),
      subscribersGained: Math.round((day % 7 === 0 ? 2 : views * 0.006)), subscribersLost: Math.round(views * 0.0015),
    };
  });
}
function previewTotals(rows: AnalyticsMetrics[]): AnalyticsMetrics {
  if (!rows.length) return { ...emptyAnalytics };
  const sum = (key: keyof AnalyticsMetrics) => rows.reduce((total, row) => total + (row[key] ?? 0), 0);
  const engagedViews = sum("engagedViews"), minutes = sum("estimatedMinutesWatched");
  return { views: sum("views"), engagedViews, estimatedMinutesWatched: minutes,
    averageViewDuration: engagedViews ? minutes * 60 / engagedViews : null,
    averageViewPercentage: engagedViews ? rows.reduce((total, row) => total + (row.averageViewPercentage ?? 0) * (row.engagedViews ?? 0), 0) / engagedViews : null,
    likes: sum("likes"), comments: sum("comments"), shares: sum("shares"), subscribersGained: sum("subscribersGained"), subscribersLost: sum("subscribersLost"),
  };
}
export const previewAnalyticsCommands: AnalyticsCommands = {
  snapshot: async (channelId) => ({ channelId, viewCount: "123456", subscriberCount: "2340", hiddenSubscriberCount: false, videoCount: "58", fetchedAt: new Date().toISOString() }),
  report: async (channelId, startDate, endDate, videoId) => {
    const cutoff = previewDate(new Date().toISOString().slice(0, 10), -2);
    const returnedEndDate = startDate <= cutoff ? (endDate < cutoff ? endDate : cutoff) : null;
    const rows = returnedEndDate ? previewDaily(startDate, returnedEndDate, videoId) : [];
    const previousStart = previewDate(startDate, -rows.length), previousEnd = previewDate(startDate, -1);
    return { channelId, videoId: videoId ?? null, startDate, endDate, returnedEndDate,
      ...previewTotals(rows), rows, warnings: [], fetchedAt: new Date().toISOString(), timezone: "America/Los_Angeles",
      comparison: rows.length ? { startDate: previousStart, endDate: previousEnd, ...previewTotals(previewDaily(previousStart, previousEnd, videoId)) } : null,
    };
  },
  breakdown: async (channelId, startDate, endDate, kind, videoId) => {
    const total = previewTotals(previewDaily(startDate, endDate, videoId));
    let rows: BreakdownRow[];
    if (kind === "videos") rows = previewAnalyticsVideos.map((video) => ({ ...video, ...previewTotals(previewDaily(startDate, endDate, video.key)) }));
    else {
      const keys = { contentType: ["SHORTS", "VIDEO_ON_DEMAND"], traffic: ["SHORTS", "YT_SEARCH", "RELATED_VIDEO", "EXT_URL", "NOTIFICATION"],
        country: ["TW", "US", "MY", "SG", "CA"], device: ["MOBILE", "TV", "DESKTOP", "TABLET"], subscribed: ["UNSUBSCRIBED", "SUBSCRIBED"] }[kind];
      const weights = keys.map((_, i) => keys.length - i), denominator = weights.reduce((sum, n) => sum + n, 0);
      rows = keys.map((key, index) => ({ ...emptyAnalytics, key, title: null, thumbnailUrl: null, contentType: kind === "contentType" ? key : null,
        views: Math.round((total.views ?? 0) * weights[index] / denominator), engagedViews: Math.round((total.engagedViews ?? 0) * weights[index] / denominator),
        estimatedMinutesWatched: (total.estimatedMinutesWatched ?? 0) * weights[index] / denominator,
        averageViewDuration: kind === "traffic" || kind === "device" ? null : total.averageViewDuration,
        averageViewPercentage: kind === "traffic" || kind === "device" ? null : total.averageViewPercentage,
      }));
    }
    return { channelId, videoId: videoId ?? null, startDate, endDate, kind, rows, truncated: false, warnings: [], fetchedAt: new Date().toISOString() };
  },
};

export const previewMediaCommands: MediaCommands = {
  snapshot: async () => ({ version: 1, jobs: previewMediaJobs, warning: null }),
  startMerge: async () => previewMediaJobs[0],
  startAudioSeparation: async () => previewMediaJobs[0],
  startSubtitleExtraction: async () => previewMediaJobs[0],
  cancel: async () => undefined,
  pause: async () => previewMediaJobs[0],
  resume: async () => previewMediaJobs[0],
  deleteJob: async () => undefined,
  hasMergedVideo: async () => previewMediaJobs.some((job) => job.kind === "merge" && job.status === "completed"),
  findMergedVideo: async () => previewMediaJobs.find((job) => job.kind === "merge" && job.status === "completed")?.outputPath ?? null,
  retry: async () => previewMediaJobs[3] ?? previewMediaJobs[0],
  subscribeProgress: async () => () => undefined,
};

// Local preview fixtures; these actions never download model files.
export const previewAIComponents: AIComponentStatus[] = [
  {
    "id": "runtime",
    "version": "2",
    "downloadBytes": 156922102,
    "installedBytes": 510405736,
    "installed": true,
    "installedVersion": "2",
    "installedPath": "/Preview/components/runtime",
    "inUse": false
  },
  {
    "id": "demucs-htdemucs",
    "version": "4.0.1",
    "downloadBytes": 77960165,
    "installedBytes": 84141932,
    "installed": true,
    "installedVersion": "4.0.1",
    "installedPath": "/Preview/components/demucs-htdemucs",
    "inUse": false
  },
  {
    "id": "whisper-small",
    "version": "20250625",
    "downloadBytes": 445457618,
    "installedBytes": 483617219,
    "installed": false,
    "installedVersion": null,
    "installedPath": null,
    "inUse": false
  },
  {
    "id": "demucs-htdemucs_ft",
    "version": "4.0.1",
    "downloadBytes": 311824838,
    "installedBytes": 336565233,
    "installed": false,
    "installedVersion": null,
    "installedPath": null,
    "inUse": false
  },
  {
    "id": "whisper-medium",
    "version": "20250625",
    "downloadBytes": 1411865773,
    "installedBytes": 1528008539,
    "installed": false,
    "installedVersion": null,
    "installedPath": null,
    "inUse": false
  }
];
