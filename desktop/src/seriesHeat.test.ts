import { afterEach, expect, it, vi } from "vitest";
import { createSeriesHeatLoader } from "./seriesHeat";

afterEach(() => vi.useRealTimers());

it("coalesces visible cards, deduplicates pending IDs and caps batches and concurrency", async () => {
  vi.useFakeTimers();
  const finish: Array<() => void> = [];
  const fetch = vi.fn((ids: string[]) => new Promise<Record<string, number>>(resolve => finish.push(() => resolve(Object.fromEntries(ids.map(id => [id, Number(id)]))))));
  const load = createSeriesHeatLoader(fetch);
  const values = Array.from({ length: 45 }, (_, i) => load(String(i)));
  expect(load("1")).toBe(values[1]);
  await vi.advanceTimersByTimeAsync(30);
  expect(fetch.mock.calls.map(([ids]) => ids.length)).toEqual([20, 20]);
  finish[0](); finish[1]();
  await vi.advanceTimersByTimeAsync(30);
  expect(fetch.mock.calls.map(([ids]) => ids.length)).toEqual([20, 20, 5]);
  finish[2]();
  expect((await Promise.all(values))[0]).toEqual({ value: 0, failed: false });
  expect(await load("1")).toEqual({ value: 1, failed: false });
  expect(fetch).toHaveBeenCalledTimes(3);
});

it("keeps absent heat absent, expires failed lookups and retries without an immediate request loop", async () => {
  vi.useFakeTimers();
  const fetch = vi.fn().mockRejectedValueOnce(new Error("network")).mockResolvedValueOnce({ "1": 10, "2": -1, "3": NaN });
  const load = createSeriesHeatLoader(fetch);
  const failed = load("1");
  await vi.advanceTimersByTimeAsync(30);
  expect(await failed).toEqual({ value: undefined, failed: true });
  expect((await load("1")).failed).toBe(true);
  expect(fetch).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(15_000);
  const retry = [load("1"), load("2"), load("3"), load("4")];
  await vi.advanceTimersByTimeAsync(30);
  expect((await Promise.all(retry)).map(result => result.value)).toEqual([10, undefined, undefined, undefined]);
  expect(await load("invalid-id")).toEqual({});
  expect(fetch).toHaveBeenCalledTimes(2);
});
