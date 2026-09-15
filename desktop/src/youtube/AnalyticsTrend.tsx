import { useState } from "react";
import type { AnalyticsRow } from "./analyticsCommands";
import { decimal, integer, netSubscribers } from "./analyticsFormatting";

type TrendMetric = "views" | "watchTime" | "netSubscribers";
const labels: Record<TrendMetric, string> = { views: "观看次数", watchTime: "观看时长（小时）", netSubscribers: "净增订阅" };
export function AnalyticsTrend({ rows, startDate, endDate }: { rows: AnalyticsRow[]; startDate: string; endDate: string }) {
  const [metric, setMetric] = useState<TrendMetric>("views");
  const [activeDate, setActiveDate] = useState<string>();
  const start = Date.parse(`${startDate}T00:00:00Z`), end = Date.parse(`${endDate}T00:00:00Z`);
  const span = Math.max(1, (end - start) / 86400000);
  const value = (row: AnalyticsRow) => metric === "netSubscribers" ? netSubscribers(row) : metric === "watchTime" ? row.estimatedMinutesWatched == null ? null : row.estimatedMinutesWatched / 60 : row.views;
  const points = rows.filter((row) => row.date >= startDate && row.date <= endDate).map((row) => ({ row, n: value(row), day: (Date.parse(`${row.date}T00:00:00Z`) - start) / 86400000 }));
  const values = points.flatMap((point) => point.n == null ? [] : [point.n]);
  const minimum = Math.min(0, ...values), maximum = Math.max(1, ...values), amplitude = maximum - minimum;
  const x = (day: number) => 68 + (day / span) * 816;
  const y = (n: number) => 198 - ((n - minimum) / amplitude) * 162;
  let previousDay: number | undefined;
  const path = points.map((point) => {
    if (point.n == null) { previousDay = undefined; return ""; }
    const continuation = previousDay != null && point.day === previousDay + 1;
    previousDay = point.day;
    return `${continuation ? "L" : "M"} ${x(point.day)} ${y(point.n)}`;
  }).join(" ");
  const active = points.find((point) => point.row.date === activeDate) ?? points[points.length - 1];
  return <section className="analytics-trend" aria-label="每日趋势">
    <div className="analytics-panel-heading"><h3>每日趋势</h3><select aria-label="趋势指标" value={metric} onChange={(event) => setMetric(event.target.value as TrendMetric)}>{Object.entries(labels).map(([id, label]) => <option key={id} value={id}>{label}</option>)}</select></div>
    {values.length ? <>
      <svg className="analytics-trend-chart" viewBox="0 0 920 240" role="img" aria-label={`${labels[metric]}趋势图，缺失日期不补零`}>
        {[0, 0.5, 1].map((fraction) => { const n = minimum + amplitude * fraction; return <g key={fraction}><line x1="68" x2="884" y1={y(n)} y2={y(n)} className="analytics-chart-grid" /><text x="58" y={y(n) + 4} textAnchor="end">{decimal(n)}</text></g>; })}
        <path d={path} fill="none" className="analytics-chart-line" />
        {points.filter((point) => point.n != null).map((point) => <circle key={point.row.date} cx={x(point.day)} cy={y(point.n!)} r={active?.row.date === point.row.date ? 5 : 3} tabIndex={0} aria-label={`${point.row.date}：${labels[metric]} ${decimal(point.n)}`} onFocus={() => setActiveDate(point.row.date)} onMouseEnter={() => setActiveDate(point.row.date)}><title>{point.row.date} · {labels[metric]}：{decimal(point.n)}</title></circle>)}
        <text x="68" y="225">{startDate}</text><text x="884" y="225" textAnchor="end">{endDate}</text>
      </svg>
      <p className="analytics-chart-caption" aria-live="polite">{active ? `${active.row.date} · ${labels[metric]}：${metric === "watchTime" ? decimal(active.n) : integer(active.n)}` : "暂无可展示的数值"}。悬停或用键盘选择数据点查看明细。</p>
    </> : <p className="analytics-no-data">此指标暂无已返回的数据。</p>}
  </section>;
}
