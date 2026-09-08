import { fireEvent, render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UseAppSettingsResult } from "./useAppSettings";
import { SettingsPage } from "./SettingsPage";

function model(overrides: Partial<UseAppSettingsResult> = {}): UseAppSettingsResult {
  return {
    settings: {
      version: 4,
      saveDir: "/Downloads/红果下载",
      definition: "auto",
      notifyDownloadComplete: true,
      notifyNewReleases: true,
      demucsModel: "htdemucs",
      whisperModel: "small",
      aiDevice: "auto",
    },
    loading: false,
    warning: "上次设置文件损坏，已使用默认值",
    notificationPermission: "denied",
    components: [],
    update: vi.fn(async () => undefined),
    chooseDirectory: vi.fn(async () => undefined),
    openDirectory: vi.fn(async () => undefined),
    installComponent: vi.fn(async () => undefined),
    removeComponent: vi.fn(async () => undefined),
    ...overrides,
  };
}

describe("SettingsPage", () => {
  it("renders path, resolution choices, notification switches and permission guidance", () => {
    const settings = model();
    const view = render(<SettingsPage model={settings} />);

    expect(view.getByText("/Downloads/红果下载")).toBeTruthy();
    expect((view.getByRole("radio", { name: /自动最高/ }) as HTMLInputElement).checked).toBe(true);
    expect(view.getByRole("radio", { name: /^1080p/ })).toBeTruthy();
    expect(view.getByRole("radio", { name: /^720p/ })).toBeTruthy();
    expect((view.getByRole("checkbox", { name: /下载完成通知/ }) as HTMLInputElement).checked).toBe(true);
    expect((view.getByRole("checkbox", { name: /新剧通知/ }) as HTMLInputElement).checked).toBe(true);
    expect(view.getByText(/macOS 系统设置/)).toBeTruthy();
    expect(view.getByText(/上次设置文件损坏/)).toBeTruthy();
    expect((view.getByRole("radio", { name: /自动选择计算设备/ }) as HTMLInputElement).checked).toBe(true);

    fireEvent.click(view.getByRole("button", { name: "选择目录" }));
    fireEvent.click(view.getByRole("radio", { name: /^720p/ }));
    fireEvent.click(view.getByRole("checkbox", { name: /新剧通知/ }));
    fireEvent.click(view.getByRole("radio", { name: /NVIDIA GPU/ }));

    expect(settings.chooseDirectory).toHaveBeenCalledTimes(1);
    expect(settings.update).toHaveBeenCalledWith({ definition: "720p" });
    expect(settings.update).toHaveBeenCalledWith({ notifyNewReleases: false });
    expect(settings.update).toHaveBeenCalledWith({ aiDevice: "cuda" });
  });

  it("shows component progress and refuses model deletion while in use", () => {
    const settings = model({
      components: [
        {
          id: "runtime",
          version: "1",
          installed: true,
          installedVersion: "1",
          installedPath: "/AppData/components/runtime/1",
          downloadBytes: 10,
          installedBytes: 12,
          inUse: true,
          stage: "downloading",
          percent: 42,
        },
        {
          id: "demucs-htdemucs",
          version: "1",
          installed: true,
          installedVersion: "1",
          installedPath: "/AppData/components/demucs-htdemucs/1",
          downloadBytes: 20,
          installedBytes: 22,
          inUse: true,
        },
      ],
    });
    const view = render(<SettingsPage model={settings} />);
    expect(view.getByText("htdemucs")).toBeTruthy();
    expect(view.getByText("/AppData/components/runtime/1")).toBeTruthy();
    expect(view.getAllByText(/正在下载/).length).toBeGreaterThan(0);
    expect(view.getByText(/42%/)).toBeTruthy();
    expect(view.getByRole("progressbar", { name: "runtime 下载进度" }).getAttribute("aria-valuenow")).toBe("42");
    const deletes = view.getAllByRole("button", { name: "删除模型" });
    expect(deletes.every((button) => (button as HTMLButtonElement).disabled)).toBe(true);
  });

  it("saves proxy and mainland mirror settings", () => {
    const settings = model();
    const view = render(<SettingsPage model={settings} />);
    fireEvent.change(view.getByPlaceholderText(/127\.0\.0\.1:7890/), { target: { value: "socks5://127.0.0.1:7890" } });
    fireEvent.change(view.getByPlaceholderText(/你的镜像域名/), { target: { value: "https://mirror.example/ai/" } });
    fireEvent.click(view.getByRole("button", { name: "保存网络设置" }));
    expect(settings.update).toHaveBeenCalledWith({
      downloadProxy: "socks5://127.0.0.1:7890",
      downloadMirror: "https://mirror.example/ai/",
    });
  });

  it("confirms component download and installed sizes before starting network work", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const settings = model({
      components: [{
        id: "runtime",
        version: "1",
        installed: false,
        installedVersion: null,
        installedPath: null,
        downloadBytes: 1024,
        installedBytes: 2048,
        inUse: false,
      }],
    });
    const view = render(<SettingsPage model={settings} />);

    fireEvent.click(view.getByRole("button", { name: "下载" }));

    expect(confirm).toHaveBeenCalledWith(expect.stringMatching(/runtime[\s\S]*1 KB[\s\S]*2 KB/));
    expect(settings.installComponent).not.toHaveBeenCalled();
    confirm.mockRestore();
  });

  it("keeps settings interactive after confirming a selected component download", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    let finish: (() => void) | undefined;
    const installComponent = vi.fn(() => new Promise<void>((resolve) => {
      finish = resolve;
    }));
    const settings = model({
      components: [{
        id: "runtime",
        version: "1",
        installed: false,
        installedVersion: null,
        installedPath: null,
        downloadBytes: 1024,
        installedBytes: 2048,
        inUse: false,
      }],
      installComponent,
    });
    const view = render(<SettingsPage model={settings} />);
    fireEvent.click(view.getByRole("checkbox", { name: "选择 runtime 下载" }));
    fireEvent.click(view.getByRole("button", { name: /下载选中组件/ }));
    await waitFor(() => expect(view.getByText("下载中…")).toBeTruthy());
    expect((view.getByRole("button", { name: "下载中…" }) as HTMLButtonElement).disabled).toBe(true);
    expect((view.getByRole("radio", { name: /自动最高/ }) as HTMLInputElement).disabled).toBe(false);
    finish?.();
    await waitFor(() => expect(installComponent).toHaveBeenCalledWith("runtime"));
    confirm.mockRestore();
  });

  it("downloads only the components selected in settings", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const installComponent = vi.fn(async (_id: string): Promise<void> => undefined);
    const settings = model({
      components: [
        {
          id: "runtime",
          version: "1",
          installed: false,
          installedVersion: null,
          installedPath: null,
          downloadBytes: 1024,
          installedBytes: 2048,
          inUse: false,
        },
        {
          id: "whisper-small",
          version: "1",
          installed: false,
          installedVersion: null,
          installedPath: null,
          downloadBytes: 4096,
          installedBytes: 8192,
          inUse: false,
        },
      ],
      installComponent,
    });
    const view = render(<SettingsPage model={settings} />);

    fireEvent.click(view.getByRole("checkbox", { name: "选择 runtime 下载" }));
    fireEvent.click(view.getByRole("button", { name: /下载选中组件/ }));

    await waitFor(() => expect(installComponent).toHaveBeenCalledWith("runtime"));
    expect(installComponent).toHaveBeenCalledTimes(1);
    expect(confirm).toHaveBeenCalledWith(expect.stringMatching(/runtime/));
    confirm.mockRestore();
  });
  it("selects only missing components for current models and reports installation failures", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const settings = model({ components: ["runtime", "demucs-htdemucs", "whisper-small", "whisper-medium"].map((id) => ({
      id, version: "1", installed: id === "runtime", installedVersion: null, installedPath: null,
      downloadBytes: 1024, installedBytes: 2048, inUse: false,
    })), installComponent: vi.fn().mockRejectedValue(new Error("组件下载失败")) });
    const view = render(<SettingsPage model={settings} />);
    fireEvent.click(view.getByRole("button", { name: "选择当前模型所需组件" }));
    expect((view.getByRole("checkbox", { name: "选择 runtime 下载" }) as HTMLInputElement).checked).toBe(false);
    expect((view.getByRole("checkbox", { name: "选择 demucs-htdemucs 下载" }) as HTMLInputElement).checked).toBe(true);
    expect((view.getByRole("checkbox", { name: "选择 whisper-small 下载" }) as HTMLInputElement).checked).toBe(true);
    expect((view.getByRole("checkbox", { name: "选择 whisper-medium 下载" }) as HTMLInputElement).checked).toBe(false);
    expect(view.getByText("下载 2 KB · 安装后占用 4 KB")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /下载选中组件/ }));
    await waitFor(() => expect(view.getByRole("alert").textContent).toBe("组件下载失败"));
    expect(settings.installComponent).toHaveBeenCalledTimes(1);
    vi.mocked(window.confirm).mockRestore();
  });

  it("uses checked defaults when older settings omit model preferences", () => {
    const settings = model();
    Reflect.deleteProperty(settings.settings!, "demucsModel");
    Reflect.deleteProperty(settings.settings!, "whisperModel");
    const view = render(<SettingsPage model={settings} />);
    expect((view.getByRole("radio", { name: /标准模式/ }) as HTMLInputElement).checked).toBe(true);
    expect((view.getByRole("radio", { name: /轻量模式/ }) as HTMLInputElement).checked).toBe(true);
    fireEvent.click(view.getByRole("radio", { name: /高精度模式/ }));
    expect(settings.update).toHaveBeenCalledWith({ whisperModel: "medium" });
  });

  it("selects the CPU runtime when Windows AI is pinned to CPU", () => {
    const settings = model({
      settings: { ...model().settings!, aiDevice: "cpu" },
      components: ["runtime-modern", "runtime-cpu", "demucs-htdemucs", "whisper-small"].map((id) => ({
        id, version: "1", installed: false, installedVersion: null, installedPath: null,
        downloadBytes: 1024, installedBytes: 2048, inUse: false,
      })),
    });
    const view = render(<SettingsPage model={settings} />);

    fireEvent.click(view.getByRole("button", { name: "选择当前模型所需组件" }));

    expect((view.getByRole("checkbox", { name: "选择 runtime-cpu 下载" }) as HTMLInputElement).checked).toBe(true);
    expect((view.getByRole("checkbox", { name: "选择 runtime-modern 下载" }) as HTMLInputElement).checked).toBe(false);
  });

});
