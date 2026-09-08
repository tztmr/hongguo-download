import type { AIComponentStatus } from "../types";

export function isRuntimeComponentId(id: string): boolean {
  return id === "runtime" || id.startsWith("runtime-");
}

export function requiredRuntimeComponentId(components: readonly AIComponentStatus[]): string {
  if (components.some((item) => item.id === "runtime")) return "runtime";
  if (components.some((item) => item.id === "runtime-modern")) return "runtime-modern";
  if (components.some((item) => item.id === "runtime-cpu")) return "runtime-cpu";
  return components.find((item) => isRuntimeComponentId(item.id))?.id ?? "runtime";
}

export function missingAiComponentIds(
  components: readonly AIComponentStatus[] | undefined,
  modelId: string,
): string[] {
  if (components === undefined) return [];
  const missing: string[] = [];
  if (!components.some((item) => isRuntimeComponentId(item.id) && item.installed)) {
    missing.push(requiredRuntimeComponentId(components));
  }
  if (!components.some((item) => item.id === modelId && item.installed)) {
    missing.push(modelId);
  }
  return missing;
}
