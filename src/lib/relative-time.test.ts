import { describe, expect, test } from "bun:test";
import { relativeTime } from "./relative-time";

const now = Date.UTC(2026, 10, 14, 12, 0, 0);
const minute = 60_000;

describe("relativeTime", () => {
  test("the last minute, and a clock slightly ahead, read as just now", () => {
    expect(relativeTime(now, now)).toBe("just now");
    expect(relativeTime(now - 59_000, now)).toBe("just now");
    expect(relativeTime(now + 5_000, now)).toBe("just now");
  });
  test("minutes and hours count down to the unit", () => {
    expect(relativeTime(now - 3 * minute, now)).toBe("3 min ago");
    expect(relativeTime(now - 59 * minute, now)).toBe("59 min ago");
    expect(relativeTime(now - 60 * minute, now)).toBe("1 h ago");
    expect(relativeTime(now - 23 * 60 * minute, now)).toBe("23 h ago");
  });
  test("days, then a date once a week has passed", () => {
    expect(relativeTime(now - 25 * 60 * minute, now)).toBe("yesterday");
    expect(relativeTime(now - 3 * 24 * 60 * minute, now)).toBe("3 days ago");
    expect(relativeTime(Date.UTC(2026, 9, 2, 12), now)).toBe("Oct 2");
    expect(relativeTime(Date.UTC(2025, 9, 2, 12), now)).toBe("Oct 2, 2025");
  });
});
