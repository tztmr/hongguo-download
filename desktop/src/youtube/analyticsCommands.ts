import { invoke } from "@tauri-apps/api/core";

export type ChannelAnalyticsSnapshot = {
  channelId: string; viewCount: string; subscriberCount: string | null;
  hiddenSubscriberCount: boolean; videoCount: string | null; fetchedAt: string;
};
export type AnalyticsMetrics = {
  views: number | null; engagedViews: number | null;
  estimatedMinutesWatched: number | null; averageViewDuration: number | null; averageViewPercentage: number | null;
  likes: number | null; comments: number | null; shares: number | null;
  subscribersGained: number | null; subscribersLost: number | null;
};
export type AnalyticsRow = AnalyticsMetrics & { date: string };
export type AnalyticsComparison = AnalyticsMetrics & { startDate: string; endDate: string };
export type AnalyticsWarning = { code: string; message: string };
export type AnalyticsReport = AnalyticsMetrics & {
  channelId: string; videoId: string | null; startDate: string; endDate: string; returnedEndDate: string | null;
  comparison: AnalyticsComparison | null; warnings: AnalyticsWarning[];
  rows: AnalyticsRow[]; fetchedAt: string; timezone: string;
};
export type BreakdownKind = "videos" | "contentType" | "traffic" | "country" | "device" | "subscribed";
export type BreakdownRow = AnalyticsMetrics & { key: string; title: string | null; thumbnailUrl: string | null; contentType: string | null };
export type AnalyticsBreakdown = {
  channelId: string; videoId: string | null; startDate: string; endDate: string; kind: BreakdownKind;
  rows: BreakdownRow[]; truncated: boolean; warnings: AnalyticsWarning[]; fetchedAt: string;
};
export type AnalyticsCommands = {
  snapshot(channelId: string): Promise<ChannelAnalyticsSnapshot>;
  report(channelId: string, startDate: string, endDate: string, videoId?: string): Promise<AnalyticsReport>;
  breakdown(channelId: string, startDate: string, endDate: string, kind: BreakdownKind, videoId?: string): Promise<AnalyticsBreakdown>;
};
export const analyticsCommands: AnalyticsCommands = {
  snapshot: (channelId) => invoke("get_youtube_channel_analytics_snapshot", { channelId }),
  report: (channelId, startDate, endDate, videoId) => invoke("get_youtube_channel_analytics_report", { channelId, startDate, endDate, videoId: videoId ?? null }),
  breakdown: (channelId, startDate, endDate, kind, videoId) => invoke("get_youtube_channel_analytics_breakdown", { channelId, startDate, endDate, kind, videoId: videoId ?? null }),
};
