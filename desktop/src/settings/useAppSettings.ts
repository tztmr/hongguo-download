import { useCallback, useEffect, useRef, useState } from "react";
import {
  chooseSaveDir,
  fetchDevicePool,
  getAiComponents,
  getSettings,
  installAiComponent,
  openSaveDir,
  removeAiComponent,
  refreshDevice as refreshDeviceApi,
  subscribeAiComponentProgress,
  updateSettings,
} from "../api";
import { createTauriNotificationAdapter, type NotificationStatus } from "../notifications";
import type { AIComponentProgress, AIComponentStatus, AppSettings, DevicePoolStatus } from "../types";
import { errorMessage } from "../errors";
export { errorMessage } from "../errors";

export type AppSettingsPatch = Partial<
  Omit<AppSettings, "version" | "warning">
>;

export type AppSettingsDependencies = {
  getSettings(): Promise<AppSettings>;
  updateSettings(patch: AppSettingsPatch): Promise<AppSettings>;
  chooseSaveDir(): Promise<string>;
  openSaveDir(): Promise<void>;
  getNotificationStatus(): Promise<NotificationStatus>;
  getDevicePool?(): Promise<DevicePoolStatus>;
  refreshDevice?(): Promise<DevicePoolStatus>;
  getAiComponents?(): Promise<AIComponentStatus[]>;
  installAiComponent?(id: string): Promise<AIComponentStatus>;
  removeAiComponent?(id: string): Promise<void>;
  subscribeAiComponentProgress?(listener: (progress: AIComponentProgress) => void): Promise<() => void>;
};

export type UseAppSettingsResult = {
  settings: AppSettings | null;
  loading: boolean;
  warning: string;
  notificationPermission: NotificationStatus;
  components: AIComponentStatus[];
  devicePool: DevicePoolStatus | null;
  devicesLoading: boolean;
  update(patch: AppSettingsPatch): Promise<void>;
  chooseDirectory(): Promise<void>;
  openDirectory(): Promise<void>;
  installComponent(id: string): Promise<void>;
  removeComponent(id: string): Promise<void>;
  refreshDevice(): Promise<void>;
  reload?(): void;
};

const notifications = createTauriNotificationAdapter();
const defaultDependencies: AppSettingsDependencies = {
  getSettings,
  updateSettings,
  chooseSaveDir,
  openSaveDir,
  getNotificationStatus: () => notifications.getStatus(),
  getDevicePool: fetchDevicePool,
  getAiComponents,
  installAiComponent,
  removeAiComponent,
  refreshDevice: refreshDeviceApi,
  subscribeAiComponentProgress,
};

