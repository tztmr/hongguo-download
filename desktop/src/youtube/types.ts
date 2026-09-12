export type YouTubeUploadFormat = "auto" | "shorts" | "standard";
export type YouTubePrivacy = "private" | "unlisted" | "public";

export type YouTubeCredential = {
  configured: boolean;
  clientIdSuffix: string;
};

export type YouTubeChannel = {
  channelId: string;
  title: string;
  authorizedAt: string;
};

export type YouTubeJobStatus =
  | "queued"
  | "pausing"
  | "paused"
  | "preparingAuthorization"
  | "creatingSession"
  | "uploading"
  | "waitingToRetry"
  | "processing"
  | "settingThumbnail"
  | "uploadingSubtitles"
  | "videoUploadedSubtitleFailed"
  | "completed"
  | "videoUploadedThumbnailFailed"
  | "failed"
  | "cancelled";

export type YouTubeJob = {
  id: string;
  title: string;
  channelId: string;
  sourcePath: string;
  status: YouTubeJobStatus;
  uploadedBytes: number;
  totalBytes: number;
  percent: number;
  errorCode: string | null;
  errorMessage: string | null;
  videoId: string | null;
  youtubeUrl: string | null;
  actualPrivacyStatus: YouTubePrivacy | null;
  thumbnailState: "pending" | "succeeded" | "skipped" | "failed";
  subtitleState?: "skipped" | "pending" | "submitted" | "failed";
  subtitleError?: string | null;
  completionNotifiedAt?: number;
  failureNotifiedAt?: number;
};

export type YouTubeSnapshot = {
  credential: YouTubeCredential;
  channels: YouTubeChannel[];
  activeChannelId: string | null;
  jobs: YouTubeJob[];
};

export type YouTubeDuplicateQuery = { channelId: string; title: string; bookId: string; dramaTitle: string };
export type YouTubeDuplicateMatch = { title: string; videoId: string; youtubeUrl: string; reason: "sameTitle" | "sameDrama" | "similarTitle" };

export type YouTubeUploadIntent = {
  uploadFormat?: YouTubeUploadFormat;
  dedup?: { channelId: string; bookId: string; dramaTitle: string; allowDuplicate: boolean };
  jobId: string;
  filePath: string;
  coverPath: string | null;
  subtitle?: { path: string | null; language: string } | null;
  title: string;
  description: string;
  tags: string[];
  categoryId: string;
  privacyStatus: YouTubePrivacy;
  selfDeclaredMadeForKids: boolean;
  containsSyntheticMedia: boolean;
  hasPaidProductPlacement: boolean;
  audienceConfirmed: boolean;
  syntheticMediaConfirmed: boolean;
  publishConfirmed: boolean;
};

export type YouTubeError = { code: string; message: string };

export type YouTubeCommands = {
  snapshot(): Promise<YouTubeSnapshot>;
  importCredential(path: string): Promise<YouTubeCredential>;
  authorize(): Promise<YouTubeChannel>;
  setChannel(channelId: string): Promise<YouTubeSnapshot>;
  revoke(channelId: string): Promise<YouTubeSnapshot>;
  removeCredential(): Promise<YouTubeSnapshot>;
  startUpload(request: YouTubeUploadIntent): Promise<YouTubeJob>;
  cancel(jobId: string): Promise<void>;
  pause(jobId: string): Promise<YouTubeJob>;
  resume(jobId: string): Promise<YouTubeJob>;
  retry(jobId: string): Promise<YouTubeJob>;
  retryThumbnail(jobId: string): Promise<YouTubeJob>;
  uploadSubtitle(jobId: string, request: YouTubeUploadIntent["subtitle"]): Promise<YouTubeJob>;
  removeJob(jobId: string): Promise<void>;
  markNotified?(jobId: string, outcome: "success" | "failure"): Promise<YouTubeJob>;
  subscribeProgress(listener: (job: YouTubeJob) => void): Promise<() => void>;
};

export type YouTubeModel = YouTubeSnapshot & {
  loading: boolean;
  busy: boolean;
  error?: YouTubeError;
  importCredential(path: string): Promise<void>;
  authorize(): Promise<void>;
  setChannel(channelId: string): Promise<void>;
  revoke(channelId: string): Promise<void>;
  removeCredential(): Promise<void>;
  startUpload(request: YouTubeUploadIntent): Promise<YouTubeJob>;
  cancel(jobId: string): Promise<void>;
  pause(jobId: string): Promise<void>;
  resume(jobId: string): Promise<void>;
  retry(jobId: string): Promise<void>;
  retryThumbnail(jobId: string): Promise<void>;
  uploadSubtitle(jobId: string, request: YouTubeUploadIntent["subtitle"]): Promise<void>;
  removeJob(jobId: string): Promise<void>;
  markNotified?(jobId: string, outcome: "success" | "failure"): Promise<void>;
};
