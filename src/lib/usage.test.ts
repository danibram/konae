import { expect, test } from "bun:test";
import { usageRange } from "./usage";

test("usage periods include today and use UTC across month and year boundaries", () => {
  expect(usageRange(7, new Date("2026-01-03T00:30:00Z"))).toEqual({ from: "2025-12-28", to: "2026-01-03" });
  expect(usageRange(30, new Date("2026-03-01T23:30:00-08:00"))).toEqual({ from: "2026-02-01", to: "2026-03-02" });
});
