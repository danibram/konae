import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { callFailures, callOutcome, noticeFailures, noticeOutcome, noticeReasons, whatHappened, type Call, type Notice, type Sources } from "./what-happened";
import type { UsageSummary } from "./usage";

const day = "2026-11-08";
const at = (time: string) => Date.parse(`${day}T${time}:00Z`);

function summary(extra: Partial<UsageSummary> & { scheduled?: unknown[] } = {}): UsageSummary {
  const { scheduled = [], ...rest } = extra;
  return {
    deploymentId: "d", projectId: "p", projectName: "Desk", url: "http://desk.localhost", updatedAt: 0,
    usage: { from: day, to: day, days: [], totals: [], scheduled } as UsageSummary["usage"],
    metrics: [], actions: [],
    calls: { states: [], retainedOnly: false }, mail: { states: [], retainedOnly: false },
    coverage: { startedAt: null, earliestMetricDay: null, prunedThrough: null, retentionDays: 400, approximate: true, previewIncluded: false, queueFlushed: true, installationDroppedSinceRestart: 0, installationWriteErrorsSinceRestart: 0 },
    ...rest,
  };
}

const notice = (over: Partial<Notice>): Notice => ({ noticeId: "n1", action: "tasks.create", state: "accepted", attempts: 1, errorKind: null, createdAt: at("09:00"), updatedAt: at("09:00"), nextAttemptAt: null, ...over });
const call = (over: Partial<Call>): Call => ({ callId: "c1", action: "hooks.send", state: "delivered", attempts: 1, method: "POST", url: "https://hooks.example.org/in", status: 200, error: null, answer: null, createdAt: at("10:00"), updatedAt: at("10:00"), nextAttemptAt: null, then: null, ...over });

