import { describe, expect, test } from "bun:test";
import { holdRemaining } from "./loading-hold";

describe("holdRemaining", () => {
  test("work that ends inside the delay never shows a loader and holds nothing", () => {
    expect(holdRemaining(0, 0)).toBe(0);
    expect(holdRemaining(0, 120)).toBe(0);
    expect(holdRemaining(0, 250)).toBe(0);
  });
  test("a loader that just appeared stays for the full minimum", () => {
    expect(holdRemaining(0, 251)).toBe(399);
    expect(holdRemaining(1000, 1300)).toBe(350);
  });
  test("a loader that already stayed long enough is released at once", () => {
    expect(holdRemaining(0, 650)).toBe(0);
    expect(holdRemaining(0, 4000)).toBe(0);
  });
});
