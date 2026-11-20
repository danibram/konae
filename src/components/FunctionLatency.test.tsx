import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { FunctionLatencySummary } from "../lib/usage";
import { FunctionLatency } from "./FunctionLatency";

const percentiles = { p50Ms: 30, p95Ms: 80, p99Ms: 105, maxMs: 110 };
function summary(): FunctionLatencySummary {
  return {
    maxSamples: 10_000, seriesLimit: 128, seriesTruncated: false,
    retainedOnly: true, retainedFrom: 1, prunedThrough: null,
    series: [{
      name: "orders", version: "build-a", limitMs: 100, samples: 100,
      coldStarts: 3, timeouts: 2, failures: 2, uncertain: 1, replays: 4, lastAt: 2,
      budgetP95Percent: 80, total: percentiles, queue: percentiles,
      initialization: percentiles, execution: percentiles,
    }],
  };
}

describe("function latency in Usage", () => {
  test("shows percentiles against the actual limit and distinguishes uncertain writes", () => {
    const html = renderToStaticMarkup(<FunctionLatency summary={summary()} />);
    expect(html).toContain("80%");
    expect(html).toContain("80 ms of 100 ms");
    expect(html).toContain("p99 105 ms");
    expect(html).toContain("2 timeouts");
    expect(html).toContain("1 write with an unconfirmed result");
    expect(html).toContain("4 retries answered from a saved result");
    expect(html).toContain("Stage percentiles do not add up");
    expect(html).toContain('aria-label="orders p95 time limit used"');
  });

  test("keeps an over-limit percentage visible while the progress bar stays bounded", () => {
    const value = summary();
    value.series = value.series.map((row) => ({ ...row, budgetP95Percent: 125 }));
    value.prunedThrough = 1;
    const html = renderToStaticMarkup(<FunctionLatency summary={value} />);
    expect(html).toContain("125%");
    expect(html).toContain('value="100"');
    expect(html).toContain("Older latency samples have been removed");
  });

  test("does not show a zero-latency claim without measurements", () => {
    expect(renderToStaticMarkup(<FunctionLatency summary={undefined} />)).toBe("");
    expect(renderToStaticMarkup(<FunctionLatency summary={{ ...summary(), series: [] }} />)).toBe("");
  });
});
