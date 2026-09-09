import { useState } from "react";
import type { YouTubePrivacy } from "./types";

const KEY = "hongguo.youtube.upload-preferences.v1";
export type UploadPreferences = {
  privacy: YouTubePrivacy;
  categoryId: string;
  madeForKids: boolean;
  synthetic: boolean;
  paidPromotion: boolean;
  subtitleLanguage: string;
};
const defaults: UploadPreferences = { privacy: "private", categoryId: "1", madeForKids: false, synthetic: true, paidPromotion: false, subtitleLanguage: "zh-Hans" };
export function readUploadPreferences(): UploadPreferences {
  try {
    const value = JSON.parse(window.localStorage.getItem(KEY) || "{}");
    return {
      privacy: ["private", "unlisted", "public"].includes(value?.privacy) ? value.privacy : defaults.privacy,
      categoryId: ["1", "24"].includes(value?.categoryId) ? value.categoryId : defaults.categoryId,
      madeForKids: typeof value?.madeForKids === "boolean" ? value.madeForKids : defaults.madeForKids,
      synthetic: typeof value?.synthetic === "boolean" ? value.synthetic : defaults.synthetic,
      paidPromotion: typeof value?.paidPromotion === "boolean" ? value.paidPromotion : defaults.paidPromotion,
      subtitleLanguage: typeof value?.subtitleLanguage === "string" && /^[a-z]{2,3}(?:-[a-zA-Z0-9]{2,8})*$/.test(value.subtitleLanguage) ? value.subtitleLanguage : defaults.subtitleLanguage,
    };
  } catch { return { ...defaults }; }
}
export function useUploadPreferences() {
  const [settings, setSettings] = useState(readUploadPreferences);
  function setSetting<K extends keyof UploadPreferences>(key: K, value: UploadPreferences[K]) {
    setSettings((previous) => ({ ...previous, [key]: value }));
    // Only the allow-listed settings are ever persisted, never per-video content or file paths.
    try { window.localStorage.setItem(KEY, JSON.stringify({ ...readUploadPreferences(), [key]: value })); } catch { /* Storage may be disabled; the current dialog still works. */ }
  }
  return { settings, setSetting };
}
