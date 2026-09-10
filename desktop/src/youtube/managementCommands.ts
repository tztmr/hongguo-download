import { invoke } from "@tauri-apps/api/core";
import type { YouTubePrivacy } from "./types";

export type ManagedVideo = { id: string; etag: string; title: string; description: string; privacyStatus: YouTubePrivacy; thumbnailUrl: string; publishedAt: string };
export type ManagedPlaylist = { id: string; title: string; privacyStatus: YouTubePrivacy; itemIds: string[] };
export type VideoUpdate = Pick<ManagedVideo, "etag" | "title" | "description" | "privacyStatus"> & { channelId: string; videoId: string };
export type ManagementCommands = {
  detail(channelId: string, videoId: string): Promise<ManagedVideo>;
  list(channelId: string, pageToken?: string): Promise<{ items: ManagedVideo[]; nextPageToken: string | null }>;
  update(request: VideoUpdate): Promise<ManagedVideo>;
  thumbnail(channelId: string, videoId: string, path: string): Promise<void>;
  playlists(channelId: string, videoId: string): Promise<ManagedPlaylist[]>;
  membership(channelId: string, videoId: string, playlistId: string, included: boolean): Promise<void>;
  createPlaylist(channelId: string, title: string, privacy: YouTubePrivacy): Promise<ManagedPlaylist>;
};
export const managementCommands: ManagementCommands = {
  detail: (channelId, videoId) => invoke("get_youtube_channel_video", { channelId, videoId }),
  list: (channelId, pageToken) => invoke("list_youtube_channel_videos", { channelId, pageToken: pageToken ?? null }),
  update: (request) => invoke("update_youtube_channel_video", { request }),
  thumbnail: (channelId, videoId, path) => invoke("set_youtube_video_thumbnail", { channelId, videoId, path }),
  playlists: (channelId, videoId) => invoke("list_youtube_video_playlists", { channelId, videoId }),
  membership: (channelId, videoId, playlistId, included) => invoke("set_youtube_video_playlist", { channelId, videoId, playlistId, included }),
  createPlaylist: (channelId, title, privacy) => invoke("create_youtube_playlist", { channelId, title, privacy }),
};
