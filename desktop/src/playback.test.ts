import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { loadEpisodeVideo } from "./playback";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { vi.unstubAllGlobals(); vi.clearAllMocks(); });

describe("loadEpisodeVideo", () => {
  it("fetches playable bytes from the desktop API using the selected quality", async () => {
    vi.mocked(invoke).mockResolvedValue("http://127.0.0.1:12345/api/duanju/download?item_id=e1&definition=720p");
    const blob = new Blob(["video"], { type: "video/mp4" });
    const fetchVideo = vi.fn().mockResolvedValue({ ok: true, headers: new Headers({ "content-type": "video/mp4" }), blob: async () => blob });
    vi.stubGlobal("fetch", fetchVideo);
    const signal = new AbortController().signal;
    expect(await loadEpisodeVideo("e1", "720p", signal)).toBe(blob);
    expect(invoke).toHaveBeenCalledWith("get_playback_url", { itemId: "e1", definition: "720p" });
    expect(fetchVideo).toHaveBeenCalledWith(expect.stringContaining("127.0.0.1:12345"), { signal, cache: "no-store" });
  });

  it("surfaces backend errors instead of treating JSON as a video", async () => {
    vi.mocked(invoke).mockResolvedValue("http://localhost/video");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false, status: 502, json: async () => ({ msg: "没有可用视频源" }) }));
    await expect(loadEpisodeVideo("e1", "auto", new AbortController().signal)).rejects.toThrow("没有可用视频源");
  });

  it("does not start fetching when closed while resolving the URL", async () => {
    vi.mocked(invoke).mockResolvedValue("http://localhost/video");
    const fetchVideo = vi.fn();
    vi.stubGlobal("fetch", fetchVideo);
    const controller = new AbortController();
    const pending = loadEpisodeVideo("e1", "auto", controller.signal);
    controller.abort();
    await expect(pending).rejects.toThrow();
    expect(fetchVideo).not.toHaveBeenCalled();
  });
});
