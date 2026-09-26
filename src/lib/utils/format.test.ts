import { describe, expect, it } from "vitest";
import { formatBytes, formatClock, formatDuration, relativeDay } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(formatBytes(2_837_072_864)).toBe("2.6 GB");
    expect(formatBytes(22_421_827_488)).toBe("21 GB");
    expect(formatBytes(643_854)).toBe("629 KB");
    expect(formatBytes(null)).toBe("—");
  });
  it("formats clocks", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(23 * 60_000 + 14_000)).toBe("23:14");
    expect(formatClock(3_723_000)).toBe("1:02:03");
  });
  it("formats durations", () => {
    expect(formatDuration(47 * 60_000)).toBe("47 min");
    expect(formatDuration(65 * 60_000)).toBe("1 h 5 min");
  });
  it("formats relative days", () => {
    const now = new Date(2026, 8, 26, 10);
    expect(relativeDay(new Date(2026, 8, 26, 8).toISOString(), now)).toBe("Today");
    expect(relativeDay(new Date(2026, 8, 25, 8).toISOString(), now)).toBe("Yesterday");
  });
});
