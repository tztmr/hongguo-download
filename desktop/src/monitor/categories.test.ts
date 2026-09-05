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
  it("normalizes campus and ancient aliases while retaining useful labels", () => {
    expect(releaseCategoryNames(item("a", "青春校园 · 古装仙侠 · 都市日常")))
      .toEqual(["校园", "古风", "都市日常"]);
  });

  it("uses structured tags and maps missing categories to other", () => {
    expect(releaseCategoryNames(item("a", "ignored", ["武侠", "校园", "校园"])))
      .toEqual(["古风", "校园"]);
    expect(releaseCategoryNames(item("b", ""))).toEqual(["其他"]);
  });

  it("builds stable unique options with other last", () => {
    expect(releaseCategoryOptions([
      item("a", "校园"), item("b", "都市"), item("c", ""), item("d", "校园"),
    ])).toEqual(["校园", "都市", "其他"]);
  });
});
