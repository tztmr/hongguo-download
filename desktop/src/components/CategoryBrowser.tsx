import { useEffect, useRef, useState } from "react";
import { fetchWebCategory, fetchWebCategoryGroups } from "../api";
import type { CategoryFilters, CategoryGroup, SeriesItem, WebCategoryPage } from "../types";
import { CategoryFilter } from "./CategoryFilter";
import { Cover } from "./Cover";
import { VideoOrientationBadge } from "./VideoOrientationBadge";

export const DEFAULT_CATEGORY_FILTERS: CategoryFilters = { background: "", topic: "", setting: "", gender: "2", time: "0", sort_type: "0" };
const liveApi = { fetchWebCategory, fetchWebCategoryGroups };
type Props = { onSelect(item: SeriesItem): void; onResetSelection?(): void; selectedId?: string; api?: typeof liveApi; detectOrientation?: boolean };

export function CategoryBrowser({ onSelect, onResetSelection, selectedId, api = liveApi, detectOrientation = true }: Props) {
  const [groups, setGroups] = useState<CategoryGroup[]>([]);
  const [filters, setFilters] = useState<CategoryFilters>(DEFAULT_CATEGORY_FILTERS);
  const [page, setPage] = useState<WebCategoryPage | null>(null);
  const [items, setItems] = useState<SeriesItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [groupError, setGroupError] = useState("");
  const [revision, setRevision] = useState(0);
  const [groupRevision, setGroupRevision] = useState(0);
  const request = useRef(0);
  const morePending = useRef(false);
  const selectRef = useRef(onSelect);
  selectRef.current = onSelect;
  const resetRef = useRef(onResetSelection);
  resetRef.current = onResetSelection;

  useEffect(() => {
    let cancelled = false;
    setGroupError("");
    void api.fetchWebCategoryGroups("drama").then((values) => {
      if (!cancelled) setGroups(values);
    }).catch(() => { if (!cancelled) setGroupError("分类选项加载失败"); });
    return () => { cancelled = true; };
  }, [api, groupRevision]);

  useEffect(() => {
    const id = ++request.current;
    morePending.current = false;
    setItems([]);
    setPage(null);
    setLoading(true);
    setError("");
    resetRef.current?.();
    void api.fetchWebCategory("drama", filters, 1).then((result) => {
      if (request.current !== id) return;
      setPage(result);
      const unique = [...new Map(result.items.map((item) => [item.bookId, item])).values()];
      setItems(unique);
      if (unique[0]) selectRef.current(unique[0]);
    }).catch((reason) => {
      if (request.current === id) setError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => { if (request.current === id) setLoading(false); });
    return () => { request.current += 1; };
  }, [api, filters, revision]);

  async function loadMore() {
    if (!page?.hasMore || loading || morePending.current) return;
    morePending.current = true;
    const id = request.current;
    setLoading(true);
    setError("");
    try {
      const result = await api.fetchWebCategory("drama", filters, page.nextPage);
      if (request.current !== id) return;
      if (result.hasMore && result.nextPage <= page.nextPage) throw new Error("分类分页未前进，请重试");
      setPage(result);
      setItems((previous) => [...new Map([...previous, ...result.items].map((item) => [item.bookId, item])).values()]);
    } catch (reason) {
      if (request.current === id) setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (request.current === id) { setLoading(false); morePending.current = false; }
    }
  }

  const chosen = groups.flatMap((group) => {
    const key = group.id as keyof CategoryFilters;
    if (filters[key] === DEFAULT_CATEGORY_FILTERS[key]) return [];
    const selected = group.items.find((item) => item.id === filters[key]);
    return selected ? [selected.name] : [];
  });
  return (
    <div className="category-browser">
      {groupError ? <div className="inline-error" role="alert">{groupError}<button type="button" className="text-action" onClick={() => setGroupRevision((value) => value + 1)}>重试分类选项</button></div> : null}
      <CategoryFilter groups={groups} values={filters} onChange={(group, id) => setFilters((state) => ({ ...state, [group]: id }))} />
      <div className="category-summary">
        <strong>{chosen.length ? chosen.join(" · ") : "全部真人剧"}</strong>
        <span>已显示 {items.length} 部{page?.total !== undefined ? ` / 共 ${page.total} 部` : ""}</span>
        {chosen.length ? <button type="button" className="text-action" onClick={() => setFilters(DEFAULT_CATEGORY_FILTERS)}>清空筛选</button> : null}
      </div>
      {error ? <div className="inline-error" role="alert">{error}<button type="button" className="text-action" disabled={loading} onClick={() => page ? void loadMore() : setRevision((value) => value + 1)}>重试</button></div> : null}
      {!loading && !error && !items.length ? <div className="empty-library"><h2>没有符合条件的真人剧</h2><p>试试减少筛选条件，或清空筛选</p></div> : null}
      <div className="poster-grid category-result-grid">
        {items.map((item) => <button type="button" key={item.bookId} className={`poster-card ${selectedId === item.bookId ? "selected" : ""}`} onClick={() => onSelect(item)}>
          <div className="poster-image"><Cover src={item.cover} title={item.title} /><div className="poster-badges"><VideoOrientationBadge seriesId={item.seriesId} firstVid={item.firstVid} enabled={detectOrientation} /></div></div>
          <div className="poster-copy"><h2>{item.title}</h2><p>{item.episodeCount || "--"} 集 · {item.category || "真人剧"}{item.score ? ` · ${item.score}分` : ""}</p></div>
        </button>)}
      </div>
      {loading ? <div className="category-loading" role="status"><span className="loading-spinner" />正在加载分类剧目…</div> : page?.hasMore ? <div className="load-more-row"><button type="button" className="secondary-button" onClick={() => void loadMore()}>加载更多</button></div> : items.length ? <p className="monitor-refreshed">当前条件的剧目已显示完毕</p> : null}
    </div>
  );
}
