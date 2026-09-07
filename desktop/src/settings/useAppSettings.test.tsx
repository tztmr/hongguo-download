import { act, renderHook, waitFor } from "@testing-library/react";
import type { AIComponentProgress, AIComponentStatus, AppSettings } from "../types";
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
