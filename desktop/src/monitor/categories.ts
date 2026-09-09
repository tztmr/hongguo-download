import type { SeriesItem } from "../types";

const SPLITTER = /[·,，、/|]+/;
const CAMPUS_ALIASES = ["校园", "青春"];
const ANCIENT_ALIASES = ["古风", "古装", "古代", "古言", "宫斗", "仙侠", "武侠"];
const TYPE_LABELS = new Set(["短剧", "真人剧", "漫剧", "AI剧", "AI漫剧"]);

export function releaseCategoryNames(item: SeriesItem) {
  const source = (item.categoryTags?.length ? item.categoryTags : [item.category || ""])
    .flatMap((value) => value.split(SPLITTER));
  const result: string[] = [];
  const add = (label: string) => {
    if (label && !result.includes(label)) result.push(label);
  };

  for (const raw of source) {
    const label = raw.trim();
    if (!label || TYPE_LABELS.has(label) || /^(?:(?:全|共|更新至|更新到|已更新|第)\s*)?\d+\s*(?:集|话|季)(?:全)?$/.test(label) || ["完结", "连载中"].includes(label)) continue;
    add(label);
    if (CAMPUS_ALIASES.some((alias) => label.includes(alias))) {
      add("校园");
    }
    if (ANCIENT_ALIASES.some((alias) => label.includes(alias))) {
      add("古风");
    }
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
