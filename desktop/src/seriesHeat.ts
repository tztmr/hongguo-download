import { fetchSeriesHeatBatch } from "./api";

export const validHeat = (value: number | undefined): value is number =>
  value !== undefined && Number.isFinite(value) && value >= 0;

export type HeatResult = { value?: number; failed?: boolean };

// Only visible cards enter this queue. Share IDs across pages, combine nearby
// requests, and cap concurrent batches so heat never floods the local API.
export function createSeriesHeatLoader(fetchBatch = (ids: string[]) => fetchSeriesHeatBatch(ids)) {
  const cache = new Map<string, { result: HeatResult; expires: number }>();
  const pending = new Map<string, Promise<HeatResult>>();
  const queue = new Map<string, (result: HeatResult) => void>();
  let active = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function schedule() {
    if (timer !== undefined || active >= 2 || !queue.size) return;
    timer = setTimeout(() => { timer = undefined; drain(); }, 30);
  }
  function drain() {
    while (active < 2 && queue.size) {
      const batch = [...queue.entries()].slice(0, 20);
      for (const [id] of batch) queue.delete(id);
      active++;
      void (async () => {
        let values: Record<string, number | undefined> = {};
        let failed = false;
        try { values = await fetchBatch(batch.map(([id]) => id)); }
        catch { failed = true; }
        for (const [id, resolve] of batch) {
          const result = { value: validHeat(values[id]) ? values[id] : undefined, failed };
          cache.set(id, { result, expires: Date.now() + (failed ? 15_000 : result.value === undefined ? 60_000 : 300_000) });
          if (cache.size > 800) cache.delete(cache.keys().next().value!);
          pending.delete(id);
          resolve(result);
        }
        active--;
        schedule();
      })();
    }
  }
  return (id: string): Promise<HeatResult> => {
    // Actual series IDs are numeric. Incomplete cards cannot form a valid query.
    if (!/^\d{1,24}$/.test(id)) return Promise.resolve({});
    const saved = cache.get(id);
    if (saved && saved.expires > Date.now()) return Promise.resolve(saved.result);
    const inFlight = pending.get(id);
    if (inFlight) return inFlight;
    const promise = new Promise<HeatResult>(resolve => queue.set(id, resolve));
    pending.set(id, promise);
    schedule();
    return promise;
  };
}

export const loadSeriesHeat = createSeriesHeatLoader();
