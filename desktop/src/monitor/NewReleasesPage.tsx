import { useEffect, useState } from "react";
import { Cover } from "../components/Cover";
import { VideoOrientationBadge } from "../components/VideoOrientationBadge";
import type { NewReleaseType } from "../types";
import type { MonitorSort, NewReleaseMonitor } from "./useNewReleaseMonitor";
import "./monitor.css";
import { MonitorSettingsDialog } from "./MonitorSettingsDialog";

const filters: Array<{ id: NewReleaseType; label: string }> = [
  { id: "playlet", label: "真人剧" },
  { id: "comic_series_rank", label: "漫剧" },
  { id: "ai_playlet", label: "AI剧" },
];

const typeLabels: Record<NewReleaseType, string> = {
  playlet: "真人剧", comic_series_rank: "漫剧", ai_playlet: "AI剧",
};

function count(value: number | undefined) {
  if (value === undefined || !Number.isFinite(value) || value < 0) return "—";
  if (value >= 100_000_000) return `${Number((value / 100_000_000).toFixed(1))}亿`;
  if (value >= 10_000) return `${Number((value / 10_000).toFixed(1))}万`;
  return String(value);
}

function onlineTime(value: number | undefined, feedDate: string) {
  if (value === undefined || !Number.isFinite(value) || value <= 0) return "上线时间未知";
  const date = new Date(value * 1000);
  if (Number.isNaN(date.valueOf())) return "上线时间未知";
  const day = new Intl.DateTimeFormat("en-CA", { timeZone: "Asia/Shanghai", year: "numeric", month: "2-digit", day: "2-digit" }).format(date);
  const time = new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
  return `上线 ${day === feedDate ? time : `${day} ${time}`}`;
}

function checkedTime(value: string) {
  const date = new Date(value);
  if (!value || Number.isNaN(date.valueOf())) return "尚未完成检查";
  return new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
}

