import { describe, expect, it } from "vitest";
import type { SeriesItem } from "../types";
import { releaseCategoryNames, releaseCategoryOptions } from "./categories";

function item(id: string, category: string, categoryTags?: string[]): SeriesItem {
  return {
    bookId: id, seriesId: id, title: id, cover: "", firstVid: "", contentTypeCode: 1,
    episodeCount: 1, abstract: "", score: "", category, categoryTags, author: "", rankTags: [],
  };
}

describe("monitor secondary categories", () => {
  it("keeps original labels alongside campus and ancient aggregations", () => {
    expect(releaseCategoryNames(item("a", "青春校园 · 古装仙侠 · 都市日常")))
      .toEqual(["青春校园", "校园", "古装仙侠", "古风", "都市日常"]);
  });

  it("uses structured tags and maps missing categories to other", () => {
    expect(releaseCategoryNames(item("a", "ignored", ["武侠", "校园", "校园"])))
      .toEqual(["武侠", "古风", "校园"]);
    expect(releaseCategoryNames(item("b", ""))).toEqual(["其他"]);
  });

  it("splits delimited structured tags without collapsing distinct genres", () => {
    expect(releaseCategoryNames(item("a", "", ["仙侠 · 武侠", "短剧", "仙侠"])))
      .toEqual(["仙侠", "古风", "武侠"]);
  });

  it("builds stable unique options with other last", () => {
    expect(releaseCategoryOptions([
      item("a", "校园"), item("b", "都市"), item("c", ""), item("d", "校园"),
    ])).toEqual(["校园", "都市", "其他"]);
  });
});
