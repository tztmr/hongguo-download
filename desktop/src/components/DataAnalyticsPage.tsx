import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { analyticsCommands, type AnalyticsCommands, type AnalyticsReport, type ChannelAnalyticsSnapshot } from "../youtube/analyticsCommands";
import { YouTubeVideoLink } from "../youtube/YouTubeVideoLink";
import type { YouTubeModel } from "../youtube/types";
import { AnalyticsTrend } from "../youtube/AnalyticsTrend";
import { AnalyticsDetails } from "../youtube/AnalyticsDetails";
import { integer as formatInteger, duration as formatDuration, hours, percent, comparison, netSubscribers } from "../youtube/analyticsFormatting";

type RangePreset = 7 | 28 | 90 | "custom";

type DataAnalyticsPageProps = {
  youtube: YouTubeModel;
  hidden?: boolean;
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

function errorMessage(reason: unknown): string {
  if (reason && typeof reason === "object" && "message" in reason && typeof reason.message === "string") return reason.message;
  return typeof reason === "string" ? reason : "读取 YouTube 统计失败，请稍后重试";
}

export const DataAnalyticsPage = memo(function DataAnalyticsPage(props: DataAnalyticsPageProps) {
  return <ChannelAnalyticsPage key={props.youtube.activeChannelId ?? "disconnected"} {...props} />;
}, (a, b) => a.hidden === b.hidden && a.commands === b.commands
  && a.youtube.activeChannelId === b.youtube.activeChannelId && a.youtube.channels === b.youtube.channels
  && a.youtube.busy === b.youtube.busy && a.youtube.authorize === b.youtube.authorize);

function ChannelAnalyticsPage({ youtube, commands = analyticsCommands, hidden = false }: DataAnalyticsPageProps) {
  const channelId = youtube.activeChannelId;
  const channel = youtube.channels.find((item) => item.channelId === channelId);
  const today = useMemo(() => pacificDate(), []);
  const [preset, setPreset] = useState<RangePreset>(28);
  const [startDate, setStartDate] = useState(() => moveDate(today, -27));
  const [endDate, setEndDate] = useState(today);
  const [draftStart, setDraftStart] = useState(startDate);
  const [draftEnd, setDraftEnd] = useState(endDate);
  const [authorizing, setAuthorizing] = useState(false);
  const [selectedVideo, setSelectedVideo] = useState<{ id: string; title: string }>();
  const pageRef = useRef<HTMLElement>(null);
  const [errorCodes, setErrorCodes] = useState<string[]>([]);
  const inFlight = useRef(false);
  const [snapshot, setSnapshot] = useState<ChannelAnalyticsSnapshot | null>(null);
  const [reportResult, setReport] = useState<{ scope: string; data: AnalyticsReport } | null>(null);
  const reportScope = `${channelId}/${selectedVideo?.id ?? ""}/${startDate}/${endDate}`;
  const report = reportResult?.scope === reportScope ? reportResult.data : null;
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState("");
  const requestId = useRef(0);

  const load = useCallback(async (showLoading = true) => {
    if (!showLoading && inFlight.current) return;
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
    inFlight.current = true;
    setErrorCodes([]);
    const failures: string[] = [];
    const codes: string[] = [];
    const failed = (reason: unknown) => {
      if (currentRequest !== requestId.current) return;
      failures.push(errorMessage(reason));
      if (reason && typeof reason === "object" && "code" in reason) codes.push(String(reason.code));
      setError(failures.join("；"));
      setErrorCodes([...codes]);
    };
    await Promise.allSettled([
      commands.snapshot(channelId).then(value => {
        if (currentRequest === requestId.current) setSnapshot(value);
      }).catch(failed),
      (selectedVideo ? commands.report(channelId, startDate, endDate, selectedVideo.id) : commands.report(channelId, startDate, endDate)).then(value => {
        if (currentRequest === requestId.current) setReport({ scope: reportScope, data: value });
      }).catch(failed),
    ]);
    if (currentRequest !== requestId.current) return;
    inFlight.current = false;
    setLoading(false);
    setRefreshing(false);
  }, [channelId, commands, endDate, startDate, selectedVideo?.id, reportScope]);

  useEffect(() => {
    setReport(null);
    void load();
    return () => { requestId.current += 1; inFlight.current = false; };
  }, [load]);

  useEffect(() => {
    if (!channelId || hidden) return undefined;
    const timer = window.setInterval(() => { if (!inFlight.current) void load(false); }, 5 * 60 * 1000);
    return () => window.clearInterval(timer);
  }, [channelId, hidden, load]);

  function selectPreset(next: RangePreset) {
    setPreset(next);
    if (next !== "custom") {
      setStartDate(moveDate(endDate, -(next - 1)));
    }
  }

  async function reauthorize() {
    if (authorizing || youtube.busy) return;
    setAuthorizing(true);
    setError("");
    try {
      await youtube.authorize();
      void load();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally { setAuthorizing(false); }
  }

  const reportNote = report?.returnedEndDate
    ? `本次已返回区间：${readableDate(report.startDate)} 至 ${readableDate(report.returnedEndDate)}。${report.returnedEndDate < endDate ? "最近日期可能仍在处理；未返回日期不参与合计与环比。" : "日期按美国太平洋时间计算。"}`
    : "YouTube Analytics 数据按美国太平洋时间归日，最近日期可能有延迟。";
  const customDays = (Date.parse(draftEnd) - Date.parse(draftStart)) / 86400000 + 1;
  const validCustomRange = !!draftStart && !!draftEnd && customDays > 0 && customDays <= 367 && draftEnd <= today;
  const authRequired = errorCodes.includes("YOUTUBE_ANALYTICS_AUTH_REQUIRED") || error.includes("需要统计权限") || error.includes("重新授权");

  if (!channelId) {
    return <main hidden={hidden} className="platform-videos-page analytics-page">
      <header className="platform-videos-header"><h1>数据分析</h1><p>YouTube · 趋势、互动、涨粉与观众来源</p></header>
      <section className="analytics-empty"><h2>请先在设置中授权 YouTube 频道</h2><p>授权后可以读取频道累计观看量和已处理的历史统计。</p></section>
    </main>;
  }

  return <main ref={pageRef} hidden={hidden} className="platform-videos-page analytics-page">
    <header className="platform-videos-header analytics-header">
      <div><h1>数据分析</h1><p>{channel?.title || "YouTube 频道"} · 统计日期按美国太平洋时间计算</p></div>
      <div className="analytics-header-actions">
        <YouTubeVideoLink url={`https://studio.youtube.com/channel/${encodeURIComponent(channelId)}/analytics`} label="打开 YouTube Studio 实时分析" />
        <button type="button" className="secondary-button" disabled={loading || refreshing} onClick={() => void load(false)}>{refreshing ? "刷新中…" : "刷新数据"}</button>
      </div>
    </header>
    {authorizing && <p role="status">请在浏览器完成统计权限授权，应用正在等待返回。</p>}
    {error ? <div className="analytics-alert" role="alert"><span>{error}</span>{authRequired ? <button type="button" className="secondary-button compact" disabled={authorizing || youtube.busy} onClick={() => void reauthorize()}>{authorizing ? "等待浏览器授权…" : "重新授权统计权限"}</button> : null}{errorCodes.includes("YOUTUBE_ANALYTICS_API_NOT_ENABLED") && <YouTubeVideoLink url="https://console.cloud.google.com/apis/library/youtubeanalytics.googleapis.com" label="前往启用 Analytics API" />}</div> : null}
    {selectedVideo && <div className="analytics-video-scope"><div><strong>单视频分析：{selectedVideo.title}</strong><small>{selectedVideo.id} · 订阅增减仅统计该视频观看页</small></div><button type="button" className="secondary-button" onClick={() => { setSelectedVideo(undefined); if (pageRef.current) pageRef.current.scrollTop = 0; }}>返回频道总览</button></div>}
    <section className="analytics-live-card">
      <div><span className="analytics-eyebrow">频道累计观看次数</span><strong>{snapshot ? formatInteger(snapshot.viewCount) : loading ? "读取中…" : "—"}</strong><p>{snapshot ? `最近刷新：${new Date(snapshot.fetchedAt).toLocaleString("zh-CN")}` : "可手动刷新；实时细分请查看 YouTube Studio"}</p></div>
      <div><span className="analytics-eyebrow">频道订阅人数（约）</span><strong>{snapshot?.hiddenSubscriberCount ? "已隐藏" : formatInteger(snapshot?.subscriberCount)}</strong><p>公开计数经过取整，不随日期或视频筛选变化</p></div>
      <div><span className="analytics-eyebrow">频道公开视频数</span><strong>{formatInteger(snapshot?.videoCount)}</strong><p>累计指标包含视频与 Shorts</p></div>
    </section>
    <section className="analytics-panel">
      <div className="analytics-panel-heading"><div><h2>{selectedVideo ? "单视频表现" : "频道表现"}</h2><p>{reportNote}</p></div><div className="analytics-range-controls" role="group" aria-label="统计日期范围">
        {[7, 28, 90].map((days) => <button key={days} type="button" className={preset === days ? "active" : ""} onClick={() => selectPreset(days as RangePreset)}>{days} 天</button>)}
        <button type="button" className={preset === "custom" ? "active" : ""} onClick={() => { setDraftStart(startDate); setDraftEnd(endDate); setPreset("custom"); }}>自定义</button>
      </div></div>
      {preset === "custom" ? <div className="analytics-custom-range"><label>开始日期<input type="date" value={draftStart} max={draftEnd} onChange={(event) => setDraftStart(event.target.value)} /></label><span>至</span><label>结束日期<input type="date" value={draftEnd} min={draftStart} max={today} onChange={(event) => setDraftEnd(event.target.value)} /></label><button type="button" className="primary-button compact" disabled={!validCustomRange} onClick={() => { if (draftStart === startDate && draftEnd === endDate) void load(false); else { setStartDate(draftStart); setEndDate(draftEnd); } }}>查询</button></div> : null}
      {preset === "custom" && customDays > 367 && <p className="error-copy" role="alert">单次最多查询 367 天，请缩短日期范围。</p>}
      {report ? <>
        {report.warnings.map((warning, index) => <p key={`${warning.code}-${index}`} role="status" className="analytics-footnote">上一周期数据未完整读取：{warning.message}。当前区间仍可查看。</p>)}
        {report.comparison && <p className="analytics-footnote">环比区间：{readableDate(report.comparison.startDate)} 至 {readableDate(report.comparison.endDate)}；与本次已返回区间天数相同。无数据或上期为 0 时不计算增长百分比。</p>}
        <div className="analytics-metrics analytics-main-metrics">
          <div><span>观看次数</span><strong>{formatInteger(report.views)}</strong><small>{comparison(report.views, report.comparison?.views)}</small></div>
          <div><span>有效观看</span><strong>{formatInteger(report.engagedViews)}</strong><small>{comparison(report.engagedViews, report.comparison?.engagedViews)}</small></div>
          <div><span>观看时长</span><strong>{hours(report.estimatedMinutesWatched)}</strong><small>{comparison(report.estimatedMinutesWatched, report.comparison?.estimatedMinutesWatched)}</small></div>
          <div><span>平均观看时长</span><strong>{formatDuration(report.averageViewDuration)}</strong><small>{comparison(report.averageViewDuration, report.comparison?.averageViewDuration)}</small></div>
          <div><span>平均观看比例</span><strong>{percent(report.averageViewPercentage)}</strong><small>平均播放比例，非完播率</small></div>
          <div><span>净增订阅</span><strong>{formatInteger(netSubscribers(report))}</strong><small>{comparison(netSubscribers(report), report.comparison ? netSubscribers(report.comparison) : null, true)}</small></div>
        </div>
        <div className="analytics-engagement" aria-label="互动与订阅明细">{[["点赞", report.likes], ["评论", report.comments], ["分享", report.shares], ["新增订阅", report.subscribersGained], ["流失订阅", report.subscribersLost]].map(([label, value]) => <div key={String(label)}><span>{label}</span><strong>{formatInteger(value)}</strong></div>)}</div>
        <p className="analytics-footnote">有效观看与播放次数的计数口径不同，尤其适用于对照 Shorts 表现；缺失数据以“—”显示。均值采用 YouTube 返回值，不对每日均值直接取平均。</p>
        {report.rows.length && report.returnedEndDate ? <>
          <AnalyticsTrend rows={report.rows} startDate={report.startDate} endDate={report.returnedEndDate} />
          <details className="analytics-daily"><summary>逐日明细 · 已返回 {report.rows.length} 天</summary><div className="analytics-table-wrap"><table className="analytics-table"><thead><tr><th>日期</th><th>观看次数</th><th>观看时长</th><th>平均观看时长</th><th>平均观看比例</th><th>点赞</th><th>评论</th><th>分享</th><th>新增订阅</th><th>流失订阅</th></tr></thead><tbody>{[...report.rows].reverse().map((row) => <tr key={row.date}><td>{readableDate(row.date)}</td><td>{formatInteger(row.views)}</td><td>{hours(row.estimatedMinutesWatched)}</td><td>{formatDuration(row.averageViewDuration)}</td><td>{percent(row.averageViewPercentage)}</td><td>{formatInteger(row.likes)}</td><td>{formatInteger(row.comments)}</td><td>{formatInteger(row.shares)}</td><td>{formatInteger(row.subscribersGained)}</td><td>{formatInteger(row.subscribersLost)}</td></tr>)}</tbody></table></div></details>
        </> : <div className="analytics-no-data">所选日期暂无已处理的 Analytics 数据。</div>}
      </> : loading ? <div className="analytics-no-data">正在读取 YouTube 统计…</div> : <div className="analytics-no-data">暂无统计数据。</div>}

    </section>
    {report?.returnedEndDate && <AnalyticsDetails key={selectedVideo?.id ?? "channel"} channelId={channelId} videoId={selectedVideo?.id} startDate={report.startDate} endDate={report.returnedEndDate} revision={report.fetchedAt} commands={commands} onSelectVideo={(video) => { setSelectedVideo(video); if (pageRef.current) pageRef.current.scrollTop = 0; }} />}
  </main>;
}
