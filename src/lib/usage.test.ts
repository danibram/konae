import { expect, test } from "bun:test";
import { dailyActiveAccounts, dailyExecutions, dailyUsage, featureRows, formatBytes, storageRows, noticeOutcomes, outcomeLabel, pageViews, usageRange, type UsageSummary } from "./usage";

test("usage periods include today and use UTC across month and year boundaries", () => {
  expect(usageRange(7, new Date("2026-01-03T00:30:00Z"))).toEqual({ from: "2025-12-28", to: "2026-01-03" });
  expect(usageRange(30, new Date("2026-03-01T23:30:00-08:00"))).toEqual({ from: "2026-02-01", to: "2026-03-02" });
});

const summary = (totals: { kind: string; name: string; count: number }[]): UsageSummary => ({
  deploymentId: "d", projectId: "p", projectName: "Form", url: "http://form.localhost", updatedAt: 0,
  usage: { from: "2026-11-01", to: "2026-11-08", days: totals.map((t) => ({ ...t, day: "2026-11-08" })), totals },
  metrics: [], actions: [{ name: "messages.send", kind: "mutation", http: null, signal: null, schedule: null, notify: { to: "owner" } }],
  accounts: { active: 0, opens: 0, actions: 0, days: [], top: [], limit: 20, startedAt: null },
  storage: { total: 0, files: 0, parts: [] },
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

test("a daily series has one point per day of the range, zero on a day with nothing recorded", () => {
  const value = summary([{ kind: "page_view", name: "index.html", count: 3 }, { kind: "page_view", name: "about.html", count: 1 }, { kind: "action", name: "notes.add", count: 9 }]);
  value.usage.days.push({ kind: "page_view", name: "index.html", count: 2, day: "2026-11-02" });
  const series = dailyUsage(value, "page_view");
  expect(series.map((p) => p.day)).toEqual(["2026-11-01", "2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-11-07", "2026-11-08"]);
  expect(series.map((p) => p.value)).toEqual([0, 2, 0, 0, 0, 0, 0, 4]);
  expect(series.at(-1)?.parts).toEqual({ "index.html": 3, "about.html": 1 });
  expect(pageViews(value)).toEqual([{ page: "index.html", count: 3 }, { page: "about.html", count: 1 }]);
  expect(featureRows(value).find((f) => f.id === "pages")?.count).toBe(4);
});

test("daily executions keep each result as a part of the day", () => {
  const value = summary([]);
  value.metrics = [
    { day: "2026-11-03", feature: "execution", name: "a", origin: "runtime", outcome: "ok", count: 5, lastAt: 0 },
    { day: "2026-11-03", feature: "execution", name: "b", origin: "route", outcome: "failed", count: 1, lastAt: 0 },
    { day: "2026-11-03", feature: "signal", name: "x.y", origin: "kernel", outcome: "recorded", count: 7, lastAt: 0 },
  ];
  const day = dailyExecutions(value).find((p) => p.day === "2026-11-03");
  expect(day).toEqual({ day: "2026-11-03", value: 6, parts: { ok: 5, failed: 1 } });
});

test("bytes read in the binary units the Kernel's limits name", () => {
  expect(formatBytes(1)).toBe("1 byte");
  expect(formatBytes(840)).toBe("840 bytes");
  expect(formatBytes(1536)).toBe("1.5 KiB");
  expect(formatBytes(20 * 1024 * 1024)).toBe("20 MiB");
  expect(formatBytes(300 * 1024 * 1024)).toBe("300 MiB");
});

test("storage leaves backups out, ranks the parts and warns near a limit", () => {
  const rows = storageRows({ total: 0, files: 2, parts: [
    { part: "database", bytes: 100, limit: 1000, exported: true },
    { part: "files", bytes: 900, limit: 1000, exported: true },
    { part: "published", bytes: 300, limit: null, exported: true },
    { part: "backups", bytes: 5000, limit: null, exported: false },
  ] });
  expect(rows.map((row) => [row.part, row.tone])).toEqual([["files", "warning"], ["published", undefined], ["database", undefined]]);
});

test("active accounts fill every day of the range, and a day without any reads as zero", () => {
  const base = summary([]);
  expect(dailyActiveAccounts(base).every((day) => day.value === 0)).toBe(true);
  const withAccounts = { ...base, accounts: { active: 2, opens: 3, actions: 4, days: [{ day: "2026-11-03", active: 2 }], top: [], limit: 20, startedAt: null } };
  const days = dailyActiveAccounts(withAccounts);
  expect(days).toHaveLength(8);
  expect(days.find((day) => day.day === "2026-11-03")?.value).toBe(2);
});
