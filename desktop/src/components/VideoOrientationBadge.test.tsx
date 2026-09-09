import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { VideoOrientationBadge } from "./VideoOrientationBadge";

const { apiGet } = vi.hoisted(() => ({ apiGet: vi.fn() }));
vi.mock("../api", () => ({ apiGet }));
let intersect: (entries: Array<{ isIntersecting: boolean }>) => void;

beforeEach(() => {
  apiGet.mockReset();
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: typeof intersect) { intersect = callback; }
    observe() {}
    disconnect() {}
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("detects a visible poster from video dimensions, even when its cover is portrait", async () => {
  apiGet.mockResolvedValue({ sources: [{ width: 1920, height: 1080 }] });
  const view = render(<div className="poster-image"><img width="200" height="280" alt="宣传图" /><span>NO.1</span><VideoOrientationBadge seriesId="wide" firstVid="wide-video" /></div>);
  expect(view.queryByText("横屏")).toBeNull();
  act(() => intersect([{ isIntersecting: true }]));
  await waitFor(() => expect(view.getByText("横屏")).toBeTruthy());
  expect(view.getByText("NO.1")).toBeTruthy();
});

it("does not let an old request overwrite a different series", async () => {
  let finishOld!: (data: unknown) => void;
  apiGet.mockImplementation((path: string) => path.includes("old-video")
    ? new Promise((resolve) => { finishOld = resolve; })
    : Promise.resolve({ sources: [{ width: 720, height: 1280 }] }));
  const view = render(<VideoOrientationBadge seriesId="old" firstVid="old-video" />);
  act(() => intersect([{ isIntersecting: true }]));
  view.rerender(<VideoOrientationBadge seriesId="new" firstVid="new-video" />);
  act(() => intersect([{ isIntersecting: true }]));
  await waitFor(() => expect(view.getByText("竖屏")).toBeTruthy());
  await act(async () => finishOld({ sources: [{ width: 1920, height: 1080 }] }));
  expect(view.getByText("竖屏")).toBeTruthy();
  expect(view.queryByText("横屏")).toBeNull();
});

it("shows an unknown state after metadata fails instead of assuming portrait", async () => {
  apiGet.mockRejectedValue(new Error("offline"));
  const view = render(<VideoOrientationBadge seriesId="offline" firstVid="offline-video" />);
  act(() => intersect([{ isIntersecting: true }]));
  await waitFor(() => expect(view.getByText("方向未知")).toBeTruthy());
  expect(view.queryByText("竖屏")).toBeNull();
});
