import { platformMcp } from "../mcp/client";

export type UsageCount = { kind: string; name: string; count: number };
export type DailyCount = UsageCount & { day: string };
export type Metric = { day: string; feature: "execution" | "signal"; name: string; origin: string; outcome: string; count: number; lastAt: number };
export type PublishedAction = { name: string; kind: string; http: { method: string; path: string } | null; signal: { name: string } | null; schedule: unknown; notify: unknown };
export type UsageSummary = {
  deploymentId: string; projectId: string; projectName: string; sourceAuthority: "managed" | "external"; url: string; updatedAt: number;
  usage: { from: string; to: string; days: DailyCount[]; totals: UsageCount[] };
  metrics: Metric[]; actions: PublishedAction[];
  calls: { states: { state: string; count: number }[]; retainedOnly: boolean };
  mail: { states: { state: string; count: number }[]; retainedOnly: boolean };
  coverage: { startedAt: number | null; earliestMetricDay: string | null; prunedThrough: number | null; retentionDays: number; approximate: boolean; previewIncluded: boolean; queueFlushed: boolean; installationDroppedSinceRestart: number; installationWriteErrorsSinceRestart: number };
};
export type UsageApp = { projectId: string; name: string; summary: UsageSummary | null; error?: string };
export type UsageOverview = { apps: UsageApp[]; nextCursor: string | null; from: string; to: string };

export function usageRange(days: number, now = new Date()) {
  const to = now.toISOString().slice(0, 10);
  const start = new Date(`${to}T00:00:00Z`);
  start.setUTCDate(start.getUTCDate() - days + 1);
  return { from: start.toISOString().slice(0, 10), to };
}
export function readUsageOverview(days: number, cursor?: string, signal?: AbortSignal) {
  return platformMcp.call<UsageOverview>("usage_overview", { ...usageRange(days), ...(cursor ? { cursor } : {}) }, signal);
}
export function readUsageSummary(deploymentId: string, days: number, signal?: AbortSignal) {
  return platformMcp.call<UsageSummary>("usage_summary", { deploymentId, ...usageRange(days) }, signal);
}

export const featureKinds = [
  { id: "data", name: "Data actions", kinds: ["action"], description: "Action calls. HTTP routes and scheduled runs appear separately." },
  { id: "auth", name: "Authentication", kinds: ["signup", "login"], description: "Successful signups and sign-ins. These are operations, not unique people." },
  { id: "files", name: "Files", kinds: ["upload", "file_read"], description: "Files uploaded and read." },
  { id: "routes", name: "HTTP routes", kinds: ["route"], description: "Successful requests to the app's declared routes." },
  { id: "scheduled", name: "Scheduled work", kinds: ["scheduled"], description: "Scheduled attempts, including retries and runs with no changes." },
  { id: "calls", name: "Service calls", kinds: ["call"], description: "Calls queued for delivery. Retries do not add another queued call." },
  { id: "notify", name: "Email notices", kinds: ["notify"], description: "Notice attempts, including notices the Kernel could not queue." },
] as const;
export function featureRows(summary: UsageSummary) {
  return featureKinds.map((feature) => {
    const kinds: readonly string[] = feature.kinds;
    const totals = summary.usage.totals.filter((row) => kinds.includes(row.kind));
    const origins = feature.id === "data" ? ["runtime", "continuation"] : feature.id === "routes" ? ["route"] : feature.id === "scheduled" ? ["schedule"] : [];
    const executions = summary.metrics.filter((m) => m.feature === "execution" && origins.includes(m.origin));
    const failures = executions.filter((m) => m.outcome === "failed").reduce((sum, m) => sum + m.count, 0);
    const refused = executions.filter((m) => m.outcome === "refused").reduce((sum, m) => sum + m.count, 0);
    const recorded = summary.usage.days.filter((row) => kinds.includes(row.kind)).map((row) => row.day).sort();
    const configured = feature.id === "auth" || feature.id === "files" ? "Available" : summary.actions.some((action) =>
      feature.id === "data" ? ["query", "mutation", "signal"].includes(action.kind) :
      feature.id === "routes" ? Boolean(action.http) : feature.id === "scheduled" ? Boolean(action.schedule) :
      feature.id === "calls" ? action.kind === "call" : Boolean(action.notify)) ? "Configured" : "Not configured";
    const notSent = feature.id === "notify" ? noticeOutcomes(summary).notSent : 0;
    return { ...feature, configured, count: totals.reduce((sum, row) => sum + row.count, 0), lastDay: [...recorded, ...executions.map((m) => m.day)].sort().at(-1), totals, failures, refused, notSent };
  });
}

// The Kernel names each notice "<action>: <reason>", where reason is "queued" or why nothing was queued (docs/mcp.md).
const noticeReasons: Record<string, string> = {
  "no-transport": "this installation has no email service",
  "owner-has-no-email": "the owner account has no email address",
  "no-account": "the caller had no app account",
  "unverified-email": "the caller's email is not verified",
  "app-limit": "the app's daily email budget was used up",
  "creator-limit": "the owner's daily email budget was used up",
  "recipient-limit": "the recipient's daily email budget was used up",
  "installation-limit": "the installation's daily email budget was used up",
  "sender-not-ready": "the email sender was not ready",
  "storage-error": "the email queue could not be written",
};
export function noticeOutcomes(summary: UsageSummary) {
  let queued = 0;
  const reasons: { action: string; reason: string; label: string; count: number }[] = [];
  for (const row of summary.usage.totals) {
    if (row.kind !== "notify") continue;
    const split = row.name.lastIndexOf(": ");
    if (split < 0) continue;
    const action = row.name.slice(0, split) === "*" ? "All actions" : row.name.slice(0, split);
    const reason = row.name.slice(split + 2);
    if (reason === "queued") queued += row.count;
    else reasons.push({ action, reason, label: noticeReasons[reason] ?? reason, count: row.count });
  }
  return { queued, notSent: reasons.reduce((sum, r) => sum + r.count, 0), reasons };
}

const outcomeLabels: Record<string, string> = { ok: "Correct", refused: "Refused by app rules", failed: "Failed", unchanged: "No changes" };
export const outcomeLabel = (outcome: string) => outcomeLabels[outcome] ?? outcome;
export function usageActivity(summary: UsageSummary) {
  // Session checks, Signals and notices can accompany an action, so they are not added to it.
  return summary.usage.totals.filter((r) => ["action", "route", "scheduled", "call"].includes(r.kind)).reduce((sum, r) => sum + r.count, 0);
}
export function executionTotals(metrics: Metric[]) {
  return metrics.filter((m) => m.feature === "execution").reduce((sum, m) => {
    sum[m.outcome] = (sum[m.outcome] ?? 0) + m.count;
    return sum;
  }, {} as Record<string, number>);
}
