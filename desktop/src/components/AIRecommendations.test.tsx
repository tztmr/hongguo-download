import { fireEvent, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fetchRank } from "../api";
import type { RankPage, SeriesItem } from "../types";
import { AIRecommendations } from "./AIRecommendations";

vi.mock("../api", () => ({ fetchRank: vi.fn() }));
const item = (id: string, seriesId = id): SeriesItem => ({ bookId: id, seriesId, title: id, cover: "", firstVid: "", episodeCount: 1, score: "", author: "", rankTags: [], contentTypeCode: 1, category: "", abstract: "" });
const rankDefaults: Pick<RankPage, "board" | "boardName" | "releaseType"> = { board: "ranklist_hot_sc", boardName: "AI剧", releaseType: "ai_playlet" };
beforeEach(() => vi.mocked(fetchRank).mockReset());

describe("AIRecommendations", () => {
  it("deduplicates by book and stops when pagination returns an already requested cursor", async () => {
    vi.mocked(fetchRank)
      .mockResolvedValueOnce({ ...rankDefaults, items: [item("第一部", "")], boards: [], hasMore: true, nextCursor: "page-2" })
      .mockResolvedValueOnce({ ...rankDefaults, items: [item("第一部", "new-series"), item("第二部", "")], boards: [], hasMore: true, nextCursor: "page-3" })
      .mockResolvedValueOnce({ ...rankDefaults, items: [item("第三部")], boards: [], hasMore: true, nextCursor: "page-2" });
    const view = render(<AIRecommendations categoryMode={false} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(await view.findByRole("button", { name: "加载更多" }));
    await view.findByRole("heading", { name: "第二部" });
    expect(view.getAllByRole("heading", { name: "第一部" })).toHaveLength(1);
    fireEvent.click(view.getByRole("button", { name: "加载更多" }));
    await view.findByRole("heading", { name: "第三部" });
    expect(view.queryByRole("button", { name: "加载更多" })).toBeNull();
    expect(fetchRank).toHaveBeenCalledTimes(3);
  });

  it("shows structured pagination errors and retries without discarding loaded results", async () => {
    vi.mocked(fetchRank)
      .mockResolvedValueOnce({ ...rankDefaults, items: [item("保留剧")], boards: [], hasMore: true, nextCursor: "page-2" })
      .mockRejectedValueOnce({ payload: { code: "UPSTREAM_ERROR", message: "榜单暂时不可用" } })
      .mockResolvedValueOnce({ ...rankDefaults, items: [item("下一部")], boards: [], hasMore: false, nextCursor: "" });
    const view = render(<AIRecommendations categoryMode={false} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(await view.findByRole("button", { name: "加载更多" }));
    expect((await view.findByRole("alert")).textContent).toContain("榜单暂时不可用");
    expect(view.getByRole("heading", { name: "保留剧" })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "重试" }));
    await view.findByRole("heading", { name: "下一部" });
    await waitFor(() => expect(fetchRank).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: "page-2" })));
  });
});
