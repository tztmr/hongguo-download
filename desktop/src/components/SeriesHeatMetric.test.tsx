import { act, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { loadSeriesHeat } from "../seriesHeat";
import { SeriesHeatMetric } from "./SeriesHeatMetric";

vi.mock("../seriesHeat", async importOriginal => ({
  ...await importOriginal<typeof import("../seriesHeat")>(), loadSeriesHeat: vi.fn(),
}));
afterEach(() => { vi.unstubAllGlobals(); vi.clearAllMocks(); });

it("loads only once when a missing-heat card becomes visible, without a click", async () => {
  let visible!: IntersectionObserverCallback;
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: IntersectionObserverCallback) { visible = callback; }
    observe = vi.fn(); disconnect = vi.fn();
  });
  vi.mocked(loadSeriesHeat).mockResolvedValue({ value: 1250000 });
  const view = render(<SeriesHeatMetric seriesId="101" />);
  expect(loadSeriesHeat).not.toHaveBeenCalled();
  await act(async () => { visible([{ isIntersecting: true }] as IntersectionObserverEntry[], {} as IntersectionObserver); });
  expect(await view.findByText("125万")).toBeTruthy();
  await act(async () => { visible([{ isIntersecting: true }] as IntersectionObserverEntry[], {} as IntersectionObserver); });
  expect(loadSeriesHeat).toHaveBeenCalledTimes(1);
});

it("preserves real zero and ignores a stale lookup after switching the card", async () => {
  let finish!: (result: { value: number }) => void;
  vi.mocked(loadSeriesHeat).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const view = render(<SeriesHeatMetric seriesId="102" />);
  await waitFor(() => expect(loadSeriesHeat).toHaveBeenCalledWith("102"));
  view.rerender(<SeriesHeatMetric seriesId="103" value={0} />);
  await act(async () => finish({ value: 999 }));
  expect(view.getByText("0")).toBeTruthy();
  expect(view.queryByText("999")).toBeNull();
  expect(loadSeriesHeat).toHaveBeenCalledTimes(1);
});
