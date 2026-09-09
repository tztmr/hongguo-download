import { apiGet } from "./api";
import type { SeriesItem } from "./types";

export type VideoOrientation = "横屏" | "竖屏" | "方屏" | "未知";

const cache = new Map<string, VideoOrientation>();
const pending = new Map<string, Promise<VideoOrientation>>();
const queue: Array<() => void> = [];
let active = 0;

function drainQueue() {
  while (active < 3 && queue.length) queue.shift()!();
}

async function readOrientation(seriesId: string, firstVid: string): Promise<VideoOrientation> {
  let itemId = firstVid;
  if (!itemId) {
    const catalog = await apiGet<{ items?: Array<{ item_id: string }> }>(
      `/api/duanju/catalog?book_id=${encodeURIComponent(seriesId)}`,
    );
    itemId = catalog.items?.[0]?.item_id || "";
  }
  if (!itemId) return "未知";
  // Read the actual video model, never the promotional cover's dimensions.
  const data = await apiGet<{ sources?: Array<{ width: number | string; height: number | string }> }>(
    `/api/duanju/content?item_id=${encodeURIComponent(itemId)}`,
  );
  for (const source of data.sources || []) {
    const width = Number(source.width);
    const height = Number(source.height);
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) continue;
    return width > height ? "横屏" : width < height ? "竖屏" : "方屏";
  }
  return "未知";
}

export function detectVideoOrientation({ seriesId, firstVid }: Pick<SeriesItem, "seriesId" | "firstVid">): Promise<VideoOrientation> {
  const key = JSON.stringify([seriesId, firstVid]);
  const cached = cache.get(key);
  if (cached) return Promise.resolve(cached);
  const existing = pending.get(key);
  if (existing) return existing;
  const result = new Promise<VideoOrientation>((resolve) => {
    queue.push(() => {
      active++;
      void readOrientation(seriesId, firstVid).catch((): VideoOrientation => "未知").then((orientation) => {
        if (orientation !== "未知") {
          if (cache.size >= 500) cache.delete(cache.keys().next().value!);
          cache.set(key, orientation);
        }
        pending.delete(key);
        active--;
        resolve(orientation);
        drainQueue();
      });
    });
  });
  pending.set(key, result);
  drainQueue();
  return result;
}
