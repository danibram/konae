// What left or ran on a published app, in plain words: the email notices its
// mutations queued, the calls its call actions sent, its scheduled runs, its
// HTTP routes and its Signals. Each outcome says who must act and where when
// it failed. The words for a refusal's fix come from the shared refusal table
// (kernel-rs/kernel/src/refusal-codes.json) where a code exists.
import { platformMcp } from "../mcp/client";
import { hintOf, type RefusalCode } from "../kernel/refusal";
import { usageRange, type UsageSummary } from "./usage";

/** Who clears a failure. `operator` runs the installation; `service` is the other end of a call. */
export type Actor = "owner" | "capsule" | "operator" | "service" | "person" | "wait";
/** Where in the editor the owner acts. `capsule` is an edit to actions.json, through Describe or Files. */
export type Place = "integrations" | "users" | "capsule";
export type Tone = "success" | "warning" | "danger" | "neutral";
export type Fix = { who: Actor; what: string; where?: Place };
export type Outcome = { tone: Tone; words: string; fix?: Fix };

export type Kind = "notice" | "call" | "scheduled" | "route" | "signal";
export type Entry = {
  id: string;
  kind: Kind;
  /** Milliseconds. A day count sorts at its last event, or at the end of its day. */
  at: number;
  /** True for a row that counts events of one UTC day instead of naming one. */
  daily?: boolean;
  /** The UTC day a counted row covers. */
  day?: string;
  title: string;
  detail?: string;
  outcome: Outcome;
};

export type Notice = { noticeId: string; action: string | null; state: "pending" | "accepted" | "delivered" | "failed"; attempts: number; errorKind: string | null; createdAt: number; updatedAt: number; nextAttemptAt: number | null };
export type Call = {
  callId: string; action: string; state: "pending" | "delivered" | "failed"; attempts: number; method: string | null; url: string | null;
  status: number | null; error: string | null; answer: string | null; createdAt: number; updatedAt: number; nextAttemptAt: number | null;
  then: { action: string; state: string | null; message: string | null } | null;
};
export type ScheduledRun = { action: string; at: number; outcome: string; changes: number; error?: string };
export type Signal = { name: string; at: number; properties: Record<string, unknown> };
export type Sources = { summary: UsageSummary; calls: Call[]; notices: Notice[]; signals: Signal[] };

const DAY_MS = 86_400_000;

const operatorMail = "Check the mail transport. mail_status shows its state.";

/**
 * A notice the Kernel did not queue. Usage counts it under notify as
 * `<action>: <reason>` (docs/mcp.md, Notices). `queued` is not here.
 */
export const noticeReasons: Record<string, Outcome> = {
  "no-transport": { tone: "danger", words: "Not sent: this installation has no mail transport.", fix: { who: "operator", what: "Set KONAE_MAIL_TRANSPORT to resend, smtp or development." } },
  "owner-has-no-email": { tone: "danger", words: "Not sent: this Deployment has no contact email for its owner.", fix: { who: "owner", what: "Publish again from Konae while signed in as an owner with an email address, or send this notice to the actor instead.", where: "capsule" } },
  "no-account": { tone: "warning", words: "Not sent: whoever ran the action had no app account to write to.", fix: { who: "capsule", what: "A visitor, a signed request, an API key, a call's answer and a scheduled run have no account. Send this notice to the owner, or make the action need a signed-in person.", where: "capsule" } },
  "unverified-email": { tone: "warning", words: "Not sent: the person has not confirmed their email, or their account is not active.", fix: { who: "person", what: "The person answers a verification, invitation or recovery link. The owner can resend an invitation.", where: "users" } },
  "not-a-verified-account": { tone: "warning", words: "Not sent: a changed row named someone who is not a confirmed account of this app.", fix: { who: "capsule", what: "The notice column must hold app account IDs. If it does, the person answers a verification or invitation link first.", where: "users" } },
  "recipient-cap": { tone: "warning", words: "Not sent: this run already wrote to 50 accounts.", fix: { who: "capsule", what: "One run reaches at most 50 accounts, in account ID order. Narrow the rows the mutation changes, or schedule it more often.", where: "capsule" } },
  "app-limit": { tone: "warning", words: "Not sent: this app used its 200 notices for the day.", fix: { who: "wait", what: "Nothing was stored. Notices send again once the day's count falls." } },
  "creator-limit": { tone: "warning", words: "Not sent: the owner's apps used their 500 notices for the day.", fix: { who: "wait", what: "Nothing was stored. Notices send again once the day's count falls." } },
  "recipient-limit": { tone: "warning", words: "Not sent: this address got its 50 notices from this app for the day.", fix: { who: "wait", what: "Nothing was stored. Notices send again once the day's count falls." } },
  "installation-limit": { tone: "warning", words: "Not sent: the installation used its 1,000 emails for the day.", fix: { who: "operator", what: "Nothing was stored. Raise the limit, or sending resumes once the day's count falls." } },
  "sender-not-ready": { tone: "danger", words: "Not sent: the sender address was not verified when the action ran.", fix: { who: "operator", what: operatorMail } },
  "storage-error": { tone: "danger", words: "Not sent: the mail queue could not be written.", fix: { who: "operator", what: "Read the Kernel log for the cause." } },
};

