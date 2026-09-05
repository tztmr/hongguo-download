import type { DownloadBatch } from "../download/model";

export type MediaJobStatus =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export type MediaJobKind = "merge" | "separateBackgroundMusic" | "extractSubtitles";
export type MediaJobScope = "episodes" | "merged";
export type AIModel = "htdemucs" | "htdemucs_ft" | "small" | "medium";

export type MergeConflictPolicy = "failIfExists" | "overwrite";

export type StartMergeInput = {
  episodeIndex: number;
  path: string;
};

export type StartMergeRequest = {
  bookId: string;
  title: string;
  seriesRoot: string;
  outputFileName: string;
  inputs: StartMergeInput[];
  transcodeH264: boolean;
  conflictPolicy: MergeConflictPolicy;
};

export type StartAIJobRequest = {
  bookId: string;
  title: string;
  seriesRoot: string;
  scope: MediaJobScope;
  inputs: StartMergeInput[];
  model: AIModel;
};

export type MediaJobOutput = {
  episodeIndex: number;
  kind: "vocals" | "backgroundMusic" | "noBackgroundMusicVideo" | "subtitles";
  path: string;
  source?: "embeddedText" | "whisperOriginalAudio" | "whisperVocals";
};

export type MediaJob = {
  id: string;
  dedupeKey: string;
  kind: MediaJobKind;
  status: MediaJobStatus;
  stage: string;
  percent: number;
  inputs: Array<{ path: string; sizeBytes: number }>;
  outputPath: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  outputs?: MediaJobOutput[];
  completionNotifiedAt?: number;
  failureNotifiedAt?: number;
};

export type MediaJobsSnapshot = {
  version: number;
  jobs: MediaJob[];
  warning: { code: string; message: string } | null;
};

export type MediaCommandError = {
  code: string;
  message: string;
};

export type MergeSubmitOptions = {
  outputName: string;
  transcodeH264: boolean;
  conflictPolicy: MergeConflictPolicy;
};

export type MediaCommands = {
  snapshot(): Promise<MediaJobsSnapshot>;
  startMerge(request: StartMergeRequest): Promise<MediaJob>;
  startAudioSeparation(request: StartAIJobRequest): Promise<MediaJob>;
  startSubtitleExtraction(request: StartAIJobRequest): Promise<MediaJob>;
  cancel(jobId: string): Promise<unknown>;
  retry(jobId: string): Promise<MediaJob>;
  markNotified?(jobId: string, outcome: "success" | "failure"): Promise<MediaJob>;
  subscribeProgress(listener: (job: MediaJob) => void): Promise<() => void>;
};

export type MediaJobsModel = {
  jobs: MediaJob[];
  warning?: string;
  error?: MediaCommandError;
  startMerge(batch: DownloadBatch, options: MergeSubmitOptions): Promise<MediaJob>;
  startAudioSeparation(batch: DownloadBatch, scope: MediaJobScope, model: "htdemucs" | "htdemucs_ft", mergedPath?: string): Promise<MediaJob>;
  startSubtitleExtraction(batch: DownloadBatch, scope: MediaJobScope, model: "small" | "medium", mergedPath?: string): Promise<MediaJob>;
  cancel(jobId: string): Promise<void>;
  retry(jobId: string): Promise<void>;
  markNotified?(jobId: string, outcome: "success" | "failure"): Promise<void>;
};
