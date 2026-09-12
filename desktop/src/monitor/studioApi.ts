import { invoke, isTauri } from "@tauri-apps/api/core";

export type StudioConnection = { provider: string; apiKey: string; baseUrl?: string };
export type StudioResult = {
  title_candidates: { title: string; angle?: string; evidence?: string }[];
  recommended_title: string;
  recommendation_reason?: string;
  description: string;
  tags: string[];
  category_suggestion: unknown;
  cover_concepts: { headline?: string; composition?: string; prompt: string; negative_prompt?: string }[];
};
export type StudioImage = { image: string; model: string; usedReference: boolean };
export function resultText(value: unknown): string {
  if (typeof value === "string") return value;
  if (value && typeof value === "object") return Object.values(value).filter(v => typeof v === "string").join(" · ");
  return "";
}
export class StudioRequestError extends Error {
  constructor(public code: string, message: string) { super(message); this.name = "StudioRequestError"; }
}
export function isUnavailableAIKey(error: unknown): boolean {
  return !!error && typeof error === "object" && "code" in error &&
    (error.code === "AI_KEY_MISSING" || error.code === "AI_KEY_INVALID");
}
export async function studioRequest<T>(action: "models" | "text" | "image", payload: StudioConnection & Record<string, unknown>): Promise<T> {
  if (isTauri()) return invoke<T>("ai_studio_request", { action, payload });
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 260_000);
  try {
    const response = await fetch(`/api/studio/${action}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload), signal: controller.signal, cache: "no-store" });
    let body;
    try { body = await response.json(); }
    catch { throw new Error("本地 AI 服务未启动，请启动 AI 服务后重试。"); }
    if (!response.ok) throw new StudioRequestError(typeof body.code === "string" ? body.code : "AI_ERROR", typeof body.message === "string" ? body.message : "AI 请求失败，请检查配置。");
    return body as T;
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw new Error("请求超时，未自动重试；请先检查服务商任务与额度。");
    if (error instanceof TypeError) throw new Error("无法连接本地 AI 服务，请确认服务正在运行。");
    throw error;
  } finally { window.clearTimeout(timeout); }
}
export function studioError(error: unknown): string {
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") return error.message;
  return "AI 请求失败，请检查对应服务的配置。";
}
export async function fileDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => typeof reader.result === "string" ? resolve(reader.result) : reject(new Error("源封面读取失败。"));
    reader.onerror = () => reject(new Error("源封面读取失败，请重新选择。"));
    reader.readAsDataURL(file);
  });
}