/** Why a queued notice failed: the job's failure kind (docs/mcp.md, Account email). */
export const noticeFailures: Record<string, Outcome> = {
  "provider-unavailable": { tone: "danger", words: "Not sent: the mail provider did not answer after every retry.", fix: { who: "operator", what: operatorMail } },
  rejected: { tone: "danger", words: "Not sent: the mail provider refused this message.", fix: { who: "operator", what: operatorMail } },
  "sender-refused": { tone: "danger", words: "Not sent: the mail provider refused the sender address. Sending stops until it is verified.", fix: { who: "operator", what: operatorMail } },
  bounced: { tone: "danger", words: "Sent, then bounced: the recipient's mailbox refused it.", fix: { who: "person", what: "The recipient checks their address. An app account can change it through recovery." } },
  "provider-failed": { tone: "danger", words: "Not delivered: the mail provider reported a failure.", fix: { who: "operator", what: operatorMail } },
  suppressed: { tone: "danger", words: "Not sent: the provider suppresses this address after an earlier bounce or complaint.", fix: { who: "operator", what: "Remove the address from the provider's suppression list." } },
  restored: { tone: "neutral", words: "Not sent: a restore of the installation ended it." },
  "notice-expired": { tone: "danger", words: "Not sent within a day, so the Kernel dropped it.", fix: { who: "operator", what: operatorMail } },
  "sender-not-ready": noticeReasons["sender-not-ready"],
};

const actionsJson = "Change the call's url in actions.json.";

/** Why a call ended without a 2xx answer: the Kernel's error kind (docs/mcp.md, calls). */
export const callFailures: Record<string, Outcome> = {
  timeout: { tone: "danger", words: "The service did not answer in time.", fix: { who: "service", what: "The service at that address is slow or down. Check it, then trigger the action again." } },
  network: { tone: "danger", words: "The connection to the service failed.", fix: { who: "service", what: "The service at that address is down or refusing connections. Check it, then trigger the action again." } },
  "dns-failed": { tone: "danger", words: "The address's host name does not resolve.", fix: { who: "capsule", what: `The host name may be misspelled. ${actionsJson}`, where: "capsule" } },
  "address-refused": { tone: "danger", words: "The address points to a private or local network, which a call may not reach.", fix: { who: "capsule", what: `Call a public host name. ${actionsJson}`, where: "capsule" } },
  "http-outside": { tone: "danger", words: "The address uses plain http, which reaches only an app of this installation.", fix: { who: "capsule", what: `Use https. ${actionsJson}`, where: "capsule" } },
  "editor-host": { tone: "danger", words: "The address is the editor's own host.", fix: { who: "capsule", what: `Call an app or an outside service. ${actionsJson}`, where: "capsule" } },
  "app-not-found": { tone: "danger", words: "No app of this installation answers at that address.", fix: { who: "capsule", what: `The other app may be unpublished or renamed. ${actionsJson}`, where: "capsule" } },
  "app-gone": { tone: "neutral", words: "This app was removed before the call was sent." },
  "action-gone": { tone: "neutral", words: "A later publish removed the call action before it was sent." },
  "secret-unset": { tone: "danger", words: "The call signs with a secret that has no value.", fix: { who: "owner", what: hintOf("call.secret-unset" satisfies RefusalCode).remedy, where: "integrations" } },
  "secret-unavailable": { tone: "danger", words: "The Kernel could not read the call's secret.", fix: { who: "operator", what: "Check the Kernel log. The call retries." } },
  "answer-too-large": { tone: "danger", words: "The service answered with more than the Kernel reads.", fix: { who: "service", what: "Ask the service for a smaller answer." } },
  "invalid-header": { tone: "danger", words: "A header value could not be sent.", fix: { who: "capsule", what: "Check the Idempotency-Key value the action sends.", where: "capsule" } },
};

/** Words for an unknown kind, so a new one still shows instead of a blank. */
function unknown(kind: string, tone: Tone = "danger"): Outcome {
  return { tone, words: `Ended with ${kind}. This editor has no words for it yet.` };
}

/** HH:MM UTC, the clock Usage days and daily schedules use. */
export function clock(ms: number) {
  return new Date(ms).toISOString().slice(11, 16) + " UTC";
}

function when(ms: number) {
  return `${new Date(ms).toISOString().slice(0, 10)} ${clock(ms)}`;
}

