import { describe, expect, it } from "vitest";
import { formatClock, formatDuration, formatSize, splitLinks, spokenCode, spokenSafetyCode } from "./format";

describe("formatting", () => {
  it("writes sizes in each language", () => {
    expect(formatSize(0, "en")).toBe("0 B");
    expect(formatSize(1_500_000, "en")).toBe("1.5 MB");
    expect(formatSize(12_400_000_000, "en")).toBe("12.4 GB");
    expect(formatSize(12_400_000_000, "fr")).toBe("12,4 Go");
    expect(formatSize(999, "fr")).toBe("999 o");
  });

  it("writes durations and countdowns", () => {
    expect(formatDuration(45, "en")).toBe("45 s");
    expect(formatDuration(200, "en")).toBe("3 min 20 s");
    expect(formatDuration(7500, "fr")).toBe("2 h 05 min");
    expect(formatClock(600)).toBe("10:00");
    expect(formatClock(59.2)).toBe("1:00");
    expect(formatClock(-3)).toBe("0:00");
  });

  it("spells codes for screen readers", () => {
    expect(spokenCode("42-orbit-velvet-candle")).toBe("42, orbit, velvet, candle");
    expect(spokenSafetyCode("418 293")).toBe("41, 82, 93");
  });

  it("finds links without touching the rest", () => {
    expect(splitLinks("see https://example.com/a?b=1 now")).toEqual([
      { text: "see ", link: false },
      { text: "https://example.com/a?b=1", link: true },
      { text: " now", link: false },
    ]);
    expect(splitLinks("no links")).toEqual([{ text: "no links", link: false }]);
    expect(splitLinks("javascript:alert(1)")).toEqual([{ text: "javascript:alert(1)", link: false }]);
  });
});
