import { describe, expect, it } from "vitest";
import { normalizeFsPath, sameFsPath } from "./paths";

describe("normalizeFsPath", () => {
  it("strips the Windows extended prefix and unifies separators", () => {
    expect(normalizeFsPath("\\\\?\\C:\\Downloads\\e21.mp4")).toBe("C:/Downloads/e21.mp4");
    expect(normalizeFsPath("C:\\Downloads\\e21.mp4")).toBe("C:/Downloads/e21.mp4");
    expect(normalizeFsPath("\\\\?\\UNC\\server\\share\\e21.mp4")).toBe("//server/share/e21.mp4");
    expect(normalizeFsPath("\\\\server\\share\\e21.mp4")).toBe("//server/share/e21.mp4");
  });

  it("uppercases Windows drive letters", () => {
    expect(normalizeFsPath("c:\\Downloads\\e21.mp4")).toBe("C:/Downloads/e21.mp4");
  });
});

describe("sameFsPath", () => {
  it("treats Windows extended paths as equal to the same drive path", () => {
    expect(sameFsPath("\\\\?\\C:\\Downloads\\e21.mp4", "C:\\Downloads\\e21.mp4")).toBe(true);
    expect(sameFsPath("\\\\?\\C:\\Downloads\\merged.mp4", "C:/Downloads/merged.mp4")).toBe(true);
  });

  it("compares Windows and UNC paths case-insensitively", () => {
    expect(sameFsPath("C:\\Downloads\\E21.mp4", "c:\\downloads\\e21.mp4")).toBe(true);
    expect(sameFsPath("\\\\Server\\Share\\E21.mp4", "\\\\server\\share\\e21.mp4")).toBe(true);
  });

  it("keeps POSIX paths case-sensitive", () => {
    expect(sameFsPath("/Downloads/e21.mp4", "/Downloads/e21.mp4")).toBe(true);
    expect(sameFsPath("/Downloads/e21.mp4", "/downloads/e21.mp4")).toBe(false);
  });

  it("rejects empty or unrelated paths", () => {
    expect(sameFsPath("", "C:\\Downloads\\e21.mp4")).toBe(false);
    expect(sameFsPath("C:\\Downloads\\e21.mp4", "C:\\Downloads\\e22.mp4")).toBe(false);
    expect(sameFsPath("/Downloads/e21.mp4", "/Downloads/merged.mp4")).toBe(false);
  });
});