export function noticeOutcome(notice: Notice): Outcome {
  if (notice.state === "delivered") return { tone: "success", words: "Delivered to the recipient's mailbox." };
  if (notice.state === "accepted") return { tone: "success", words: "Handed to the mail provider." };
  if (notice.state === "pending") {
    const next = notice.nextAttemptAt ? ` Next try ${when(notice.nextAttemptAt)}.` : "";
    return notice.attempts > 0
      ? { tone: "warning", words: `Waiting to send after ${notice.attempts} ${notice.attempts === 1 ? "try" : "tries"}.${next}` }
      : { tone: "neutral", words: `Waiting to send.${next}` };
  }
  return (notice.errorKind && noticeFailures[notice.errorKind]) || unknown(notice.errorKind ?? "no reason");
}

export function callOutcome(call: Call): Outcome {
  const then = call.then ? thenWords(call.then) : "";
  if (call.state === "pending") {
    const next = call.nextAttemptAt ? ` Next try ${when(call.nextAttemptAt)}.` : "";
    if (call.attempts === 0) return { tone: "neutral", words: `Waiting to send.${next}` };
    const reason = call.error ? (callFailures[call.error]?.words ?? call.error) : call.status ? `The service answered ${call.status}.` : "";
    const fix = call.error ? callFailures[call.error]?.fix : undefined;
    return { tone: "warning", words: `Retrying after ${call.attempts} ${call.attempts === 1 ? "try" : "tries"}. ${reason}${next}`.replace(/\s+/g, " ").trim(), ...(fix ? { fix } : {}) };
  }
  if (call.state === "delivered") return { tone: "success", words: `Answered ${call.status}.${then}` };
  if (call.error) {
    const known = callFailures[call.error] ?? unknown(call.error);
    return { ...known, words: known.words + then };
  }
  const status = call.status ?? 0;
  const fix: Fix = status === 401 || status === 403
    ? { who: "owner", what: "The service refused the credentials. Check the secret the call signs or sends with.", where: "integrations" }
    : status >= 500 || status === 429 || status === 408
      ? { who: "service", what: "The service failed or asked to slow down on every try. Check it, then trigger the action again." }
      : { who: "capsule", what: "The service refused what the call sent. Check the url and the body in actions.json against what the service expects.", where: "capsule" };
  return { tone: "danger", words: `The service answered ${status || "nothing"}${call.attempts > 1 ? ` after ${call.attempts} tries` : ""}.${then}`, fix };
}

function thenWords(then: NonNullable<Call["then"]>) {
  if (then.state === "done") return ` Then ${then.action} ran.`;
  if (then.state === "refused") return ` Then ${then.action} was refused${then.message ? `: ${then.message}` : "."}`;
  if (then.state === "failed") return ` Then ${then.action} failed${then.message ? `: ${then.message}` : "."}`;
  return ` Then ${then.action} has not run yet.`;
}

export function scheduledOutcome(run: ScheduledRun): Outcome {
  if (run.outcome === "ok") return run.changes > 0
    ? { tone: "success", words: `Ran and changed ${run.changes} ${run.changes === 1 ? "row" : "rows"}.` }
    : { tone: "neutral", words: "Ran and changed nothing." };
  return {
    tone: "danger",
    words: `Failed${run.error ? `: ${run.error}` : "."} The next run tries again.`,
    fix: { who: "capsule", what: "The statement failed on the published database. Fix it in actions.json, or the data it expects.", where: "capsule" },
  };
}

function routeOutcome(outcome: string, count: number): Outcome {
  const times = count === 1 ? "1 request" : `${count} requests`;
  if (outcome === "ok") return { tone: "success", words: `${times} answered.` };
  if (outcome === "unchanged") return { tone: "neutral", words: `${times} answered and changed nothing.` };
  if (outcome === "refused") return { tone: "warning", words: `${times} refused by the action's guard.`, fix: { who: "service", what: "The caller sent a request the guard does not accept. Nothing to fix here unless the guard is wrong." } };
  if (outcome === "failed") return { tone: "danger", words: `${times} failed.`, fix: { who: "capsule", what: "The route's statement failed. Fix it in actions.json.", where: "capsule" } };
  return unknown(outcome, "neutral");
}

function endOfDay(day: string, now: number) {
  return Math.min(Date.parse(`${day}T00:00:00Z`) + DAY_MS - 1, now);
}