export function NewReleasesPage({ model, onSelect, detectOrientation = true }: { model: NewReleaseMonitor; onSelect: (item: NewReleaseMonitor["items"][number]) => void; detectOrientation?: boolean }) {
  const [expandedCategories, setExpandedCategories] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const rangeText = model.days != null ? `仅显示最近 ${model.days} 天上线的剧目（含今天，北京时间）` : null;
  useEffect(() => setExpandedCategories(false), [model.type]);
  const emptyForFilter = model.items.length > 0 && model.filteredItems.length === 0;
  const isDailyFeed = model.dateScope === "today";
  const status = model.loading ? "正在检查" : model.error ? "检查未完成" : model.scanComplete ? "检查完成" : "等待检查";
  const visibleCategories = expandedCategories ? model.categories : model.categories.filter((category, index) => index < 12 || category === model.selectedCategory);
  return (
    <main className="monitor-page" data-testid="monitor-scroll">
      <header className="monitor-header">
        <div>
          <div className="monitor-eyebrow"><span>NEW RELEASE MONITOR</span><time dateTime={model.date}>{model.date} · 北京时间</time></div>
          <h1>新剧监听</h1>
          <p>{rangeText || (isDailyFeed ? "仅显示北京时间今天上线的剧目" : "按新剧榜最新收录展示，上线时间可能早于今天")} · 已收录 {model.items.length} 部</p>
        </div>
        <div className="monitor-header-actions"><button type="button" className="secondary-button" aria-label="监听设置" onClick={() => setSettingsOpen(true)}>设置</button><button type="button" className="secondary-button" onClick={() => void model.refresh()} disabled={model.loading}>{model.loading ? "检查中…" : "立即刷新"}</button></div>
      </header>
      <div className="monitor-status-strip" role="status" aria-live="polite">
        <span className={`monitor-status-dot ${model.loading ? "scanning" : model.error ? "incomplete" : model.scanComplete ? "complete" : ""}`} aria-hidden="true" />
        <strong>{status}</strong>
        <span>已检查 {model.scanPages} 页</span>
        <span>{model.loading ? `已发现 ${model.items.length} 部，结果持续更新` : model.error ? "已获取的剧目已保留，可重新刷新" : "应用打开期间每 5 分钟检查"}</span>
      </div>
      <section className="monitor-controls" aria-label="监听筛选">
        <div className="monitor-filters monitor-type-filters" aria-label="监听类型">
          {filters.map((filter) => <button type="button" key={filter.id} aria-pressed={model.type === filter.id} className={model.type === filter.id ? "active" : ""} onClick={() => model.setType(filter.id)}>{filter.label}</button>)}
        </div>
        {model.categories.length ? (
          <div className="monitor-category-row">
            <span className="monitor-filter-label">题材</span>
            <div className={`monitor-filters monitor-category-filters${expandedCategories ? " expanded" : ""}`} aria-label="详细分类">
              <button type="button" aria-pressed={model.selectedCategory === ""} className={model.selectedCategory === "" ? "active" : ""} onClick={() => model.setCategory("")}>全部</button>
              {visibleCategories.map((category) => <button type="button" key={category} aria-pressed={model.selectedCategory === category} className={model.selectedCategory === category ? "active" : ""} onClick={() => model.setCategory(category)}>{category}</button>)}
              {model.categories.length > 12 ? <button type="button" className="monitor-category-toggle" aria-expanded={expandedCategories} onClick={() => setExpandedCategories((value) => !value)}>{expandedCategories ? "收起分类" : `更多分类（${model.categories.length}）`}</button> : null}
            </div>
          </div>
        ) : null}
        <div className="monitor-toolbar">
          <input type="search" aria-label="搜索已收录剧名" placeholder="搜索已收录剧名" value={model.query} onChange={(event) => model.setQuery(event.target.value)} />
          <label>排序<select aria-label="新剧排序" value={model.sort} onChange={(event) => model.setSort(event.target.value as MonitorSort)}><option value="latest">最新上线</option><option value="hot">热度优先</option><option value="collected">收藏优先</option></select></label>
          <span className="monitor-result-count">显示 {model.filteredItems.length} / {model.items.length} 部</span>
        </div>
      </section>
      {model.error ? <div className="inline-error" role="alert">{model.error}</div> : null}
      {!model.loading && !model.error && model.scanComplete && !model.items.length ? <div className="empty-monitor"><h2>{rangeText ? `最近 ${model.days} 天暂无符合条件的新剧` : isDailyFeed ? "今天还没有新上线剧目" : "当前暂无可用新剧"}</h2><p>已完整检查当前类型的上新列表，下次检查会自动更新</p></div> : null}
      {!model.loading && !model.error && !model.scanComplete && !model.items.length ? <div className="empty-monitor"><h2>等待检查新剧</h2><p>点击“立即刷新”获取当前类型的上新列表</p></div> : null}
      {emptyForFilter ? <div className="empty-monitor"><h2>没有符合筛选条件的剧目</h2><p>试试其他剧名或题材</p><button type="button" className="secondary-button" onClick={() => { model.setQuery(""); model.setCategory(""); }}>清除筛选</button></div> : null}
      <section className="monitor-grid" aria-label="已收录新剧" aria-busy={model.loading}>
        {model.filteredItems.map((item) => (
          <button type="button" className="monitor-card" key={`${item.contentTypeCode}-${item.seriesId}`} onClick={() => onSelect(item)}>
            <div className="monitor-cover"><Cover src={item.cover} title={item.title} /><span className="monitor-online-time">{onlineTime(item.onlineTime, model.date)}</span><VideoOrientationBadge seriesId={item.seriesId} firstVid={item.firstVid} enabled={detectOrientation} /></div>
            <h2 title={item.title}>{item.title}</h2>
            <p title={item.category}>{typeLabels[model.type]} · {item.episodeCount || "--"} 集 · {item.category || "分类未知"}</p>
            <div className="monitor-metrics"><span>播放 {count(item.playCount)}</span><span>🔥 热度 {count(item.hotCount)}</span><span>收藏 {count(item.collectCount)}</span><span>讨论 {count(item.commentCount)}</span></div>
          </button>
        ))}
      </section>
      {model.loading ? <div className="load-more-row">{rangeText ? `正在扫描最近 ${model.days} 天的新剧` : isDailyFeed ? "正在扫描今日上新列表" : "正在扫描新剧榜最新收录"} · 已检查 {model.scanPages} 页…</div> : null}
      {model.refreshedAt ? <p className="monitor-refreshed">上次完整检查：{checkedTime(model.refreshedAt)}</p> : null}
      {settingsOpen && <MonitorSettingsDialog days={model.days} onSave={model.setDays} onClose={() => setSettingsOpen(false)} />}
    </main>
  );
}