export function useAppSettings(
  dependencies: AppSettingsDependencies = defaultDependencies,
): UseAppSettingsResult {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [warning, setWarning] = useState("");
  const [componentWarning, setComponentWarning] = useState("");
  const [notificationPermission, setNotificationPermission] =
    useState<NotificationStatus>("prompt");
  const [components, setComponents] = useState<AIComponentStatus[]>([]);
  const [devicePool, setDevicePool] = useState<DevicePoolStatus | null>(null);
  const [devicesLoading, setDevicesLoading] = useState(true);
  const confirmedSettings = useRef<AppSettings | null>(null);
  const pendingPatches = useRef<AppSettingsPatch[]>([]);
  const batchWarning = useRef("");
  const directoryPending = useRef(false);
  const updateQueue = useRef<Promise<void>>(Promise.resolve());
  const [loadRevision, setLoadRevision] = useState(0);

  const publishSettings = useCallback(() => {
    if (!confirmedSettings.current) return;
    const value = pendingPatches.current.reduce<AppSettings>((current, patch) => ({ ...current, ...patch }), confirmedSettings.current);
    setSettings(value);
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true); setDevicesLoading(true); setWarning(""); setComponentWarning("");
    void dependencies.getSettings()
      .then(value => {
        if (!active) return;
        confirmedSettings.current = value;
        publishSettings();
        setWarning(value.warning || "");
      })
      .catch((error) => {
        if (active) setWarning(errorMessage(error));
      })
      .finally(() => {
        if (active) {
          setLoading(false);
        }
      });
    void dependencies.getNotificationStatus().then(value => { if (active) setNotificationPermission(value); }).catch(() => undefined);
    if (dependencies.getAiComponents) void dependencies.getAiComponents().then(value => { if (active) setComponents(value); })
      .catch(error => { if (active) setComponentWarning(`媒体组件读取失败：${errorMessage(error)}`); });
    void (dependencies.getDevicePool?.() ?? Promise.resolve(null)).then(value => { if (active) setDevicePool(value); })
      .catch(() => { if (active) setDevicePool(null); }).finally(() => { if (active) setDevicesLoading(false); });
    return () => {
      active = false;
    };
  }, [dependencies, loadRevision, publishSettings]);

  useEffect(() => {
    if (!dependencies.subscribeAiComponentProgress) return;
    let active = true;
    let unsubscribe: (() => void) | undefined;
    void dependencies.subscribeAiComponentProgress((progress) => {
      if (!active) return;
      setComponents((current) => current.map((item) =>
        item.id === progress.id ? { ...item, stage: progress.stage, percent: progress.percent } : item
      ));
    }).then((next) => {
      if (active) unsubscribe = next;
      else next();
    }).catch(() => undefined);
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [dependencies]);

  const update = useCallback(
    async (patch: AppSettingsPatch) => {
      if (!confirmedSettings.current) return;
      if (!pendingPatches.current.length) { batchWarning.current = ""; setWarning(""); }
      pendingPatches.current.push(patch);
      publishSettings();
      const operation = updateQueue.current.then(async () => {
        try {
          const saved = await dependencies.updateSettings(patch);
          confirmedSettings.current = saved;
          if (!batchWarning.current) setWarning(saved.warning || "");
        } catch (error) {
          batchWarning.current = errorMessage(error);
          setWarning(batchWarning.current);
        } finally {
          pendingPatches.current.splice(pendingPatches.current.indexOf(patch), 1);
          publishSettings();
        }
      });
      updateQueue.current = operation;
      await operation;
    },
    [dependencies, publishSettings],
  );

  const chooseDirectory = useCallback(async () => {
    if (directoryPending.current) return;
    directoryPending.current = true;
    const operation = updateQueue.current.then(async () => { try {
      const saveDir = await dependencies.chooseSaveDir();
      if (!confirmedSettings.current) return;
      confirmedSettings.current = { ...confirmedSettings.current, saveDir };
      publishSettings();
      setWarning("");
    } catch (error) {
      setWarning(errorMessage(error));
    } finally { directoryPending.current = false; } });
    updateQueue.current = operation;
    await operation;
  }, [dependencies, publishSettings]);

  const openDirectory = useCallback(async () => {
    try {
      await dependencies.openSaveDir();
    } catch (error) {
      setWarning(errorMessage(error));
    }
  }, [dependencies]);

  const installComponent = useCallback(async (id: string) => {
    if (!dependencies.installAiComponent) return;
    setComponents((current) => current.map((item) => (
      item.id === id ? { ...item, stage: "checking", percent: 5 } : item
    )));
    try {
      const next = await dependencies.installAiComponent(id);
      setComponents((current) => {
        const index = current.findIndex((item) => item.id === id);
        if (index < 0) return [...current, next];
        const copy = current.slice();
        copy[index] = { ...next, stage: "installed", percent: 100 };
        return copy;
      });
    } catch (error) {
      setComponents((current) => current.map((item) => (
        item.id === id ? { ...item, stage: "failed" } : item
      )));
      setWarning(errorMessage(error));
      throw error;
    }
  }, [dependencies]);

  const removeComponent = useCallback(async (id: string) => {
    if (!dependencies.removeAiComponent) return;
    try {
      await dependencies.removeAiComponent(id);
      setComponents((current) => current.map((item) => item.id === id ? { ...item, installed: false, installedVersion: null, installedPath: null, stage: undefined, percent: 0, inUse: item.inUse } : item));
    } catch (error) {
      setWarning(errorMessage(error));
    }
  }, [dependencies]);

  const refreshDevice = useCallback(async () => {
    if (!dependencies.refreshDevice) return;
    setDevicesLoading(true);
    try {
      setDevicePool(await dependencies.refreshDevice());
      setWarning("");
    } catch (error) {
      setWarning(errorMessage(error));
    } finally {
      setDevicesLoading(false);
    }
  }, [dependencies]);

  return {
    settings,
    loading,
    warning: [warning, componentWarning].filter(Boolean).join("；"),
    notificationPermission,
    components,
    devicePool,
    devicesLoading,
    update,
    chooseDirectory,
    openDirectory,
    installComponent,
    removeComponent,
    refreshDevice,
    reload: () => setLoadRevision(value => value + 1),
  };
}
