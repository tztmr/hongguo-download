import type { SeriesItem } from "../types";

const SPLITTER = /[·,，、/|]+/;
const CAMPUS_ALIASES = ["校园", "青春"];
const ANCIENT_ALIASES = ["古风", "古装", "古代", "古言", "宫斗", "仙侠", "武侠"];
const TYPE_LABELS = new Set(["短剧", "真人剧", "漫剧", "AI剧", "AI漫剧"]);

export function releaseCategoryNames(item: SeriesItem) {
  const source = item.categoryTags?.length
    ? item.categoryTags
    : item.category.split(SPLITTER);
  const result: string[] = [];
  const add = (label: string) => {
    if (label && !result.includes(label)) result.push(label);
  };

  for (const raw of source) {
    const label = raw.trim();
    if (!label || TYPE_LABELS.has(label)) continue;
    let normalized = false;
    if (CAMPUS_ALIASES.some((alias) => label.includes(alias))) {
      add("校园");
      normalized = true;
    }
    if (ANCIENT_ALIASES.some((alias) => label.includes(alias))) {
      add("古风");
      normalized = true;
    }
    if (!normalized) add(label);
  }
  return result.length ? result : ["其他"];
}

export function releaseCategoryOptions(items: SeriesItem[]) {
  const options: string[] = [];
  for (const item of items) {
    for (const category of releaseCategoryNames(item)) {
      if (category !== "其他" && !options.includes(category)) options.push(category);
    }
  }
  if (items.some((item) => releaseCategoryNames(item).includes("其他"))) options.push("其他");
  return options;
}
