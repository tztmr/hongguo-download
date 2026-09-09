import { useEffect, useRef, useState } from "react";
import { fetchRank } from "../api";
import type { RankPage, SeriesItem } from "../types";
import { releaseCategoryNames, releaseCategoryOptions } from "../monitor/categories";
import { metricLabel } from "../seriesPresentation";
import { Cover } from "./Cover";
import { VideoOrientationBadge } from "./VideoOrientationBadge";

export function AIRecommendations({ categoryMode, onSelect, selectedId, detectOrientation = true }: {
  categoryMode: boolean; onSelect(item: SeriesItem): void; selectedId?: string; detectOrientation?: boolean;
}) {
  const [items, setItems] = useState<SeriesItem[]>([]);
  const [page, setPage] = useState<RankPage | null>(null);
  const [category, setCategory] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  const pending = useRef(false);
  const request = useRef(0);
  const selectRef = useRef(onSelect);
  selectRef.current = onSelect;
  useEffect(() => {
    const id = ++request.current;
    setLoading(true); setError(""); pending.current = true;
    void fetchRank({ type: "ai_playlet", board: "ranklist_hot_sc", limit: 20 }).then((result) => {
      if (id !== request.current) return;
      setItems(result.items); setPage(result);
      if (result.items[0]) selectRef.current(result.items[0]);
    }).catch((reason) => { if (id === request.current) setError(String(reason)); })
      .finally(() => { if (id === request.current) { setLoading(false); pending.current = false; } });
    return () => { request.current += 1; };
  }, [revision]);
  async function more() {
    if (!page?.hasMore || pending.current) return;
    const id = request.current;
    pending.current = true; setLoading(true); setError("");
    try {
      const result = await fetchRank({ type: "ai_playlet", board: "ranklist_hot_sc", cursor: page.nextCursor, limit: 20 });
      if (id !== request.current) return;
      setItems((current) => [...new Map([...current, ...result.items].map((item) => [item.seriesId, item])).values()]);
      setPage(result);
    } catch (reason) { if (id === request.current) setError(String(reason)); }
    finally { if (id === request.current) { setLoading(false); pending.current = false; } }
  }
  const visible = categoryMode && category ? items.filter((item) => releaseCategoryNames(item).includes(category)) : items;
  return <div className="category-browser">
    {categoryMode ? <>
      <div className="filter-row" aria-label="AI剧题材"><span>题材</span><button className={`filter-chip ${!category ? "active" : ""}`} onClick={() => setCategory("")}>全部</button>{releaseCategoryOptions(items).map((name) => <button key={name} className={`filter-chip ${category === name ? "active" : ""}`} onClick={() => setCategory(name)}>{name}</button>)}</div>
      <p className="monitor-refreshed">按已加载的 AI 推荐剧目筛选题材，加载更多可扩展结果。</p>
    </> : null}
    <p className="monitor-refreshed">AI剧推荐 · 已加载 {items.length} 部{categoryMode ? ` · 符合题材 ${visible.length} 部` : ""}</p>
    {error ? <div className="inline-error" role="alert">{error}<button onClick={() => page ? void more() : setRevision((value) => value + 1)}>重试</button></div> : null}
    <div className="poster-grid">{visible.map((item) => <button type="button" className={`poster-card ${selectedId === item.bookId ? "selected" : ""}`} key={item.bookId} onClick={() => onSelect(item)}>
      <div className="poster-image"><Cover src={item.cover} title={item.title} /><div className="poster-badges"><VideoOrientationBadge seriesId={item.seriesId} firstVid={item.firstVid} enabled={detectOrientation} /></div></div>
      <div className="poster-copy"><h2>{item.title}</h2><p>{item.episodeCount || "--"} 集 · AI剧{item.category ? ` · ${item.category}` : ""}</p><p>🔥 热度 {metricLabel(item.hotCount)}</p></div>
    </button>)}</div>
    {loading ? <div className="category-loading" role="status">正在加载 AI 剧…</div> : page?.hasMore ? <div className="load-more-row"><button className="secondary-button" onClick={() => void more()}>加载更多</button></div> : <p className="monitor-refreshed">当前推荐剧目已显示完毕</p>}
  </div>;
}
