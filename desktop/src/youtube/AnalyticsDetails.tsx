import { useEffect, useMemo, useRef, useState } from "react";
import type { AnalyticsBreakdown, AnalyticsCommands, BreakdownKind, BreakdownRow } from "./analyticsCommands";
import { breakdownLabel, contentLabels, decimal, duration, hours, integer, netSubscribers, percent } from "./analyticsFormatting";
import { YouTubeVideoLink } from "./YouTubeVideoLink";

const tabs: [BreakdownKind, string][] = [["videos", "热门视频"], ["contentType", "视频与 Shorts"], ["traffic", "流量来源"], ["country", "国家与地区"], ["device", "观看设备"], ["subscribed", "订阅状态"]];
export function AnalyticsDetails({ channelId, videoId, startDate, endDate, revision, commands, onSelectVideo }: {
  channelId: string; videoId?: string; startDate: string; endDate: string; revision: string; commands: AnalyticsCommands;
  onSelectVideo(video: { id: string; title: string }): void;
}) {
  const [kind, setKind] = useState<BreakdownKind>(videoId ? "traffic" : "videos");
  const [result, setResult] = useState<{ key: string; data: AnalyticsBreakdown }>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [contentType, setContentType] = useState("all");
  const [sort, setSort] = useState("views");
  const generation = useRef(0);
  const inFlight = useRef(false);
  const cache = useRef(new Map<string, AnalyticsBreakdown>());
  const previousScope = useRef("");
  const scope = `${channelId}/${videoId ?? ""}/${startDate}/${endDate}/${revision}`;
  const key = `${scope}/${kind}`;
  async function load(force = false) {
    if (force && inFlight.current) return;
    const request = ++generation.current;
    if (previousScope.current !== scope) { cache.current.clear(); previousScope.current = scope; }
    setError("");
    const cached = force ? undefined : cache.current.get(key);
    if (cached) { setResult({ key, data: cached }); setLoading(false); inFlight.current = false; return; }
    inFlight.current = true; setLoading(true);
    try {
      const data = await commands.breakdown(channelId, startDate, endDate, kind, videoId);
      if (request !== generation.current) return;
      cache.current.set(key, data); setResult({ key, data });
    } catch (reason) {
      if (request === generation.current) setError(reason && typeof reason === "object" && "message" in reason ? String(reason.message) : "该细分报表读取失败，请重试");
    } finally { if (request === generation.current) { inFlight.current = false; setLoading(false); } }
  }
  useEffect(() => { void load(); return () => { generation.current++; inFlight.current = false; }; }, [key, commands]);
  const data = result?.key === key ? result.data : undefined;
  const metric = (row: BreakdownRow) => sort === "watchTime" ? row.estimatedMinutesWatched : sort === "subscribers" ? row.subscribersGained : sort === "retention" ? row.averageViewPercentage : row.views;
  const rows = useMemo(() => (data?.rows ?? []).filter((row) => kind !== "videos" || ((contentType === "all" || (row.contentType || "UNSPECIFIED") === contentType)
    && `${row.title ?? ""} ${row.key}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())))
    .sort((a, b) => (metric(b) ?? -Infinity) - (metric(a) ?? -Infinity)), [data, contentType, query, sort, kind]);
  const total = (data?.rows ?? []).reduce((sum, row) => sum + (row.views ?? 0), 0);
  const hasAverages = kind !== "traffic" && kind !== "device";
  return <section className="analytics-panel analytics-details" aria-label="细分报表">
    <div className="analytics-panel-heading"><div><h2>细分表现</h2><p>{startDate} 至 {endDate} · {videoId ? "当前视频" : "当前频道"}</p></div><button type="button" className="secondary-button" disabled={loading} onClick={() => void load(true)}>刷新此报表</button></div>
    <div className="analytics-detail-tabs" role="group" aria-label="选择细分报表">{tabs.filter(([id]) => id !== "videos" || !videoId).map(([id, label]) => <button type="button" key={id} className={kind === id ? "active" : ""} aria-pressed={kind === id} onClick={() => { setKind(id); setSort("views"); }}>{label}</button>)}</div>
    {kind === "videos" && <><div className="analytics-video-filters">
      <input type="search" aria-label="搜索热门视频" placeholder="搜索标题或视频 ID" value={query} onChange={(event) => setQuery(event.target.value)} />
      <select aria-label="筛选热门视频类型" value={contentType} onChange={(event) => setContentType(event.target.value)}><option value="all">全部类型</option>{Object.entries(contentLabels).map(([id, label]) => <option key={id} value={id}>{label}</option>)}</select>
      <select aria-label="热门视频排序" value={sort} onChange={(event) => setSort(event.target.value)}><option value="views">观看次数</option><option value="watchTime">观看时长</option><option value="subscribers">新增订阅</option><option value="retention">平均观看比例</option></select>
    </div><p className="analytics-footnote">读取本区间观看量前 200 条；筛选和排序只作用于已返回的榜单。订阅数据为视频观看页带来的增减。</p></>}
    {error && <div role="alert" className="analytics-alert">{error}{data ? "；下方保留上次成功读取的数据。" : ""}</div>}
    {data?.warnings.map((warning, index) => <p role="status" className="analytics-footnote" key={`${warning.code}-${index}`}>{warning.message}；已读取的统计仍可查看，缺少标题的视频以 ID 显示。</p>)}
    {loading && <p role="status">正在读取{tabs.find(([id]) => id === kind)?.[1]}…</p>}
    {data && rows.length > 0 && <div className="analytics-table-wrap analytics-detail-scroll"><table className="analytics-table"><thead><tr>
      <th>{kind === "videos" ? "视频" : "分类"}</th><th>观看次数</th><th>有效观看</th><th>观看时长</th>
      {hasAverages && <><th>平均观看时长</th><th>平均观看比例</th></>}
      {kind === "videos" ? <><th>点赞</th><th>评论</th><th>分享</th><th>净增订阅</th><th>操作</th></> : <th>观看占比</th>}
    </tr></thead><tbody>{rows.map((row) => <tr key={`${row.key}/${row.contentType ?? ""}`}>
      <td className={kind === "videos" ? "analytics-video-title" : ""}>{kind === "videos" ? <><strong>{row.title || row.key}</strong><small>{contentLabels[row.contentType ?? ""] ?? row.contentType ?? "类型未知"} · {row.key}</small></> : <><strong>{breakdownLabel(row.key, kind)}</strong>{kind === "country" && <small>{row.key}</small>}</>}</td>
      <td>{integer(row.views)}</td><td>{integer(row.engagedViews)}</td><td>{hours(row.estimatedMinutesWatched)}</td>
      {hasAverages && <><td>{duration(row.averageViewDuration)}</td><td>{percent(row.averageViewPercentage)}</td></>}
      {kind === "videos" ? <><td>{integer(row.likes)}</td><td>{integer(row.comments)}</td><td>{integer(row.shares)}</td><td>{integer(netSubscribers(row))}</td><td><button type="button" className="secondary-button compact" aria-label={`查看 ${row.title || row.key} 的趋势`} onClick={() => onSelectVideo({ id: row.key, title: row.title || row.key })}>查看趋势</button><YouTubeVideoLink url={`https://studio.youtube.com/video/${encodeURIComponent(row.key)}/analytics`} label="Studio ↗" /></td></>
        : <td className="analytics-share">{row.views == null || total <= 0 ? "—" : <><span>{decimal(row.views / total * 100)}%</span><meter min="0" max="100" value={row.views / total * 100} aria-label={`${breakdownLabel(row.key, kind)}观看占比`} /></>}</td>}
    </tr>)}</tbody></table></div>}
    {data && !rows.length && !loading && <p className="analytics-no-data">{data.rows.length ? "没有匹配的热门视频，请调整筛选。" : "此报表暂无已返回数据；数据不足或隐私阈值可能使部分分类不显示。"}</p>}
    {data && <p className="analytics-footnote">已显示 {rows.length} 条{data.truncated ? " · 达到本次查询上限" : ""} · 刷新于 {new Date(data.fetchedAt).toLocaleString("zh-CN")}{kind !== "videos" ? "。占比按此报表已返回的观看量计算，受隐私阈值和统计口径影响，合计可能与频道总览不同。" : ""}</p>}
  </section>;
}
