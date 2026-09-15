import { describe, expect, it } from "vitest";
import type { AIComponentStatus } from "../types";
import { missingAiComponentIds, requiredRuntimeComponentId } from "./aiRuntime";

function component(id: string, installed = false): AIComponentStatus {
  return {
    id,
    version: "1",
    installed,
    installedVersion: installed ? "1" : null,
    installedPath: null,
    downloadBytes: 1024,
    installedBytes: 2048,
    inUse: false,
  };
}

const macosCatalog = [component("runtime"), component("demucs-htdemucs"), component("whisper-small")];
const windowsCatalog = [
  component("runtime-modern"),
  component("runtime-legacy"),
  component("runtime-cpu"),
  component("demucs-htdemucs"),
  component("whisper-small"),
];

describe("requiredRuntimeComponentId", () => {
  it("keeps the macOS runtime id when the catalog still uses it", () => {
    expect(requiredRuntimeComponentId(macosCatalog)).toBe("runtime");
  });

  it("asks Windows catalogs for runtime-modern instead of runtime", () => {
    expect(requiredRuntimeComponentId(windowsCatalog)).toBe("runtime-modern");
  });

  it("falls back to runtime-cpu when that is the only runtime in the catalog", () => {
    expect(requiredRuntimeComponentId([component("runtime-cpu"), component("demucs-htdemucs")])).toBe("runtime-cpu");
  });
});

describe("missingAiComponentIds", () => {
  it("uses CPU installation for CPU requests and accepts CPU-only installations in auto mode", () => {
    expect(missingAiComponentIds(windowsCatalog, "demucs-htdemucs", "cpu")).toEqual(["runtime-cpu", "demucs-htdemucs"]);
    const installed = windowsCatalog.map(item => ({ ...item, installed: ["runtime-cpu", "demucs-htdemucs"].includes(item.id) }));
    expect(missingAiComponentIds(installed, "demucs-htdemucs", "auto")).toEqual([]);
    expect(requiredRuntimeComponentId(installed, "auto")).toBe("runtime-cpu");
    expect(missingAiComponentIds(installed, "demucs-htdemucs", "cuda")).toEqual(["runtime-modern"]);
  });

  it("reuses an installed CUDA runtime for CPU jobs without another large download", () => {
    const installed = windowsCatalog.map(item => ({ ...item, installed: ["runtime-legacy", "demucs-htdemucs"].includes(item.id) }));
    expect(missingAiComponentIds(installed, "demucs-htdemucs", "cpu")).toEqual([]);
  });
  it("skips the install gate when the component list is still loading", () => {
    expect(missingAiComponentIds(undefined, "demucs-htdemucs")).toEqual([]);
  });

  it("treats any installed runtime or runtime-* as enough", () => {
    expect(missingAiComponentIds([
      component("runtime-modern", true),
      component("demucs-htdemucs", true),
    ], "demucs-htdemucs")).toEqual([]);
    expect(missingAiComponentIds([
      component("runtime-legacy", true),
      component("runtime-modern"),
      component("demucs-htdemucs", true),
    ], "demucs-htdemucs")).toEqual([]);
    expect(missingAiComponentIds([
      component("runtime", true),
      component("demucs-htdemucs", true),
    ], "demucs-htdemucs")).toEqual([]);
  });

  it("asks a missing Windows catalog for runtime-modern rather than runtime", () => {
    expect(missingAiComponentIds(windowsCatalog, "demucs-htdemucs")).toEqual(["runtime-modern", "demucs-htdemucs"]);
  });

  it("still reports a missing model when a Windows runtime is already installed", () => {
    expect(missingAiComponentIds([
      component("runtime-cpu", true),
      ...windowsCatalog,
    ], "whisper-small")).toEqual(["whisper-small"]);
  });
});
