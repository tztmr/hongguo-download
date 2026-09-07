import type { DownloadAdapter } from "./download/useDownloadManager";
import type { DownloadBatch, DownloadItem, DownloadItemStatus, DownloadManagerState } from "./download/model";
import type { MediaCommands, MediaJob } from "./media/types";
import type { AIComponentStatus, EpisodeItem, SeriesItem } from "./types";
import type { YouTubeModel } from "./youtube/types";

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
  markNotified: async () => undefined,
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
