import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MediaCommands, MediaJob, MediaJobsSnapshot, StartAIJobRequest, StartMergeRequest } from "./types";

export const mediaCommands: MediaCommands = {
  snapshot: () => invoke<MediaJobsSnapshot>("get_media_jobs"),
  startMerge: (request: StartMergeRequest) => invoke<MediaJob>("start_merge_job", { request }),
  startAudioSeparation: (request: StartAIJobRequest) => invoke<MediaJob>("start_audio_separation_job", { request }),
  startSubtitleExtraction: (request: StartAIJobRequest) => invoke<MediaJob>("start_subtitle_job", { request }),
  cancel: (jobId: string) => invoke<void>("cancel_media_job", { jobId, job_id: jobId }),
  pause: (jobId: string) => invoke<MediaJob>("pause_media_job", { jobId, job_id: jobId }),
  resume: (jobId: string) => invoke<MediaJob>("resume_media_job", { jobId, job_id: jobId }),
  deleteJob: (jobId: string) => invoke<void>("delete_media_job", { jobId, job_id: jobId }),
  hasMergedVideo: (seriesRoot: string) => invoke<boolean>("has_merged_video", { seriesRoot, series_root: seriesRoot }),
  retry: (jobId: string) => invoke<MediaJob>("retry_media_job", { jobId, job_id: jobId }),
  markNotified: (jobId, outcome) => invoke<MediaJob>("mark_media_job_notified", { jobId, outcome }),
  subscribeProgress: async (listener) => {
    const unlisten = await listen<MediaJob>("media-job-progress", (event) => listener(event.payload));
    return unlisten;
  },
};
