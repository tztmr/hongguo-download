import { heatLabel } from "../seriesPresentation";

export function HeatMetric({ value, detail = false }: { value?: number; detail?: boolean }) {
  const available = value !== undefined && Number.isFinite(value) && value >= 0;
  const title = available
    ? `热度 ${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 }).format(value)} · 上游热度指标`
    : "上游暂未提供热度数据";
  return <span className={`heat-metric ${available ? "available" : "unavailable"}`} title={title} data-testid={detail ? "metric-hot" : undefined}>
    {available ? `🔥 ${detail ? "热度量" : "热度"} ` : "热度 "}<strong>{available ? heatLabel(value) : "暂缺"}</strong>
  </span>;
}
