import { createPathMatcher } from "./paths";
import { describe, expect, it } from "vitest";
import { normalizeFsPath, pathIsWithin, sameFsPath } from "./paths";

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

  it("maps macOS /private firmlink prefixes onto the public path", () => {
    expect(normalizeFsPath("/private/tmp/foo")).toBe("/tmp/foo");
    expect(normalizeFsPath("/private/var/folders/xx/e21.mp4")).toBe("/var/folders/xx/e21.mp4");
    expect(normalizeFsPath("/private/etc/hosts")).toBe("/etc/hosts");
    expect(normalizeFsPath("/private/Users/edking")).toBe("/private/Users/edking");
    expect(normalizeFsPath("/private/variable/log")).toBe("/private/variable/log");
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

  it("treats macOS /tmp and /private/tmp as the same location", () => {
    expect(sameFsPath("/tmp/foo", "/private/tmp/foo")).toBe(true);
    expect(sameFsPath("/var/folders/xx/e21.mp4", "/private/var/folders/xx/e21.mp4")).toBe(true);
    expect(sameFsPath("/etc/hosts", "/private/etc/hosts")).toBe(true);
  });

  it("rejects empty or unrelated paths", () => {
    expect(sameFsPath("", "C:\\Downloads\\e21.mp4")).toBe(false);
    expect(sameFsPath("C:\\Downloads\\e21.mp4", "C:\\Downloads\\e22.mp4")).toBe(false);
    expect(sameFsPath("/Downloads/e21.mp4", "/Downloads/merged.mp4")).toBe(false);
  });
});

describe("pathIsWithin", () => {
  it("requires a full path component boundary", () => {
    expect(pathIsWithin("/foo/bar", "/foo/bar/合并视频/merged.mp4")).toBe(true);
    expect(pathIsWithin("/foo/bar", "/foo/barbecue/merged.mp4")).toBe(false);
    expect(pathIsWithin("/tmp/foo", "/private/tmp/foo/合并视频/merged.mp4")).toBe(true);
  });
});

it("indexes file paths while preserving Windows aliases and POSIX case sensitivity", () => {
  const matches = createPathMatcher(["C:/Series/Episode.mp4", "/private/tmp/Episode.mp4", null, ""]);
  expect(matches("c:\\series\\episode.mp4")).toBe(true);
  expect(matches("/tmp/Episode.mp4")).toBe(true);
  expect(matches("/tmp/episode.mp4")).toBe(false);
  expect(matches(undefined)).toBe(false);
});
