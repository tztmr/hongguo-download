import { heatLabel } from "../seriesPresentation";
import type { Ref } from "react";

export function HeatMetric({ value, detail = false, status, elementRef }: { value?: number; detail?: boolean; status?: "loading" | "error"; elementRef?: Ref<HTMLSpanElement> }) {
  const available = value !== undefined && Number.isFinite(value) && value >= 0;
  const title = available
    ? `热度 ${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 }).format(value)} · 上游热度指标`
    : status === "loading" ? "正在自动查询热度" : status === "error" ? "热度查询暂时失败，稍后重新浏览可重试" : "上游暂未提供热度数据";
  return <span ref={elementRef} className={`heat-metric ${available ? "available" : "unavailable"}`} title={title} data-testid={detail ? "metric-hot" : undefined}>
    {available ? `🔥 ${detail ? "热度量" : "热度"} ` : "热度 "}<strong>{available ? heatLabel(value) : status === "loading" ? "查询中" : status === "error" ? "未获取" : "暂缺"}</strong>
  </span>;
}
