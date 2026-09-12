import { invoke } from "@tauri-apps/api/core";

export type AutomationSecrets = Partial<Record<"text" | "image", string>>;
export type AutomationJob = {
  id: string; title: string; bookId: string; season?: number; stage: string;
  status: "pending" | "working" | "review" | "failed" | "completed" | "skipped" | "observing";
  message: string; episodeDone: number; episodeTotal: number; progress: number;
  mainVideoUrl?: string; shortVideoUrl?: string; updatedAt: number;
};
export type AutomationSnapshot = {
  config: Record<string, unknown> | null;
  mode: "stopped" | "running" | "paused";
  jobs: AutomationJob[];
  logs: { at: number; jobId?: string; message: string }[];
  lastScan: number; nextScan: number; warning: string;
  keyStatus: { text: boolean; image: boolean };
};
export const automationRuntime = {
  snapshot: () => invoke<AutomationSnapshot>("get_automation_snapshot"),
  save: (config: Record<string, unknown>, secrets: AutomationSecrets = {}) => invoke<AutomationSnapshot>("save_automation_settings", {
    config,
    ...(Object.keys(secrets).length ? { secrets } : {}),
  }),
  start: () => invoke<AutomationSnapshot>("start_automation"),
  control: (action: "pause" | "resume" | "stop" | "scan") => invoke<AutomationSnapshot>("control_automation", { action }),
  review: (jobId: string, action: "continue" | "skip" | "retry") => invoke<AutomationSnapshot>("review_automation_job", { jobId, action }),
};
export function automationError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error) return String(error.message);
  return "后台请求失败，请重试。";
}
