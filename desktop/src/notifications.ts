import {
  isPermissionGranted,
  onAction,
  requestPermission,
  sendNotification,
  type Options,
} from "@tauri-apps/plugin-notification";

export type NotificationStatus = "granted" | "denied" | "prompt";
export type NotificationTargetKind = "monitor" | "downloadBatch" | "mediaJob" | "youtubeJob";
export type NotificationTarget = { kind: NotificationTargetKind; id: string };
export type NotificationMessage = { title: string; body: string; target: NotificationTarget };

export type NotificationRuntime = {
  isPermissionGranted(): Promise<boolean>;
  requestPermission(): Promise<NotificationPermission>;
  sendNotification(options: { title: string; body: string; extra: Record<string, unknown> }): void;
  onAction?(listener: (notification: { extra?: Record<string, unknown> }) => void): Promise<{ unregister(): Promise<void> }>;
};

export type NotificationAdapter = {
  getStatus(): Promise<NotificationStatus>;
  send(message: NotificationMessage): Promise<boolean>;
  subscribeActions?(listener: (target: NotificationTarget) => void): Promise<() => void>;
};

const KINDS = new Set<NotificationTargetKind>(["monitor", "downloadBatch", "mediaJob", "youtubeJob"]);

function validTarget(value: unknown): NotificationTarget | undefined {
  if (!value || typeof value !== "object") return undefined;
  const target = value as Partial<NotificationTarget>;
  if (!target.kind || !KINDS.has(target.kind) || typeof target.id !== "string") return undefined;
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(target.id)) return undefined;
  return { kind: target.kind, id: target.id };
}

export function createNotificationAdapter(runtime: NotificationRuntime): NotificationAdapter {
  let knownStatus: NotificationStatus | undefined;
  let permissionAttempt: Promise<boolean> | undefined;

  const getStatus = async (): Promise<NotificationStatus> => {
    if (knownStatus === "granted" || knownStatus === "denied") return knownStatus;
    try {
      if (await runtime.isPermissionGranted()) {
        knownStatus = "granted";
        return knownStatus;
      }
    } catch {
      return "prompt";
    }
    return "prompt";
  };

  const ensurePermission = async () => {
    const status = await getStatus();
    if (status === "granted") return true;
    if (status === "denied") return false;
    permissionAttempt ??= runtime.requestPermission().then((permission) => {
      knownStatus = permission === "granted" ? "granted" : "denied";
      return knownStatus === "granted";
    }).catch(() => {
      knownStatus = "denied";
      return false;
    });
    return permissionAttempt;
  };

  return {
    getStatus,
    async send(message) {
      const target = validTarget(message.target);
      if (!target || !message.title.trim() || !message.body.trim()) return false;
      if (!(await ensurePermission())) return false;
      try {
        runtime.sendNotification({
          title: message.title.slice(0, 100),
          body: message.body.slice(0, 300),
          extra: { targetKind: target.kind, targetId: target.id },
        });
        return true;
      } catch {
        return false;
      }
    },
    async subscribeActions(listener) {
      if (!runtime.onAction) return () => undefined;
      const subscription = await runtime.onAction((notification) => {
        const target = validTarget({ kind: notification.extra?.targetKind, id: notification.extra?.targetId });
        if (target) listener(target);
      });
      return () => { void subscription.unregister(); };
    },
  };
}

export function createTauriNotificationAdapter() {
  return createNotificationAdapter({
    isPermissionGranted,
    requestPermission,
    sendNotification,
    onAction: (listener) => onAction((notification: Options) => listener(notification)),
  });
}
