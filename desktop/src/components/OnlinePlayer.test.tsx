import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadEpisodeVideo } from "../playback";
import { OnlinePlayer } from "./OnlinePlayer";

vi.mock("../playback", () => ({ loadEpisodeVideo: vi.fn() }));
const episodes = [1, 2, 3].map((index) => ({ index, itemId: `e${index}`, title: `第 ${index} 集` }));

describe("OnlinePlayer", () => {
  beforeEach(() => {
    vi.mocked(loadEpisodeVideo).mockReset().mockResolvedValue(new Blob(["video"], { type: "video/mp4" }));
    vi.stubGlobal("URL", class extends URL {
      static createObjectURL = vi.fn(() => "blob:episode");
      static revokeObjectURL = vi.fn();
    });
  });
  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

  it("starts at the requested episode, switches and releases media on close", async () => {
    const view = render(<OnlinePlayer title="测试剧" episodes={episodes} initialItemId="e2" definition="720p" onClose={vi.fn()} />);
    await waitFor(() => expect(view.container.querySelector("video")?.getAttribute("src")).toBe("blob:episode"));
    expect(loadEpisodeVideo).toHaveBeenCalledWith("e2", "720p", expect.any(AbortSignal));
    const signal = vi.mocked(loadEpisodeVideo).mock.calls[0][2];
    fireEvent.click(view.getByRole("button", { name: "下一集" }));
    await waitFor(() => expect(loadEpisodeVideo).toHaveBeenCalledWith("e3", "720p", expect.any(AbortSignal)));
    expect(signal.aborted).toBe(true);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:episode");
    expect(view.getByRole("button", { name: "下一集" })).toHaveProperty("disabled", true);
    view.unmount();
    expect(vi.mocked(loadEpisodeVideo).mock.calls[1][2].aborted).toBe(true);
  });

  it("shows request failures and retries the same episode", async () => {
    vi.mocked(loadEpisodeVideo).mockRejectedValueOnce(new Error("视频暂不可用"));
    const view = render(<OnlinePlayer title="测试剧" episodes={episodes} initialItemId="e1" definition="auto" onClose={vi.fn()} />);
    await waitFor(() => expect(view.getByRole("alert").textContent).toContain("视频暂不可用"));
    fireEvent.click(view.getByRole("button", { name: "重试播放" }));
    await waitFor(() => expect(view.container.querySelector("video")).not.toBeNull());
    expect(loadEpisodeVideo).toHaveBeenCalledTimes(2);
  });

  it("ignores a late response from the previous episode", async () => {
    let resolve!: (blob: Blob) => void;
    vi.mocked(loadEpisodeVideo).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const view = render(<OnlinePlayer title="测试剧" episodes={episodes} initialItemId="e1" definition="auto" onClose={vi.fn()} />);
    fireEvent.click(view.getByRole("button", { name: "下一集" }));
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(1));
    await act(async () => { resolve(new Blob(["stale"])); });
    expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
    expect(view.getByLabelText("播放剧集")).toHaveProperty("value", "e2");
  });
});
