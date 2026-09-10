import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { analyticsCommands, type AnalyticsCommands, type AnalyticsReport, type ChannelAnalyticsSnapshot } from "../youtube/analyticsCommands";
import { YouTubeVideoLink } from "../youtube/YouTubeUploadJobs";
import type { YouTubeModel } from "../youtube/types";

type RangePreset = 7 | 28 | 90 | "custom";

type DataAnalyticsPageProps = {
  youtube: YouTubeModel;
  commands?: AnalyticsCommands;
};

function pacificDate(value = new Date()): string {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: "America/Los_Angeles",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(value);
  const get = (type: string) => parts.find((part) => part.type === type)?.value ?? "";
  return `${get("year")}-${get("month")}-${get("day")}`;
}

function moveDate(date: string, days: number): string {
  const [year, month, day] = date.split("-").map(Number);
  const moved = new Date(Date.UTC(year, month - 1, day + days));
  return moved.toISOString().slice(0, 10);
}

function readableDate(date: string): string {
  const [year, month, day] = date.split("-");
  return `${year}/${month}/${day}`;
}

function formatInteger(value: number | string): string {
  try {
    return BigInt(String(value)).toLocaleString("zh-CN");
  } catch {
    return Number(value).toLocaleString("zh-CN");
  }
}

function formatDecimal(value: number, digits = 1): string {
  return Number.isFinite(value) ? value.toLocaleString("zh-CN", { maximumFractionDigits: digits }) : "—";
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  const rounded = Math.round(seconds);
  const minutes = Math.floor(rounded / 60);
  const remainder = rounded % 60;
  return minutes ? `${minutes} 分 ${remainder} 秒` : `${remainder} 秒`;
}

function errorMessage(reason: unknown): string {
  if (reason && typeof reason === "object" && "message" in reason && typeof reason.message === "string") return reason.message;
  return typeof reason === "string" ? reason : "读取 YouTube 统计失败，请稍后重试";
}

