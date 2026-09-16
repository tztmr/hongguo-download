import { useEffect, useRef, useState } from "react";
import { loadSeriesHeat, validHeat, type HeatResult } from "../seriesHeat";
import { HeatMetric } from "./HeatMetric";

export function SeriesHeatMetric({ seriesId, value, enabled = true }: {
  seriesId: string; value?: number; enabled?: boolean;
}) {
  const target = useRef<HTMLSpanElement>(null);
  const [loaded, setLoaded] = useState<{ id: string; result: HeatResult } | null>(null);
  const result = loaded?.id === seriesId ? loaded.result : undefined;
  const provided = validHeat(value);
  useEffect(() => {
    if (!enabled || provided) return;
    let disposed = false, started = false;
    const load = () => {
      if (disposed || started) return;
      started = true;
      void loadSeriesHeat(seriesId).then(result => {
        if (!disposed) setLoaded({ id: seriesId, result });
      });
    };
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) { observer?.disconnect(); load(); }
    });
    if (observer && target.current) observer.observe(target.current);
    else load();
    return () => { disposed = true; observer?.disconnect(); };
  }, [seriesId, enabled, provided]);
  return <HeatMetric elementRef={target} value={provided ? value : result?.value}
    status={!provided && enabled ? result?.failed ? "error" : !result ? "loading" : undefined : undefined} />;
}
