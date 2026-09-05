import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EpisodeItem, SeriesItem } from "../types";
import { SeriesInspector } from "./SeriesInspector";

const series: SeriesItem = {
  bookId: "book-a",
  seriesId: "book-a",
  title: "天下第一纨绔",
  cover: "",
  firstVid: "",
  contentTypeCode: 1,
  episodeCount: 4,
  abstract: "乱世逆袭",
  score: "",
  category: "真人剧",
  author: "",
  rankTags: [],
};

const episodes: EpisodeItem[] = [1, 2, 3, 4].map((index) => ({ index, itemId: `e${index}`, title: `第 ${index} 集` }));

describe("SeriesInspector", () => {
  it("selects an inclusive episode range", () => {
    const onSelectionChange = vi.fn();
    const view = render(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={[]}
        loading={false}
        onSelectionChange={onSelectionChange}
        onEnqueue={vi.fn()}
      />,
    );

    fireEvent.change(view.getByLabelText("起始集"), { target: { value: "2" } });
    fireEvent.change(view.getByLabelText("结束集"), { target: { value: "4" } });
    fireEvent.click(view.getByRole("button", { name: "选择范围" }));

    expect(onSelectionChange).toHaveBeenCalledWith(["e2", "e3", "e4"]);
  });

  it("supports select all and inverse selection", () => {
    const onSelectionChange = vi.fn();
    const view = render(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={["e1"]}
        loading={false}
        onSelectionChange={onSelectionChange}
        onEnqueue={vi.fn()}
      />,
    );

    fireEvent.click(view.getByRole("button", { name: "全选" }));
    expect(onSelectionChange).toHaveBeenNthCalledWith(1, ["e1", "e2", "e3", "e4"]);
    fireEvent.click(view.getByRole("button", { name: "反选" }));
    expect(onSelectionChange).toHaveBeenNthCalledWith(2, ["e2", "e3", "e4"]);
  });

  it("shows the selected count and only enqueues when at least one episode is selected", () => {
    const onEnqueue = vi.fn();
    const view = render(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={["e1", "e2"]}
        loading={false}
        onSelectionChange={vi.fn()}
        onEnqueue={onEnqueue}
      />,
    );

    const button = view.getByRole("button", { name: "加入下载队列（已选 2 集）" });
    fireEvent.click(button);
    expect(onEnqueue).toHaveBeenCalledTimes(1);

    view.rerender(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={[]}
        loading={false}
        onSelectionChange={vi.fn()}
        onEnqueue={onEnqueue}
      />,
    );
    expect(view.getByRole("button", { name: "加入下载队列（已选 0 集）" })).toHaveProperty("disabled", true);
  });

  it("shows online time and real metrics while preserving zero and missing values", () => {
    const view = render(
      <SeriesInspector
        series={{
          ...series,
          onlineTime: 1_788_321_600,
          playCount: 0,
          hotCount: 25_000,
          collectCount: undefined,
          likeCount: 8,
        }}
        definition="720p"
        episodes={episodes}
        selectedIds={[]}
        loading={false}
        onSelectionChange={vi.fn()}
        onEnqueue={vi.fn()}
      />,
    );

    expect(view.getByText(/720p/)).toBeTruthy();
    expect(view.getByTestId("metric-online").textContent).toContain("上线时间");
    expect(view.getByTestId("metric-play").textContent).toBe("播放量 0");
    expect(view.getByTestId("metric-hot").textContent).toBe("热度量 2.5万");
    expect(view.getByTestId("metric-collect").textContent).toBe("收藏量 —");
    expect(view.getByTestId("metric-like").textContent).toBe("点赞量 8");
  });

  it("distinguishes metric loading and request failure from missing values", () => {
    const view = render(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={[]}
        loading={false}
        metricsLoading={true}
        metricsError=""
        onSelectionChange={vi.fn()}
        onEnqueue={vi.fn()}
      />,
    );

    expect(view.getByRole("status").textContent).toContain("正在加载剧集数据");
    view.rerender(
      <SeriesInspector
        series={series}
        definition="auto"
        episodes={episodes}
        selectedIds={[]}
        loading={false}
        metricsLoading={false}
        metricsError="HTTP 404 Not Found"
        onSelectionChange={vi.fn()}
        onEnqueue={vi.fn()}
      />,
    );
    expect(view.getByRole("alert").textContent).toContain("剧集数据加载失败");
  });
});
