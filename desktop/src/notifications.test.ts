import { createNotificationAdapter, type NotificationRuntime } from "./notifications";
import { describe, expect, it, vi } from "vitest";

function fakeRuntime(options: { granted: boolean; requestResult?: "granted" | "denied" }) {
  let granted = options.granted;
  const runtime: NotificationRuntime = {
    isPermissionGranted: vi.fn(async () => granted),
    requestPermission: vi.fn(async () => {
      const result = options.requestResult ?? "denied";
      granted = result === "granted";
      return result;
    }),
    sendNotification: vi.fn(),
  };
  return runtime;
}

describe("notification adapter", () => {
  it("requests permission once and sends every notification after grant", async () => {
    const runtime = fakeRuntime({ granted: false, requestResult: "granted" });
    const adapter = createNotificationAdapter(runtime);

    await expect(adapter.send({ title: "发现新剧", body: "新增 2 部", target: { kind: "monitor", id: "playlet" } })).resolves.toBe(true);
    await expect(adapter.send({ title: "下载完成", body: "共 60 集", target: { kind: "downloadBatch", id: "batch-1" } })).resolves.toBe(true);

    expect(runtime.requestPermission).toHaveBeenCalledTimes(1);
    expect(runtime.sendNotification).toHaveBeenNthCalledWith(1, {
      title: "发现新剧",
      body: "新增 2 部",
      extra: { targetKind: "monitor", targetId: "playlet" },
    });
    expect(runtime.sendNotification).toHaveBeenCalledTimes(2);
    await expect(adapter.getStatus()).resolves.toBe("granted");
  });

  it("does not request again or throw after permission is denied", async () => {
    const runtime = fakeRuntime({ granted: false, requestResult: "denied" });
    const adapter = createNotificationAdapter(runtime);

    await expect(adapter.send({ title: "发现新剧", body: "新增 1 部", target: { kind: "monitor", id: "playlet" } })).resolves.toBe(false);
    await expect(adapter.send({ title: "发现新剧", body: "新增 2 部", target: { kind: "monitor", id: "playlet" } })).resolves.toBe(false);

    expect(runtime.requestPermission).toHaveBeenCalledTimes(1);
    expect(runtime.sendNotification).not.toHaveBeenCalled();
    await expect(adapter.getStatus()).resolves.toBe("denied");
  });

  it("contains runtime send failures", async () => {
    const runtime = fakeRuntime({ granted: true });
    vi.mocked(runtime.sendNotification).mockImplementation(() => {
      throw new Error("native unavailable");
    });
    const adapter = createNotificationAdapter(runtime);

    await expect(adapter.send({ title: "下载完成", body: "共 1 集", target: { kind: "downloadBatch", id: "batch-1" } })).resolves.toBe(false);
  });

  it("routes only bounded safe action targets and unregisters the listener", async () => {
    let action: ((value: { extra?: Record<string, unknown> }) => void) | undefined;
    const unregister = vi.fn().mockResolvedValue(undefined);
    const runtime = fakeRuntime({ granted: true });
    runtime.onAction = vi.fn(async (listener) => { action = listener; return { unregister }; });
    const adapter = createNotificationAdapter(runtime);
    const listener = vi.fn();
    const stop = await adapter.subscribeActions!(listener);
    action?.({ extra: { targetKind: "mediaJob", targetId: "job-1" } });
    action?.({ extra: { targetKind: "mediaJob", targetId: "/Users/secret" } });
    expect(listener).toHaveBeenCalledTimes(1);
    expect(listener).toHaveBeenCalledWith({ kind: "mediaJob", id: "job-1" });
    stop();
    expect(unregister).toHaveBeenCalledTimes(1);
  });
});
