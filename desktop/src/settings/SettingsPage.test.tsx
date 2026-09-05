import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UseAppSettingsResult } from "./useAppSettings";
import { SettingsPage } from "./SettingsPage";

function model(overrides: Partial<UseAppSettingsResult> = {}): UseAppSettingsResult {
  return {
    settings: {
      version: 2,
      saveDir: "/Downloads/红果下载",
      definition: "auto",
      notifyDownloadComplete: true,
      notifyNewReleases: true,
      demucsModel: "htdemucs",
      whisperModel: "small",
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

    fireEvent.click(view.getByRole("button", { name: "选择目录" }));
    fireEvent.click(view.getByRole("radio", { name: /^720p/ }));
    fireEvent.click(view.getByRole("checkbox", { name: /新剧通知/ }));

    expect(settings.chooseDirectory).toHaveBeenCalledTimes(1);
    expect(settings.update).toHaveBeenCalledWith({ definition: "720p" });
    expect(settings.update).toHaveBeenCalledWith({ notifyNewReleases: false });
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
    expect(view.getByText(/downloading/i)).toBeTruthy();
    expect(view.getByText(/42%/)).toBeTruthy();
    const deletes = view.getAllByRole("button", { name: "删除模型" });
    expect(deletes.every((button) => (button as HTMLButtonElement).disabled)).toBe(true);
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

    fireEvent.click(view.getByRole("button", { name: "安装 / 重下" }));

    expect(confirm).toHaveBeenCalledWith(expect.stringMatching(/runtime[\s\S]*1 KB[\s\S]*2 KB/));
    expect(settings.installComponent).not.toHaveBeenCalled();
    confirm.mockRestore();
  });
});
