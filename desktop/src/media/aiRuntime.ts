import type { AIComponentStatus, AIDevicePreference } from "../types";

export function isRuntimeComponentId(id: string): boolean {
  return id === "runtime" || id.startsWith("runtime-");
}

export function runtimeComponentCandidates(components: readonly AIComponentStatus[], device: AIDevicePreference = "auto"): string[] {
  if (components.some(item => item.id === "runtime")) return ["runtime"];
  if (device === "cpu") return ["runtime-cpu", "runtime-modern", "runtime-legacy"];
  if (device === "cuda") return ["runtime-modern", "runtime-legacy"];
  return ["runtime-modern", "runtime-legacy", "runtime-cpu"];
}

export function requiredRuntimeComponentId(components: readonly AIComponentStatus[], device: AIDevicePreference = "auto"): string {
  const candidates = runtimeComponentCandidates(components, device);
  return candidates.find(id => components.some(item => item.id === id && item.installed))
    ?? candidates.find(id => components.some(item => item.id === id)) ?? candidates[0];
}

export function missingAiComponentIds(
  components: readonly AIComponentStatus[] | undefined,
  modelId: string,
  device: AIDevicePreference = "auto",
): string[] {
  if (components === undefined) return [];
  const missing: string[] = [];
  const runtimes = runtimeComponentCandidates(components, device);
  if (!components.some((item) => runtimes.includes(item.id) && item.installed)) {
    missing.push(requiredRuntimeComponentId(components, device));
  }
  if (!components.some((item) => item.id === modelId && item.installed)) {
    missing.push(modelId);
  }
  return missing;
}
