import { useCallback, useEffect, useRef, useState } from "react";
import {
  chooseSaveDir,
  getAiComponents,
  getSettings,
  installAiComponent,
  openSaveDir,
  removeAiComponent,
  subscribeAiComponentProgress,
  updateSettings,
} from "../api";
import { createTauriNotificationAdapter, type NotificationStatus } from "../notifications";
import type { AIComponentProgress, AIComponentStatus, AppSettings } from "../types";

export type AppSettingsPatch = Partial<
  Omit<AppSettings, "version" | "warning">
>;

export type AppSettingsDependencies = {
  getSettings(): Promise<AppSettings>;
  updateSettings(patch: AppSettingsPatch): Promise<AppSettings>;
  chooseSaveDir(): Promise<string>;
  openSaveDir(): Promise<void>;
  getNotificationStatus(): Promise<NotificationStatus>;
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
  update(patch: AppSettingsPatch): Promise<void>;
  chooseDirectory(): Promise<void>;
  openDirectory(): Promise<void>;
  installComponent(id: string): Promise<void>;
  removeComponent(id: string): Promise<void>;
};

const notifications = createTauriNotificationAdapter();
const defaultDependencies: AppSettingsDependencies = {
  getSettings,
  updateSettings,
  chooseSaveDir,
  openSaveDir,
  getNotificationStatus: () => notifications.getStatus(),
  getAiComponents,
  installAiComponent,
  removeAiComponent,
  subscribeAiComponentProgress,
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function useAppSettings(
  dependencies: AppSettingsDependencies = defaultDependencies,
): UseAppSettingsResult {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [warning, setWarning] = useState("");
  const [notificationPermission, setNotificationPermission] =
    useState<NotificationStatus>("prompt");
  const [components, setComponents] = useState<AIComponentStatus[]>([]);
  const settingsRef = useRef<AppSettings | null>(null);
  const updateQueue = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    let active = true;
    void Promise.all([
      dependencies.getSettings(),
      dependencies.getNotificationStatus().catch(() => "prompt" as const),
      dependencies.getAiComponents ? dependencies.getAiComponents().catch(() => [] as AIComponentStatus[]) : Promise.resolve([] as AIComponentStatus[]),
    ])
      .then(([value, permission, nextComponents]) => {
        if (!active) return;
        settingsRef.current = value;
        setSettings(value);
        setWarning(value.warning || "");
        setNotificationPermission(permission);
        setComponents(nextComponents);
      })
      .catch((error) => {
        if (active) setWarning(errorMessage(error));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [dependencies]);

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
      const previous = settingsRef.current;
      if (!previous) return;
      const optimistic = { ...previous, ...patch };
      settingsRef.current = optimistic;
      setSettings(optimistic);
      setWarning("");
      const operation = updateQueue.current.then(async () => {
        try {
          const saved = await dependencies.updateSettings(patch);
          settingsRef.current = saved;
          setSettings(saved);
          setWarning(saved.warning || "");
        } catch (error) {
          settingsRef.current = previous;
          setSettings(previous);
          setWarning(errorMessage(error));
        }
      });
      updateQueue.current = operation;
      await operation;
    },
    [dependencies],
  );

  const chooseDirectory = useCallback(async () => {
    try {
      const saveDir = await dependencies.chooseSaveDir();
      if (!settingsRef.current) return;
      const next = { ...settingsRef.current, saveDir };
      settingsRef.current = next;
      setSettings(next);
      setWarning("");
    } catch (error) {
      setWarning(errorMessage(error));
    }
  }, [dependencies]);

  const openDirectory = useCallback(async () => {
    try {
      await dependencies.openSaveDir();
    } catch (error) {
      setWarning(errorMessage(error));
    }
  }, [dependencies]);

  const installComponent = useCallback(async (id: string) => {
    if (!dependencies.installAiComponent) return;
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

  return {
    settings,
    loading,
    warning,
    notificationPermission,
    components,
    update,
    chooseDirectory,
    openDirectory,
    installComponent,
    removeComponent,
  };
}
