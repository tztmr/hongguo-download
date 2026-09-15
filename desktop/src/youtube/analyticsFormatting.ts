import type { AnalyticsMetrics } from "./analyticsCommands";

export function integer(value: number | string | null | undefined): string {
  if (value == null) return "—";
  try { return BigInt(String(value)).toLocaleString("zh-CN"); }
  catch { return Number.isFinite(Number(value)) ? Number(value).toLocaleString("zh-CN") : "—"; }
}
export function decimal(value: number | null | undefined, digits = 1): string {
  return value != null && Number.isFinite(value) ? value.toLocaleString("zh-CN", { maximumFractionDigits: digits }) : "—";
}
export function hours(value: number | null | undefined): string { return value == null ? "—" : `${decimal(value / 60)} 小时`; }
export function percent(value: number | null | undefined): string { return value == null ? "—" : `${decimal(value)}%`; }
export function duration(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "—";
  const seconds = Math.round(value), minutes = Math.floor(seconds / 60), remainder = seconds % 60;
  return minutes ? `${minutes} 分 ${remainder} 秒` : `${remainder} 秒`;
}
export function netSubscribers(value: Pick<AnalyticsMetrics, "subscribersGained" | "subscribersLost">): number | null {
  return value.subscribersGained == null || value.subscribersLost == null ? null : value.subscribersGained - value.subscribersLost;
}
export function comparison(current: number | null | undefined, previous: number | null | undefined, difference = false): string {
  if (current == null || previous == null || !Number.isFinite(current) || !Number.isFinite(previous)) return "暂无可比数据";
  const change = current - previous;
  if (difference) return `较上期 ${change > 0 ? "+" : ""}${decimal(change)}`;
  if (previous === 0) return current === 0 ? "与上期持平" : "上期为 0";
  return `较上期 ${change > 0 ? "+" : ""}${decimal(change / Math.abs(previous) * 100)}%`;
}
export const contentLabels: Record<string, string> = { SHORTS: "Shorts", VIDEO_ON_DEMAND: "普通视频", LIVE_STREAM: "直播", STORY: "故事", UNSPECIFIED: "类型未知" };
const sourceLabels: Record<string, string> = {
  SHORTS: "Shorts 信息流", YT_SEARCH: "YouTube 搜索", RELATED_VIDEO: "推荐视频", SUBSCRIBER: "首页与订阅动态", EXT_URL: "外部网站与搜索", NOTIFICATION: "通知与邮件", PLAYLIST: "播放列表", YT_CHANNEL: "频道页面", YT_OTHER_PAGE: "YouTube 其他页面", NO_LINK_OTHER: "直接访问或来源未知", NO_LINK_EMBEDDED: "外部嵌入", ADVERTISING: "广告", END_SCREEN: "片尾画面", ANNOTATION: "注释", HASHTAGS: "话题页面", SOUND_PAGE: "Shorts 音频页面", VIDEO_REMIXES: "Remix 视频", LIVE_REDIRECT: "直播跳转", PRODUCT_PAGE: "商品页面", PROMOTED: "YouTube 推广", WATCH_WITH: "一起观看",
  MOBILE: "手机", DESKTOP: "电脑", TABLET: "平板", TV: "电视", GAME_CONSOLE: "游戏机", UNKNOWN_PLATFORM: "未知设备", SUBSCRIBED: "已订阅观众", UNSUBSCRIBED: "未订阅观众",
};
export function breakdownLabel(key: string, kind: string): string {
  if (kind === "country" && /^[A-Z]{2}$/.test(key)) {
    try { return new Intl.DisplayNames(["zh-CN"], { type: "region" }).of(key) ?? key; } catch { return key; }
  }
  return (kind === "contentType" ? contentLabels[key] : sourceLabels[key]) ?? key;
}
