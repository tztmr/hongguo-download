import { describe, expect, it } from "vitest";
import { notificationVideoIds } from "./managementVideoInput";

describe("notification video links", () => {
  it("reads watch, Shorts, Studio notification links and plain IDs without duplicates", () => {
    expect(notificationVideoIds("视频封锁：https://www.youtube.com/watch?v=long0000001&feature=share。\nhttps://youtube.com/shorts/short000001?si=demo\nhttps://studio.youtube.com/video/short000001/copyright\nhttps://youtu.be/long0000001\nother000001"))
      .toEqual(["long0000001", "short000001", "other000001"]);
  });
  it("rejects foreign hosts, invalid IDs and embedded query injections", () => {
    expect(notificationVideoIds("https://youtube.com.evil.test/watch?v=long0000001\nhttps://evil.test/shorts/short000001\nhttps://www.youtube.com/watch?v=short000001%2Cother000001\ninvalid\nhttps://youtube.com/channel/short000001"))
      .toEqual([]);
  });
});
