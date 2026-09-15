import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../types";
import { SettingsPage } from "./SettingsPage";
import { useAppSettings, type AppSettingsDependencies } from "./useAppSettings";

const initial: AppSettings = { version: 4, saveDir: "/Downloads", definition: "auto", notifyDownloadComplete: true, notifyNewReleases: true,
  demucsModel: "htdemucs", whisperModel: "small", aiDevice: "auto", downloadProxy: "http://127.0.0.1:8080", downloadMirror: "" };
function dependencies(): AppSettingsDependencies {
  return { getSettings: vi.fn().mockResolvedValue(initial), updateSettings: vi.fn(async patch => ({ ...initial, ...patch })),
    chooseSaveDir: vi.fn().mockResolvedValue(initial.saveDir), openSaveDir: vi.fn().mockResolvedValue(undefined), getNotificationStatus: vi.fn().mockResolvedValue("granted") };
}
function Harness({ api }: { api: AppSettingsDependencies }) { return <SettingsPage model={useAppSettings(api)} />; }
function deferred() {
  let resolve!: (value: AppSettings) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<AppSettings>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

describe("settings save feedback", () => {
  it("blocks duplicate submissions and retains edits made while the previous save is pending", async () => {
    const api = dependencies(), pending = deferred();
    vi.mocked(api.updateSettings).mockReturnValueOnce(pending.promise);
    const view = render(<Harness api={api} />);
    const input = await view.findByRole("textbox", { name: "代理地址" });
    const network = within(view.getByRole("heading", { name: "下载网络" }).closest("section")!);
    fireEvent.change(input, { target: { value: " http://127.0.0.1:7890 " } });
    expect(network.getByRole("status").textContent).toContain("未保存");
    const save = network.getByRole("button", { name: "保存网络设置" });
    fireEvent.click(save); fireEvent.click(save);
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalledTimes(1));
    expect(network.getByRole("button", { name: "恢复直连" })).toHaveProperty("disabled", true);
    expect(network.getByRole("status").textContent).toBe("正在保存网络设置…");
    fireEvent.change(input, { target: { value: "http://127.0.0.1:7891" } });
    await act(async () => pending.resolve({ ...initial, downloadProxy: "http://127.0.0.1:7890" }));
    expect(input).toHaveProperty("value", "http://127.0.0.1:7891");
    expect(network.getByRole("status").textContent).toContain("仍有新的修改未保存");
    fireEvent.click(network.getByRole("button", { name: "保存网络设置" }));
    await network.findByText("网络设置已保存");
    expect(api.updateSettings).toHaveBeenLastCalledWith({ downloadProxy: "http://127.0.0.1:7891", downloadMirror: "" });
    expect(network.getByRole("button", { name: "保存网络设置" })).toHaveProperty("disabled", true);
  });

  it("retains the failed draft and reports success only after retry is persisted", async () => {
    const api = dependencies();
    vi.mocked(api.updateSettings).mockRejectedValueOnce({ message: "磁盘写入失败" });
    const view = render(<Harness api={api} />);
    const input = await view.findByRole("textbox", { name: "代理地址" });
    const network = within(view.getByRole("heading", { name: "下载网络" }).closest("section")!);
    fireEvent.change(input, { target: { value: "http://127.0.0.1:7890" } });
    fireEvent.click(network.getByRole("button", { name: "保存网络设置" }));
    const retry = await network.findByRole("button", { name: "重试保存网络设置" });
    expect(input).toHaveProperty("value", "http://127.0.0.1:7890");
    expect(network.getByRole("status").textContent).toContain("磁盘写入失败");
    expect(network.queryByText("网络设置已保存")).toBeNull();
    fireEvent.click(retry);
    await network.findByText("网络设置已保存");
    expect(api.updateSettings).toHaveBeenCalledTimes(2);
  });

  it("keeps a failed restore to direct connection editable and retryable", async () => {
    const api = dependencies();
    vi.mocked(api.updateSettings).mockRejectedValueOnce(new Error("设置暂时锁定"));
    const view = render(<Harness api={api} />);
    const input = await view.findByRole("textbox", { name: "代理地址" });
    fireEvent.click(view.getByRole("button", { name: "恢复直连" }));
    fireEvent.click(await view.findByRole("button", { name: "重试保存网络设置" }));
    await view.findByText("网络设置已保存");
    expect(input).toHaveProperty("value", "");
    expect(api.updateSettings).toHaveBeenLastCalledWith({ downloadProxy: "", downloadMirror: "" });
  });
});
