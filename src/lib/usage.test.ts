import { expect, test } from "bun:test";
import { featureRows, noticeOutcomes, outcomeLabel, usageRange, type UsageSummary } from "./usage";

test("usage periods include today and use UTC across month and year boundaries", () => {
  expect(usageRange(7, new Date("2026-01-03T00:30:00Z"))).toEqual({ from: "2025-12-28", to: "2026-01-03" });
  expect(usageRange(30, new Date("2026-03-01T23:30:00-08:00"))).toEqual({ from: "2026-02-01", to: "2026-03-02" });
});

const summary = (totals: { kind: string; name: string; count: number }[]): UsageSummary => ({
  deploymentId: "d", projectId: "p", projectName: "Form", sourceAuthority: "managed", url: "http://form.localhost", updatedAt: 0,
  usage: { from: "2026-11-01", to: "2026-11-08", days: totals.map((t) => ({ ...t, day: "2026-11-08" })), totals },
  metrics: [], actions: [{ name: "messages.send", kind: "mutation", http: null, signal: null, schedule: null, notify: { to: "owner" } }],
  calls: { states: [], retainedOnly: true }, mail: { states: [], retainedOnly: true },
  coverage: { startedAt: null, earliestMetricDay: null, prunedThrough: null, retentionDays: 400, approximate: true, previewIncluded: false, queueFlushed: true, installationDroppedSinceRestart: 0, installationWriteErrorsSinceRestart: 0 },
});

test("a notice the Kernel could not queue counts as not sent, with its reason in words", () => {
  const notices = noticeOutcomes(summary([
    { kind: "notify", name: "messages.send: queued", count: 2 },
    { kind: "notify", name: "messages.send: no-transport", count: 3 },
    { kind: "notify", name: "messages.send: some-new-kind", count: 1 },
  ]));
  expect(notices.queued).toBe(2);
  expect(notices.notSent).toBe(4);
  expect(notices.reasons).toEqual([
    { action: "messages.send", reason: "no-transport", label: "this installation has no email service", count: 3 },
    { action: "messages.send", reason: "some-new-kind", label: "some-new-kind", count: 1 },
  ]);
  const row = featureRows(summary([{ kind: "notify", name: "messages.send: no-transport", count: 3 }])).find((f) => f.id === "notify");
  expect(row?.count).toBe(3);
  expect(row?.notSent).toBe(3);
});

test("execution outcomes have one label in every table", () => {
  expect(outcomeLabel("ok")).toBe("Correct");
  expect(outcomeLabel("refused")).toBe("Refused by app rules");
  expect(outcomeLabel("failed")).toBe("Failed");
  expect(outcomeLabel("unchanged")).toBe("No changes");
  expect(outcomeLabel("other")).toBe("other");
});

test("the overview's compacted notices keep their reason, and a nameless notice row claims nothing", () => {
  const notices = noticeOutcomes(summary([
    { kind: "notify", name: "*: no-transport", count: 3 },
    { kind: "notify", name: "", count: 5 },
  ]));
  expect(notices).toEqual({ queued: 0, notSent: 3, reasons: [{ action: "All actions", reason: "no-transport", label: "this installation has no email service", count: 3 }] });
});
