import { invoke } from "@tauri-apps/api/core";
import type { YouTubePrivacy } from "./types";

export type ManagedVideo = {
  id: string; etag: string; title: string; description: string;
  privacyStatus: YouTubePrivacy; thumbnailUrl: string; publishedAt: string;
  videoFormat: "shorts" | "shortsCandidate" | "standard" | "unknown";
  durationSeconds: number | null;
  restriction: {
    kind: "noneReported" | "global" | "region" | "copyright" | "unavailable";
    reason: string; allowedRegions: string[] | null; blockedRegions: string[];
  };
};
export type ManagedPlaylist = { id: string; title: string; privacyStatus: YouTubePrivacy; itemIds: string[] };
export type VideoUpdate = Pick<ManagedVideo, "etag" | "title" | "description" | "privacyStatus"> & { channelId: string; videoId: string };
export type VideoAiDraft = { video: ManagedVideo; title: string | null; description: string | null; image: string | null; model: string };
export type ManagementCommands = {
  detail(channelId: string, videoId: string): Promise<ManagedVideo>;
  list(channelId: string, pageToken?: string): Promise<{ items: ManagedVideo[]; nextPageToken: string | null }>;
  lookup(channelId: string, videoIds: string[]): Promise<{ items: ManagedVideo[]; failures: { videoId: string; message: string }[] }>;
  deleteVideo(channelId: string, videoId: string): Promise<void>;
  makeBlockedVideoPrivate(channelId: string, videoId: string): Promise<ManagedVideo>;
  setPrivacy(channelId: string, videoId: string, privacy: YouTubePrivacy): Promise<ManagedVideo>;
  generateAi(channelId: string, videoId: string, etag: string, kind: "text" | "cover"): Promise<VideoAiDraft>;
  generatedThumbnail(channelId: string, videoId: string, etag: string, image: string): Promise<void>;
  update(request: VideoUpdate): Promise<ManagedVideo>;
  thumbnail(channelId: string, videoId: string, path: string): Promise<void>;
  playlists(channelId: string, videoId: string): Promise<ManagedPlaylist[]>;
  membership(channelId: string, videoId: string, playlistId: string, included: boolean): Promise<void>;
  createPlaylist(channelId: string, title: string, privacy: YouTubePrivacy): Promise<ManagedPlaylist>;
};
export const managementCommands: ManagementCommands = {
  lookup: (channelId, videoIds) => invoke("lookup_youtube_channel_videos", { channelId, videoIds }),
  deleteVideo: (channelId, videoId) => invoke("delete_youtube_channel_video", { channelId, videoId }),
  makeBlockedVideoPrivate: (channelId, videoId) => invoke("make_youtube_blocked_video_private", { channelId, videoId }),
  setPrivacy: (channelId, videoId, privacy) => invoke("set_youtube_video_privacy", { channelId, videoId, privacy }),
  generateAi: (channelId, videoId, etag, kind) => invoke("generate_youtube_video_ai", { channelId, videoId, etag, kind }),
  generatedThumbnail: (channelId, videoId, etag, image) => invoke("set_youtube_generated_thumbnail", { channelId, videoId, etag, image }),
  detail: (channelId, videoId) => invoke("get_youtube_channel_video", { channelId, videoId }),
  list: (channelId, pageToken) => invoke("list_youtube_channel_videos", { channelId, pageToken: pageToken ?? null }),
  update: (request) => invoke("update_youtube_channel_video", { request }),
  thumbnail: (channelId, videoId, path) => invoke("set_youtube_video_thumbnail", { channelId, videoId, path }),
  playlists: (channelId, videoId) => invoke("list_youtube_video_playlists", { channelId, videoId }),
  membership: (channelId, videoId, playlistId, included) => invoke("set_youtube_video_playlist", { channelId, videoId, playlistId, included }),
  createPlaylist: (channelId, title, privacy) => invoke("create_youtube_playlist", { channelId, title, privacy }),
};
