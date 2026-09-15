import { act, renderHook, waitFor } from "@testing-library/react";
import type { AIComponentProgress, AIComponentStatus, AppSettings, DevicePoolStatus } from "../types";
import { describe, expect, it, vi } from "vitest";
import { useAppSettings, type AppSettingsDependencies, type AppSettingsPatch } from "./useAppSettings";

const defaultSettings: AppSettings = {
  version: 4,
  saveDir: "/Downloads/红果下载",
  definition: "auto",
  notifyDownloadComplete: true,
  notifyNewReleases: true,
  demucsModel: "htdemucs",
  whisperModel: "small",
  aiDevice: "auto",
};

function dependencies(overrides: Partial<AppSettingsDependencies> = {}): AppSettingsDependencies {
  return {
    getSettings: vi.fn(async () => defaultSettings),
    updateSettings: vi.fn(async (patch: AppSettingsPatch) => ({ ...defaultSettings, ...patch })),
    chooseSaveDir: vi.fn(async () => "/Downloads/新目录"),
    openSaveDir: vi.fn(async () => undefined),
    getNotificationStatus: vi.fn(async () => "prompt" as const),
    ...overrides,
  };
}

describe("useAppSettings", () => {
  it("does not restore an earlier failed optimistic patch after consecutive failures", async () => {
    const deps = dependencies({ updateSettings: vi.fn().mockRejectedValue(new Error("磁盘写入失败")) });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings).not.toBeNull());
    await act(async () => {
      await Promise.all([result.current.update({ definition: "1080p" }), result.current.update({ notifyNewReleases: false })]);
    });
    expect(result.current.settings?.definition).toBe("auto");
    expect(result.current.settings?.notifyNewReleases).toBe(true);
    expect(result.current.warning).toContain("磁盘写入失败");
  });

  it("keeps later edits visible while an earlier queued save completes", async () => {
    let finishFirst!: (value: AppSettings) => void;
    let finishSecond!: (value: AppSettings) => void;
    const deps = dependencies({ updateSettings: vi.fn()
      .mockReturnValueOnce(new Promise(resolve => { finishFirst = resolve; }))
      .mockReturnValueOnce(new Promise(resolve => { finishSecond = resolve; })) });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings).not.toBeNull());
    let first!: Promise<void>, second!: Promise<void>;
    act(() => { first = result.current.update({ definition: "720p" }); second = result.current.update({ notifyNewReleases: false }); });
    await act(async () => { finishFirst({ ...defaultSettings, definition: "720p" }); await first; });
    expect(result.current.settings?.notifyNewReleases).toBe(false);
    await act(async () => { finishSecond({ ...defaultSettings, definition: "720p", notifyNewReleases: false }); await second; });
  });

  it("loads settings independently of a slow device pool", async () => {
    const deps = dependencies({ getDevicePool: vi.fn(() => new Promise<DevicePoolStatus>(() => {})) });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings?.saveDir).toBe(defaultSettings.saveDir));
    expect(result.current.loading).toBe(false);
    expect(result.current.devicesLoading).toBe(true);
  });

  it("keeps independent load errors and recovers settings through retry", async () => {
    const deps = dependencies({
      getSettings: vi.fn().mockRejectedValueOnce(new Error("设置读取失败")).mockResolvedValue(defaultSettings),
      getAiComponents: vi.fn().mockRejectedValueOnce(new Error("组件暂不可用")).mockResolvedValue([]),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.warning).toContain("设置读取失败");
    expect(result.current.warning).toContain("组件暂不可用");
    act(() => result.current.reload?.());
    await waitFor(() => expect(result.current.settings).toEqual(defaultSettings));
    expect(result.current.warning).toBe("");
  });

  it("loads and refreshes the device identity pool", async () => {
    const initial: DevicePoolStatus = { devices: [], pool_size: 0, active_count: 0 };
    const refreshed: DevicePoolStatus = {
      devices: [{
        device_id: "device-123",
        install_id: "install-456",
        status: "active",
        remaining_seconds: 3600,
        expired: false,
      }],
      pool_size: 1,
      active_count: 1,
    };
    const deps = dependencies({
      getDevicePool: vi.fn(async () => initial),
      refreshDevice: vi.fn(async () => refreshed),
    });
    const { result } = renderHook(() => useAppSettings(deps));

    await waitFor(() => expect(result.current.devicePool).toBe(initial));
    await act(async () => result.current.refreshDevice());

    expect(deps.refreshDevice).toHaveBeenCalledTimes(1);
    expect(result.current.devicePool).toBe(refreshed);
    expect(result.current.devicesLoading).toBe(false);
  });

  it("keeps the last native progress and error when an update fails", async () => {
    const component: AIComponentStatus = { id: "runtime-modern", version: "4", installed: false,
      installedVersion: "3", installedPath: null, downloadBytes: 1024, installedBytes: 2048, inUse: false };
    let progress: ((event: AIComponentProgress) => void) | undefined;
    let rejectInstall: ((reason: unknown) => void) | undefined;
    const failure = { code: "AI_COMPONENT_DOWNLOAD_FAILED", message: "媒体组件下载失败（HTTP 404 Not Found）" };
    const deps = dependencies({
      getAiComponents: vi.fn(async () => [component]),
      subscribeAiComponentProgress: vi.fn(async (listener) => { progress = listener; return () => undefined; }),
      installAiComponent: vi.fn(() => new Promise<AIComponentStatus>((_resolve, reject) => { rejectInstall = reject; })),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.components).toHaveLength(1));
    let operation: Promise<unknown>;
    act(() => { operation = result.current.installComponent(component.id).catch(error => error); });
    act(() => progress?.({ id: component.id, stage: "downloading", percent: 20 }));
    await act(async () => { rejectInstall?.(failure); await operation; });
    expect(result.current.components[0]).toMatchObject({ installed: false, installedVersion: "3", stage: "failed", percent: 20 });
    expect(result.current.warning).toBe(failure.message);
  });

  it("loads components and applies live native progress", async () => {
    const component: AIComponentStatus = {
      id: "runtime",
      version: "1",
      installed: false,
      installedVersion: null,
      installedPath: null,
      downloadBytes: 1024,
      installedBytes: 2048,
      inUse: false,
    };
    let progress: ((event: AIComponentProgress) => void) | undefined;
    const deps = dependencies({
      getAiComponents: vi.fn(async () => [component]),
      subscribeAiComponentProgress: vi.fn(async (listener) => {
        progress = listener;
        return () => undefined;
      }),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.components).toHaveLength(1));

    act(() => progress?.({ id: "runtime", stage: "downloading", percent: 42 }));

    expect(result.current.components[0]).toMatchObject({ stage: "downloading", percent: 42 });
  });

  it("marks a component as checking before the native install resolves", async () => {
    const component: AIComponentStatus = {
      id: "runtime",
      version: "1",
      installed: false,
      installedVersion: null,
      installedPath: null,
      downloadBytes: 1024,
      installedBytes: 2048,
      inUse: false,
    };
    const installed: AIComponentStatus = {
      ...component,
      installed: true,
      installedVersion: "1",
      installedPath: "/AppData/components/runtime/1",
    };
    let release: ((value: AIComponentStatus) => void) | undefined;
    const pending = new Promise<AIComponentStatus>((resolve) => {
      release = resolve;
    });
    const deps = dependencies({
      getAiComponents: vi.fn(async () => [component]),
      installAiComponent: vi.fn(() => pending),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.components).toHaveLength(1));

    act(() => {
      void result.current.installComponent("runtime");
    });
    await waitFor(() => expect(result.current.components[0]?.stage).toBe("checking"));

    await act(async () => {
      release?.(installed);
    });
    await waitFor(() => expect(result.current.components[0]?.installed).toBe(true));
  });

  it("installs and removes components while preserving the returned path", async () => {
    const installed: AIComponentStatus = {
      id: "runtime",
      version: "1",
      installed: true,
      installedVersion: "1",
      installedPath: "/AppData/components/runtime/1",
      downloadBytes: 1024,
      installedBytes: 2048,
      inUse: false,
    };
    const deps = dependencies({
      getAiComponents: vi.fn(async () => []),
      installAiComponent: vi.fn(async () => installed),
      removeAiComponent: vi.fn(async () => undefined),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => result.current.installComponent("runtime"));
    expect(result.current.components[0]?.installedPath).toBe(installed.installedPath);

    await act(async () => result.current.removeComponent("runtime"));
    expect(result.current.components[0]).toMatchObject({ installed: false, installedPath: null });
  });

  it("loads settings and rolls back an update that fails to persist", async () => {
    const deps = dependencies({
      updateSettings: vi.fn(async () => {
        throw new Error("disk full");
      }),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings?.definition).toBe("auto"));

    await act(async () => {
      await result.current.update({ definition: "720p" });
    });

    expect(result.current.settings?.definition).toBe("auto");
    expect(result.current.warning).toContain("disk full");
  });

  it("extracts the message from a structured native error", async () => {
    const deps = dependencies({
      getSettings: vi.fn(async () => {
        throw { code: "SETTINGS_LOAD_FAILED", message: "无法读取设置" };
      }),
    });
    const { result } = renderHook(() => useAppSettings(deps));

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.warning).toBe("无法读取设置");
    expect(result.current.warning).not.toBe("[object Object]");
  });

  it("serializes rapid updates so later changes are not overwritten", async () => {
    const order: string[] = [];
    let server = defaultSettings;
    const deps = dependencies({
      updateSettings: vi.fn(async (patch: AppSettingsPatch) => {
        order.push(`start:${Object.keys(patch)[0]}`);
        await Promise.resolve();
        server = { ...server, ...patch };
        order.push(`end:${Object.keys(patch)[0]}`);
        return server;
      }),
    });
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings).not.toBeNull());

    await act(async () => {
      await Promise.all([
        result.current.update({ definition: "1080p" }),
        result.current.update({ notifyNewReleases: false }),
      ]);
    });

    expect(order).toEqual([
      "start:definition",
      "end:definition",
      "start:notifyNewReleases",
      "end:notifyNewReleases",
    ]);
    expect(result.current.settings?.definition).toBe("1080p");
    expect(result.current.settings?.notifyNewReleases).toBe(false);
  });

  it("uses the persisted directory returned by the native chooser", async () => {
    const deps = dependencies();
    const { result } = renderHook(() => useAppSettings(deps));
    await waitFor(() => expect(result.current.settings).not.toBeNull());

    await act(async () => {
      await result.current.chooseDirectory();
    });

    expect(result.current.settings?.saveDir).toBe("/Downloads/新目录");
  });
});