describe("what happened", () => {
  test("lists notices, calls, scheduled runs, routes and signals newest first", () => {
    const sources: Sources = {
      summary: summary({
        scheduled: [{ action: "days.close", at: at("11:00"), outcome: "ok", changes: 2 }],
        actions: [{ name: "hooks.receive", kind: "mutation", http: { method: "POST", path: "/hooks" }, signal: null, schedule: null, notify: null }],
        metrics: [{ day, feature: "execution", name: "hooks.receive", origin: "route", outcome: "ok", count: 3, lastAt: at("08:00") }],
      }),
      calls: [call({})],
      notices: [notice({})],
      signals: [{ name: "task.created", at: at("12:00"), properties: { title: "Milk" } }],
    };
    const entries = whatHappened(sources, at("23:00"));
    // The day's counted rows close its group, after the events.
    expect(entries.map((entry) => entry.title)).toEqual(["Signal task.created", "Scheduled run of days.close", "Call from hooks.send", "Email from tasks.create", "Route POST /hooks"]);
    expect(entries.map((entry) => entry.outcome.words)).toEqual(["Recorded.", "Ran and changed 2 rows.", "Answered 200.", "Handed to the mail provider.", "3 requests answered."]);
    expect(entries[0].detail).toBe("title Milk");
  });

  test("a notice the Kernel did not queue is a day row that says who acts", () => {
    const days = [
      { day, kind: "notify", name: "tasks.create: queued", count: 4 },
      { day, kind: "notify", name: "tasks.create: no-transport", count: 2 },
    ];
    const [entry, ...rest] = whatHappened({ summary: summary({ usage: { from: day, to: day, days, totals: [] } }), calls: [], notices: [], signals: [] }, at("23:00"));
    expect(rest).toEqual([]);
    expect(entry).toMatchObject({ daily: true, day, title: "Email from tasks.create", detail: "2 notices that day", outcome: { tone: "danger", fix: { who: "operator" } } });
    expect(entry.outcome.words).toBe("Not sent: this installation has no mail transport.");
  });

  test("a sender-not-ready day count is left out when its failed job is listed", () => {
    const days = [{ day, kind: "notify", name: "tasks.create: sender-not-ready", count: 1 }];
    const entries = whatHappened({ summary: summary({ usage: { from: day, to: day, days, totals: [] } }), calls: [], notices: [notice({ state: "failed", errorKind: "sender-not-ready" })], signals: [] });
    expect(entries).toHaveLength(1);
    expect(entries[0].daily).toBeUndefined();
  });

  test("a day count sits under its own day, below that day's events, and days run newest first", () => {
    const days = [{ day: "2026-11-07", kind: "notify", name: "tasks.create: no-account", count: 1 }, { day, kind: "notify", name: "tasks.create: no-account", count: 1 }];
    const entries = whatHappened({ summary: summary({ usage: { from: "2026-11-07", to: day, days, totals: [] } }), calls: [call({ createdAt: at("10:00") })], notices: [], signals: [] }, at("23:00"));
    expect(entries.map((entry) => `${entry.kind}:${entry.day ?? ""}`)).toEqual(["call:", `notice:${day}`, "notice:2026-11-07"]);
  });

  test("a call retrying after a known failure already says who acts", () => {
    const outcome = callOutcome(call({ state: "pending", attempts: 1, status: null, error: "dns-failed", nextAttemptAt: at("10:01") }));
    expect(outcome.words).toBe("Retrying after 1 try. The address's host name does not resolve. Next try 2026-11-08 10:01 UTC.");
    expect(outcome.fix?.who).toBe("capsule");
  });

  test("a call that needs an unset secret names the owner and Integrations, in the refusal table's words", () => {
    const outcome = callOutcome(call({ state: "failed", status: null, error: "secret-unset" }));
    expect(outcome.fix).toEqual({ who: "owner", what: "The owner sets the secret with secret_set, or in Auth > Integrations, before publishing.", where: "integrations" });
  });

  test("a call answered with an error status and its then mutation say what happened", () => {
    const outcome = callOutcome(call({ state: "failed", status: 422, attempts: 1, then: { action: "hooks.logged", state: "done", message: null } }));
    expect(outcome.words).toBe("The service answered 422. Then hooks.logged ran.");
    expect(outcome.fix?.who).toBe("capsule");
  });

  test("an unknown kind still shows its word", () => {
    expect(noticeOutcome(notice({ state: "failed", errorKind: "brand-new" })).words).toBe("Ended with brand-new. This editor has no words for it yet.");
    expect(callOutcome(call({ state: "failed", error: "brand-new" })).words).toContain("brand-new");
  });

  test("every reason and failure kind the reference names has words", () => {
    const page = (name: string) => readFileSync(new URL(`../../docs/reference/${name}.md`, import.meta.url), "utf8");
    const usage = page("usage-and-signals");
    const notices = usage.slice(usage.indexOf("### Notices"));
    const reasons = [...notices.slice(0, notices.indexOf("A queued notice")).matchAll(/^\| `([a-z-]+)`/gm)].map((match) => match[1]);
    const limits = [...notices.matchAll(/`([a-z]+-limit)`/g)].map((match) => match[1]);
    expect(reasons.length).toBeGreaterThan(5);
    for (const reason of [...reasons, ...limits]) expect(noticeReasons[reason], reason).toBeDefined();
    const accounts = page("accounts");
    const kinds = accounts.slice(accounts.indexOf("A failure kind is one of"));
    for (const [, kind] of kinds.slice(0, kinds.indexOf("\n")).matchAll(/`([a-z-]+)`/g)) {
      if (["superseded", "link-expired", "secret-unavailable"].includes(kind)) continue; // account links only
      expect(noticeFailures[kind], kind).toBeDefined();
    }
    const calls = readFileSync(new URL("../../kernel-rs/kernel/src/calls.rs", import.meta.url), "utf8");
    for (const [, kind] of calls.matchAll(/(?:Err\(\(|error: Some\()"([a-z-]+)"/g)) expect(callFailures[kind], kind).toBeDefined();
  });
});
