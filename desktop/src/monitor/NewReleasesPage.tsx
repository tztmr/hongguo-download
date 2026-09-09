import { Cover } from "../components/Cover";
import { VideoOrientationBadge } from "../components/VideoOrientationBadge";
import type { NewReleaseType } from "../types";
import type { NewReleaseMonitor } from "./useNewReleaseMonitor";

const filters: Array<{ id: NewReleaseType; label: string }> = [
  { id: "playlet", label: "真人剧" },
  { id: "comic_series_rank", label: "漫剧" },
  { id: "ai_playlet", label: "AI剧" },
];

const typeLabels: Record<NewReleaseType, string> = {
  playlet: "真人剧",
  comic_series_rank: "漫剧",
  ai_playlet: "AI剧",
};

function count(value: number | undefined) {
  if (value === undefined) return "—";
  if (value >= 100_000_000) return `${Number((value / 100_000_000).toFixed(1))}亿`;
  if (value >= 10_000) return `${Number((value / 10_000).toFixed(1))}万`;
  return String(value);
}

function onlineTime(value: number | undefined) {
  if (value === undefined) return "—";
  return new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", hour: "2-digit", minute: "2-digit", hour12: false }).format(new Date(value * 1000));
}

export function NewReleasesPage({ model, onSelect, detectOrientation = true }: { model: NewReleaseMonitor; onSelect: (item: NewReleaseMonitor["items"][number]) => void; detectOrientation?: boolean }) {
  const emptyForCategory = model.items.length > 0 && model.filteredItems.length === 0;
  const isDailyFeed = model.type === "playlet";
  return (
    <main className="monitor-page" data-testid="monitor-scroll">
      <header className="monitor-header">
        <div>
          <span>NEW RELEASE MONITOR</span>
          <h1>新剧监听</h1>
          <p>{isDailyFeed ? "仅显示北京时间今天上线的剧目" : "按新剧榜最新收录展示"} · 已收录 {model.items.length} 部</p>
        </div>
        <button type="button" className="secondary-button" onClick={() => void model.refresh()} disabled={model.loading}>{model.loading ? "刷新中…" : "立即刷新"}</button>
      </header>
      <div className="monitor-filters" aria-label="监听类型">
        {filters.map((filter) => <button type="button" key={filter.id} className={model.type === filter.id ? "active" : ""} onClick={() => model.setType(filter.id)}>{filter.label}</button>)}
      </div>
      {model.categories.length ? (
        <div className="monitor-filters monitor-category-filters" aria-label="详细分类">
          <button type="button" className={model.selectedCategory === "" ? "active" : ""} onClick={() => model.setCategory("")}>全部</button>
          {model.categories.map((category) => <button type="button" key={category} className={model.selectedCategory === category ? "active" : ""} onClick={() => model.setCategory(category)}>{category}</button>)}
        </div>
      ) : null}
      {model.error ? <div className="inline-error">{model.error}</div> : null}
      {!model.loading && !model.items.length ? <div className="empty-monitor"><h2>{isDailyFeed ? "今天还没有新上线剧目" : "当前暂无可用新剧"}</h2><p>已完整检查当前类型的上新列表，应用打开期间每 5 分钟继续检查</p></div> : null}
      {!model.loading && emptyForCategory ? <div className="empty-monitor"><h2>当前分类暂无剧目</h2><p>可以切换“全部”或其他详细分类</p></div> : null}
      <section className="monitor-grid">
        {model.filteredItems.map((item) => (
          <button type="button" className="monitor-card" key={`${item.contentTypeCode}-${item.seriesId}`} onClick={() => onSelect(item)}>
            <div className="monitor-cover"><Cover src={item.cover} title={item.title} /><span>上线 {onlineTime(item.onlineTime)}</span><VideoOrientationBadge seriesId={item.seriesId} firstVid={item.firstVid} enabled={detectOrientation} /></div>
            <h2>{item.title}</h2>
            <p>{typeLabels[model.type]} · {item.episodeCount || "--"} 集 · {item.category || "其他"}</p>
            <div className="monitor-metrics"><span>播放 {count(item.playCount)}</span><span>热度 {count(item.hotCount)}</span><span>收藏 {count(item.collectCount)}</span><span>点赞 {count(item.likeCount)}</span></div>
          </button>
        ))}
      </section>
      {model.loading ? <div className="load-more-row">正在扫描全部今日新剧…</div> : null}
      {model.refreshedAt ? <p className="monitor-refreshed">最后检查：{new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", hour: "2-digit", minute: "2-digit", hour12: false }).format(new Date(model.refreshedAt))}</p> : null}
    </main>
  );
}
