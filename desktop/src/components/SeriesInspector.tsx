import { useEffect, useState } from "react";
import type { DefinitionPreference, EpisodeItem, SeriesItem } from "../types";
import { Cover } from "./Cover";

type SeriesInspectorProps = {
  series: SeriesItem | null;
  definition: DefinitionPreference;
  episodes: EpisodeItem[];
  selectedIds: string[];
  loading: boolean;
  metricsLoading?: boolean;
  metricsError?: string;
  onSelectionChange: (ids: string[]) => void;
  onEnqueue: () => void;
};

export function SeriesInspector({
  series,
  definition,
  episodes,
  selectedIds,
  loading,
  metricsLoading = false,
  metricsError = "",
  onSelectionChange,
  onEnqueue,
}: SeriesInspectorProps) {
  const [rangeStart, setRangeStart] = useState(1);
  const [rangeEnd, setRangeEnd] = useState(1);

  useEffect(() => {
    setRangeStart(episodes[0]?.index || 1);
    setRangeEnd(episodes[episodes.length - 1]?.index || 1);
  }, [episodes]);

  if (!series) {
    return (
      <aside className="series-inspector empty-inspector">
        <div className="empty-symbol" aria-hidden="true">+</div>
        <h2>选择一部剧</h2>
        <p>查看剧集并批量加入下载队列</p>
      </aside>
    );
  }

  const selected = new Set(selectedIds);
  const applyRange = () => {
    const start = Math.min(rangeStart, rangeEnd);
    const end = Math.max(rangeStart, rangeEnd);
    onSelectionChange(episodes.filter((episode) => episode.index >= start && episode.index <= end).map((episode) => episode.itemId));
  };
  const toggle = (itemId: string) => {
    onSelectionChange(selected.has(itemId) ? selectedIds.filter((id) => id !== itemId) : [...selectedIds, itemId]);
  };
  const definitionLabel = definition === "auto" ? "自动最高" : definition;
  const countLabel = (value: number | undefined) => {
    if (value === undefined) return "—";
    if (value >= 100_000_000) return `${Number((value / 100_000_000).toFixed(1))}亿`;
    if (value >= 10_000) return `${Number((value / 10_000).toFixed(1))}万`;
    return String(value);
  };
  const onlineLabel = series.onlineTime === undefined
    ? "—"
    : new Intl.DateTimeFormat("zh-CN", {
        timeZone: "Asia/Shanghai",
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      }).format(new Date(series.onlineTime * 1000));

  return (
    <aside className="series-inspector">
      <header className="inspector-header">
        <Cover src={series.cover} title={series.title} className="inspector-poster" />
        <div className="inspector-copy">
          <h2>{series.title}</h2>
          <p>{series.episodeCount || episodes.length} 集 · {series.category || "真人剧"} · {definitionLabel}</p>
          {series.abstract ? <span>{series.abstract}</span> : null}
        </div>
      </header>

      <section className="series-metrics" aria-label="剧集数据">
        {metricsLoading ? <p className="metrics-state" role="status">正在加载剧集数据…</p> : (
          <>
            {metricsError ? <p className="metrics-state error" role="alert">剧集数据加载失败，请重新点击该剧重试</p> : null}
            <span data-testid="metric-online">上线时间 {onlineLabel}</span>
            <span data-testid="metric-play">播放量 {countLabel(series.playCount)}</span>
            <span data-testid="metric-hot">热度量 {countLabel(series.hotCount)}</span>
            <span data-testid="metric-collect">收藏量 {countLabel(series.collectCount)}</span>
            <span data-testid="metric-like">点赞量 {countLabel(series.likeCount)}</span>
          </>
        )}
      </section>

      <section className="episode-picker" aria-label="剧集选择">
        <div className="section-heading">
          <div>
            <h3>剧集选择</h3>
            <p>已选 {selectedIds.length} 集</p>
          </div>
          <div className="selection-actions">
            <button type="button" className="quiet-button" onClick={() => onSelectionChange(episodes.map((episode) => episode.itemId))}>全选</button>
            <button type="button" className="quiet-button" onClick={() => onSelectionChange(episodes.filter((episode) => !selected.has(episode.itemId)).map((episode) => episode.itemId))}>反选</button>
          </div>
        </div>

        {loading ? <p className="muted-line">正在加载剧集…</p> : null}
        {!loading && !episodes.length ? <p className="muted-line">暂无可下载剧集</p> : null}
        <div className="episode-grid">
          {episodes.map((episode) => (
            <label className={`episode-tile ${selected.has(episode.itemId) ? "selected" : ""}`} key={episode.itemId}>
              <input
                type="checkbox"
                checked={selected.has(episode.itemId)}
                onChange={() => toggle(episode.itemId)}
                aria-label={episode.title}
              />
              <span>{episode.index}</span>
            </label>
          ))}
        </div>
      </section>

      <div className="range-picker">
        <label>起始集<input type="number" min={1} max={episodes.length || 1} value={rangeStart} onChange={(event) => setRangeStart(Number(event.target.value))} /></label>
        <span aria-hidden="true">—</span>
        <label>结束集<input type="number" min={1} max={episodes.length || 1} value={rangeEnd} onChange={(event) => setRangeEnd(Number(event.target.value))} /></label>
        <button type="button" className="secondary-button" onClick={applyRange} disabled={!episodes.length}>选择范围</button>
      </div>

      <button type="button" className="primary-button enqueue-button" onClick={onEnqueue} disabled={!selectedIds.length}>
        加入下载队列（已选 {selectedIds.length} 集）
      </button>
      <p className="inspector-footnote">重复剧集会自动跳过 · 下载记录重启后保留</p>
    </aside>
  );
}
