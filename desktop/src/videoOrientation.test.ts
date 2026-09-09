import { beforeEach, describe, expect, it, vi } from "vitest";

const { apiGet } = vi.hoisted(() => ({ apiGet: vi.fn() }));
vi.mock("./api", () => ({ apiGet }));

beforeEach(() => {
  vi.resetModules();
  apiGet.mockReset();
});

describe("video orientation detection", () => {
  it.each([
    [1920, 1080, "横屏"],
    [720, 1280, "竖屏"],
    [1080, 1080, "方屏"],
    [0, 1280, "未知"],
    [-720, 1280, "未知"],
    ["invalid", 1280, "未知"],
    ["1920", "1080", "横屏"],
  ])("classifies video dimensions %s × %s as %s", async (width, height, expected) => {
    apiGet.mockResolvedValue({ sources: [{ width, height }] });
    const { detectVideoOrientation } = await import("./videoOrientation");
    expect(await detectVideoOrientation({ seriesId: "series", firstVid: "video" })).toBe(expected);
  });

  it("uses a valid source when another definition has no dimensions", async () => {
    apiGet.mockResolvedValue({ sources: [{ width: 0, height: 0 }, { width: 1080, height: 1920 }] });
    const { detectVideoOrientation } = await import("./videoOrientation");
    expect(await detectVideoOrientation({ seriesId: "series", firstVid: "video" })).toBe("竖屏");
  });

  it("looks up the first episode when the card has no video ID", async () => {
    apiGet.mockImplementation(async (path: string) => {
      if (path === "/api/duanju/catalog?book_id=series") return { items: [{ item_id: "episode-1" }] };
      if (path === "/api/duanju/content?item_id=episode-1") return { sources: [{ width: 1920, height: 1080 }] };
      throw new Error(`Unexpected request: ${path}`);
    });
    const { detectVideoOrientation } = await import("./videoOrientation");
    expect(await detectVideoOrientation({ seriesId: "series", firstVid: "" })).toBe("横屏");
  });

  it("reuses detected dimensions across cards and retries after a failure", async () => {
    apiGet.mockRejectedValueOnce(new Error("offline"));
    const { detectVideoOrientation } = await import("./videoOrientation");
    const series = { seriesId: "series", firstVid: "video" };
    expect(await detectVideoOrientation(series)).toBe("未知");
    apiGet.mockResolvedValueOnce({ sources: [{ width: 1920, height: 1080 }] });
    expect(await detectVideoOrientation(series)).toBe("横屏");
    apiGet.mockRejectedValue(new Error("offline again"));
    expect(await detectVideoOrientation(series)).toBe("横屏");
  });

  it("bounds concurrent metadata requests and drains the queue", async () => {
    let active = 0;
    let peak = 0;
    const releases: Array<() => void> = [];
    apiGet.mockImplementation(() => new Promise((resolve) => {
      active++;
      peak = Math.max(peak, active);
      releases.push(() => {
        active--;
        resolve({ sources: [{ width: 1920, height: 1080 }] });
      });
    }));
    const { detectVideoOrientation } = await import("./videoOrientation");
    const requests = Array.from({ length: 8 }, (_, index) => detectVideoOrientation({ seriesId: `${index}`, firstVid: `${index}` }));
    for (let index = 0; index < 8; index++) {
      await vi.waitFor(() => expect(releases.length).toBeGreaterThan(0));
      releases.shift()!();
    }
    expect(await Promise.all(requests)).toEqual(Array(8).fill("横屏"));
    expect(peak).toBeLessThanOrEqual(3);
  });
});