/** Every source in one list, newest first. */
export function whatHappened({ summary, calls, notices, signals }: Sources, now = Date.now()): Entry[] {
  const entries: Entry[] = [];
  for (const notice of notices) {
    entries.push({ id: `notice:${notice.noticeId}`, kind: "notice", at: notice.createdAt, title: `Email from ${notice.action ?? "an action"}`, outcome: noticeOutcome(notice) });
  }
  // A notice the Kernel stored as failed with sender-not-ready is a job above too, so its day count is left out.
  const jobsCover = new Set(notices.filter((n) => n.errorKind === "sender-not-ready").map((n) => `${n.action}:${new Date(n.createdAt).toISOString().slice(0, 10)}`));
  for (const row of summary.usage.days) {
    if (row.kind !== "notify") continue;
    const split = row.name.lastIndexOf(": ");
    const [action, reason] = split < 0 ? [row.name, ""] : [row.name.slice(0, split), row.name.slice(split + 2)];
    if (reason === "queued") continue;
    if (reason === "sender-not-ready" && jobsCover.has(`${action}:${row.day}`)) continue;
    const outcome = noticeReasons[reason] ?? unknown(reason || "no reason");
    entries.push({ id: `notify:${row.day}:${row.name}`, kind: "notice", daily: true, day: row.day, at: endOfDay(row.day, now), title: `Email from ${action}`, detail: `${row.count} ${row.count === 1 ? "notice" : "notices"} that day`, outcome });
  }
  for (const call of calls) {
    entries.push({ id: `call:${call.callId}`, kind: "call", at: call.createdAt, title: `Call from ${call.action}`, detail: [call.method, call.url].filter(Boolean).join(" ") || undefined, outcome: callOutcome(call) });
  }
  // usage_summary carries usage_read's answer, scheduled runs included.
  const scheduled = (summary.usage as { scheduled?: ScheduledRun[] }).scheduled ?? [];
  for (const [index, run] of scheduled.entries()) {
    entries.push({ id: `scheduled:${run.at}:${run.action}:${index}`, kind: "scheduled", at: run.at, title: `Scheduled run of ${run.action}`, outcome: scheduledOutcome(run) });
  }
  const routes = new Map(summary.actions.filter((action) => action.http).map((action) => [action.name, action.http!]));
  for (const metric of summary.metrics) {
    if (metric.feature !== "execution" || metric.origin !== "route") continue;
    const http = routes.get(metric.name);
    entries.push({
      id: `route:${metric.day}:${metric.name}:${metric.outcome}`, kind: "route", daily: true, day: metric.day, at: metric.lastAt || endOfDay(metric.day, now),
      title: `Route ${http ? `${http.method} ${http.path}` : metric.name}`, detail: `${metric.name}, that day`, outcome: routeOutcome(metric.outcome, metric.count),
    });
  }
  signals.forEach((signal, index) => {
    const props = Object.entries(signal.properties).map(([key, value]) => `${key} ${typeof value === "string" ? value : JSON.stringify(value)}`).join(", ");
    entries.push({ id: `signal:${signal.at}:${signal.name}:${index}`, kind: "signal", at: signal.at, title: `Signal ${signal.name}`, detail: props || undefined, outcome: { tone: "neutral", words: "Recorded." } });
  });
  return entries.sort((a, b) => dayOf(b).localeCompare(dayOf(a)) || Number(a.daily ?? false) - Number(b.daily ?? false) || b.at - a.at);
}

/** The UTC day an entry belongs to, YYYY-MM-DD. A day count belongs to its day. */
export function dayOf(entry: Entry) {
  return entry.day ?? new Date(entry.at).toISOString().slice(0, 10);
}

/** True when someone must act on the entry: it has a fix and the fix is not waiting. */
export function needsAction(entry: Entry) {
  return Boolean(entry.outcome.fix && entry.outcome.fix.who !== "wait");
}

/** The words for who acts, as the view shows them. */
export const actorWords: Record<Actor, string> = {
  owner: "You need to act.",
  capsule: "The app needs a change.",
  operator: "The person who runs this installation needs to act.",
  service: "The other service needs to act.",
  person: "The person with the app account needs to act.",
  wait: "Nobody needs to act. It waits.",
};

export type Read = { sources: Sources; days: number };

/** The newest page of each source for the last `days` days. */
export async function readWhatHappened(deploymentId: string, days: number, signal?: AbortSignal): Promise<Read> {
  const [summary, calls, notices, signals] = await Promise.all([
    platformMcp.call<UsageSummary>("usage_summary", { deploymentId, ...usageRange(days) }, signal),
    platformMcp.call<{ calls: Call[] }>("calls_read", { deploymentId }, signal),
    platformMcp.call<{ notices: Notice[] }>("notices_read", { deploymentId }, signal),
    platformMcp.call<{ signals: Signal[] }>("signals_read", { deploymentId }, signal),
  ]);
  const from = Date.parse(`${usageRange(days).from}T00:00:00Z`);
  return {
    days,
    sources: {
      summary,
      calls: calls.calls.filter((call) => call.createdAt >= from),
      notices: notices.notices.filter((notice) => notice.createdAt >= from),
      signals: signals.signals.filter((item) => item.at >= from),
    },
  };
}
