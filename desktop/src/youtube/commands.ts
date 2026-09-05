import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { YouTubeChannel, YouTubeCommands, YouTubeCredential, YouTubeJob, YouTubeSnapshot, YouTubeUploadIntent } from "./types";

export const youtubeCommands: YouTubeCommands = {
  snapshot: () => invoke<YouTubeSnapshot>("get_youtube_snapshot"),
  importCredential: (path) => invoke<YouTubeCredential>("import_youtube_oauth_config", { path }),
  authorize: () => invoke<YouTubeChannel>("authorize_youtube"),
  setChannel: (channelId) => invoke<YouTubeSnapshot>("set_youtube_channel", { channelId }),
  revoke: (channelId) => invoke<YouTubeSnapshot>("revoke_youtube", { channelId }),
  removeCredential: () => invoke<YouTubeSnapshot>("remove_youtube_oauth_config"),
  startUpload: (request: YouTubeUploadIntent) => invoke<YouTubeJob>("start_youtube_upload_job", { request }),
  cancel: (jobId) => invoke<void>("cancel_youtube_upload_job", { jobId }),
  retry: (jobId) => invoke<YouTubeJob>("retry_youtube_upload_job", { jobId }),
  retryThumbnail: (jobId) => invoke<YouTubeJob>("retry_youtube_thumbnail", { jobId }),
  markNotified: (jobId, outcome) => invoke<YouTubeJob>("mark_youtube_job_notified", { jobId, outcome }),
  subscribeProgress: async (listener) => listen<YouTubeJob>("youtube-job-progress", (event) => listener(event.payload)),
};
