import { invoke } from "@tauri-apps/api/core";

export type ChannelAnalyticsSnapshot = {
  channelId: string;
  viewCount: string;
  fetchedAt: string;
};

export type AnalyticsRow = {
  date: string;
  views: number;
  estimatedMinutesWatched: number;
  averageViewDuration: number;
};

export type AnalyticsReport = {
  channelId: string;
  startDate: string;
  endDate: string;
  returnedEndDate: string | null;
  views: number;
  estimatedMinutesWatched: number;
  averageViewDuration: number;
  rows: AnalyticsRow[];
  fetchedAt: string;
  timezone: string;
};

export type AnalyticsCommands = {
  snapshot(channelId: string): Promise<ChannelAnalyticsSnapshot>;
  report(channelId: string, startDate: string, endDate: string): Promise<AnalyticsReport>;
};

export const analyticsCommands: AnalyticsCommands = {
  snapshot: (channelId) => invoke("get_youtube_channel_analytics_snapshot", { channelId }),
  report: (channelId, startDate, endDate) => invoke("get_youtube_channel_analytics_report", { channelId, startDate, endDate }),
};