export const DataAnalyticsPage = memo(function DataAnalyticsPage({ youtube, commands = analyticsCommands }: DataAnalyticsPageProps) {
  const channelId = youtube.activeChannelId;
  const channel = youtube.channels.find((item) => item.channelId === channelId);
  const today = useMemo(() => pacificDate(), []);
  const [preset, setPreset] = useState<RangePreset>(28);
  const [startDate, setStartDate] = useState(() => moveDate(today, -27));
  const [endDate, setEndDate] = useState(today);
  const [snapshot, setSnapshot] = useState<ChannelAnalyticsSnapshot | null>(null);
  const [report, setReport] = useState<AnalyticsReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState("");
  const requestId = useRef(0);

  const load = useCallback(async (showLoading = true) => {
    if (!channelId) {
      setSnapshot(null);
      setReport(null);
      setError("");
      return;
    }
    const currentRequest = ++requestId.current;
    if (showLoading) setLoading(true);
    else setRefreshing(true);
    setError("");
    setReport(null);
    const [snapshotResult, reportResult] = await Promise.allSettled([
      commands.snapshot(channelId),
      commands.report(channelId, startDate, endDate),
    ]);
    if (currentRequest !== requestId.current) return;
    if (snapshotResult.status === "fulfilled") setSnapshot(snapshotResult.value);
    else setSnapshot(null);
    if (reportResult.status === "fulfilled") setReport(reportResult.value);
    const failures = [snapshotResult, reportResult].filter((result) => result.status === "rejected") as PromiseRejectedResult[];
    setError(failures.length ? failures.map((failure) => errorMessage(failure.reason)).join("；") : "");
    setLoading(false);
    setRefreshing(false);
  }, [channelId, commands, endDate, startDate]);

  useEffect(() => {
    void load();
    return () => { requestId.current += 1; };
  }, [load]);

  useEffect(() => {
    if (!channelId) return undefined;
    const timer = window.setInterval(() => { void load(false); }, 5 * 60 * 1000);
    return () => window.clearInterval(timer);
  }, [channelId, load]);

  function selectPreset(next: RangePreset) {
    setPreset(next);
    if (next !== "custom") {
      setStartDate(moveDate(endDate, -(next - 1)));
    }
  }

  async function reauthorize() {
    setError("");
    try {
      await youtube.authorize();
      void load();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  const reportNote = report?.returnedEndDate && report.returnedEndDate < endDate
    ? `YouTube 当前只返回到 ${readableDate(report.returnedEndDate)}，最近日期可能仍在处理。`
    : "YouTube Analytics 数据按美国太平洋时间归日，最近日期可能有延迟。";
  const authRequired = error.includes("需要统计权限") || error.includes("重新授权");

  if (!channelId) {
    return <main className="platform-videos-page analytics-page">
      <header className="platform-videos-header"><h1>数据分析</h1><p>YouTube · 观看次数、观看时长和平均观看时长</p></header>
      <section className="analytics-empty"><h2>请先在设置中授权 YouTube 频道</h2><p>授权后可以读取频道累计观看量和已处理的历史统计。</p></section>
    </main>;
  }

  return <main className="platform-videos-page analytics-page">
    <header className="platform-videos-header analytics-header">
      <div><h1>数据分析</h1><p>{channel?.title || "YouTube 频道"} · 统计日期按美国太平洋时间计算</p></div>
      <div className="analytics-header-actions">
        <YouTubeVideoLink url={`https://studio.youtube.com/channel/${encodeURIComponent(channelId)}/analytics`} label="打开 YouTube Studio 实时分析" />
        <button type="button" className="secondary-button" disabled={loading || refreshing} onClick={() => void load(false)}>{refreshing ? "刷新中…" : "刷新数据"}</button>
      </div>
    </header>
    {error ? <div className="analytics-alert" role="alert"><span>{error}</span>{authRequired ? <button type="button" className="secondary-button compact" onClick={() => void reauthorize()}>重新授权统计权限</button> : null}</div> : null}
    <section className="analytics-live-card">
      <div><span className="analytics-eyebrow">频道累计观看次数</span><strong>{snapshot ? formatInteger(snapshot.viewCount) : loading ? "读取中…" : "—"}</strong><p>{snapshot ? `最近刷新：${new Date(snapshot.fetchedAt).toLocaleString("zh-CN")}` : "可手动刷新；实时细分请查看 YouTube Studio"}</p></div>
      <div className="analytics-live-status"><span className="analytics-live-dot" />累计数据</div>
    </section>
    <section className="analytics-panel">
      <div className="analytics-panel-heading"><div><h2>历史统计</h2><p>{reportNote}</p></div><div className="analytics-range-controls" role="group" aria-label="统计日期范围">
        {[7, 28, 90].map((days) => <button key={days} type="button" className={preset === days ? "active" : ""} onClick={() => selectPreset(days as RangePreset)}>{days} 天</button>)}
        <button type="button" className={preset === "custom" ? "active" : ""} onClick={() => setPreset("custom")}>自定义</button>
      </div></div>
      {preset === "custom" ? <div className="analytics-custom-range"><label>开始日期<input type="date" value={startDate} max={endDate} onChange={(event) => setStartDate(event.target.value)} /></label><span>至</span><label>结束日期<input type="date" value={endDate} min={startDate} max={today} onChange={(event) => setEndDate(event.target.value)} /></label><button type="button" className="primary-button compact" onClick={() => void load()}>查询</button></div> : null}
      {report ? <>
        <div className="analytics-metrics">
          <div><span>观看次数</span><strong>{formatInteger(report.views)}</strong></div>
          <div><span>观看时长</span><strong>{formatDecimal(report.estimatedMinutesWatched / 60)} 小时</strong></div>
          <div><span>平均观看时长</span><strong>{formatDuration(report.averageViewDuration)}</strong></div>
          <div><span>已返回天数</span><strong>{report.rows.length} 天</strong></div>
        </div>
        {report.rows.length ? <div className="analytics-table-wrap"><table className="analytics-table"><thead><tr><th>日期</th><th>观看次数</th><th>观看时长</th><th>平均观看时长</th></tr></thead><tbody>{[...report.rows].reverse().map((row) => <tr key={row.date}><td>{readableDate(row.date)}</td><td>{formatInteger(row.views)}</td><td>{formatDecimal(row.estimatedMinutesWatched / 60)} 小时</td><td>{formatDuration(row.averageViewDuration)}</td></tr>)}</tbody></table></div> : <div className="analytics-no-data">所选日期暂无已处理的 Analytics 数据。</div>}
      </> : loading ? <div className="analytics-no-data">正在读取 YouTube 统计…</div> : <div className="analytics-no-data">暂无统计数据。</div>}
    </section>
  </main>;
});
