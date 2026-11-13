import { platformMcp } from "../mcp/client";

export type UsageCount = { kind: string; name: string; count: number };
export type DailyCount = UsageCount & { day: string };
export type Metric = { day: string; feature: "execution" | "signal"; name: string; origin: string; outcome: string; count: number; lastAt: number };
export type PublishedAction = { name: string; kind: string; http: { method: string; path: string } | null; signal: { name: string } | null; schedule: unknown; notify: unknown };
/** One signed-in app account's activity in the period. `email` is null once the account is gone. */
export type AccountActivity = { account: string; email: string | null; opens: number; actions: number; activeDays: number; lastAt: number };
export type UsageAccounts = { active: number; opens: number; actions: number; days: { day: string; active: number }[]; top: AccountActivity[]; limit: number; startedAt: number | null };
export type StoragePart = { part: string; bytes: number; limit: number | null; exported: boolean };
export type UsageStorage = { total: number; files: number; parts: StoragePart[] };
export type UsageSummary = {
  deploymentId: string; projectId: string; projectName: string; url: string; updatedAt: number;
  usage: { from: string; to: string; days: DailyCount[]; totals: UsageCount[] };
  metrics: Metric[]; actions: PublishedAction[];
  accounts: UsageAccounts;
  storage: UsageStorage;
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
  { id: "pages", name: "Page views", kinds: ["page_view"], description: "Browser loads of the app's pages. A view is a load, not a person, and a single-page app counts only its first load." },
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
    const configured = feature.id === "auth" || feature.id === "files" || feature.id === "pages" ? "Available" : summary.actions.some((action) =>
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
  "owner-has-no-email": "the Deployment has no contact email",
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

/** Every UTC day from `from` to `to`, both included. */
export function rangeDays(from: string, to: string) {
  const days: string[] = [];
  const day = new Date(`${from}T00:00:00Z`);
  const end = Date.parse(`${to}T00:00:00Z`);
  while (day.getTime() <= end) {
    days.push(day.toISOString().slice(0, 10));
    day.setUTCDate(day.getUTCDate() + 1);
  }
  return days;
}
export type DailyPoint = { day: string; value: number; parts: Record<string, number> };
/** One point per day of the range, zero where the Kernel recorded nothing, so a quiet day reads as a gap rather than disappearing. */
export function dailyUsage(summary: UsageSummary, kind: string): DailyPoint[] {
  const byDay = new Map<string, Record<string, number>>();
  for (const row of summary.usage.days) {
    if (row.kind !== kind) continue;
    const parts = byDay.get(row.day) ?? {};
    parts[row.name] = (parts[row.name] ?? 0) + row.count;
    byDay.set(row.day, parts);
  }
  return rangeDays(summary.usage.from, summary.usage.to).map((day) => {
    const parts = byDay.get(day) ?? {};
    return { day, value: Object.values(parts).reduce((sum, n) => sum + n, 0), parts };
  });
}
/** Executions per day, with each result as a part, over the summary's range. */
export function dailyExecutions(summary: UsageSummary): DailyPoint[] {
  const byDay = new Map<string, Record<string, number>>();
  for (const metric of summary.metrics) {
    if (metric.feature !== "execution") continue;
    const parts = byDay.get(metric.day) ?? {};
    parts[metric.outcome] = (parts[metric.outcome] ?? 0) + metric.count;
    byDay.set(metric.day, parts);
  }
  return rangeDays(summary.usage.from, summary.usage.to).map((day) => {
    const parts = byDay.get(day) ?? {};
    return { day, value: Object.values(parts).reduce((sum, n) => sum + n, 0), parts };
  });
}
export function kindTotal(summary: UsageSummary, kind: string) {
  return summary.usage.totals.filter((row) => row.kind === kind).reduce((sum, row) => sum + row.count, 0);
}
/** Page views by the file the Kernel served, most viewed first. */
export function pageViews(summary: UsageSummary) {
  return summary.usage.totals.filter((row) => row.kind === "page_view").map((row) => ({ page: row.name, count: row.count })).sort((a, b) => b.count - a.count || a.page.localeCompare(b.page));
}

const units = ["KiB", "MiB", "GiB", "TiB"];
/** Bytes in the binary units the Kernel's limits use: `840 bytes`, `1.4 MiB`. */
export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} ${bytes === 1 ? "byte" : "bytes"}`;
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value >= 100 ? Math.round(value) : Number(value.toFixed(1))} ${units[unit]}`;
}

export const storageParts: Record<string, { name: string; description: string }> = {
  database: { name: "Database", description: "The app's own tables, database.sqlite." },
  files: { name: "Uploaded files", description: "Files people uploaded through the app." },
  accounts: { name: "App accounts", description: "Accounts, sessions and invitations, identity.sqlite." },
  usage: { name: "Usage history", description: "These counts and the app's Signals, usage.sqlite." },
  published: { name: "Published pages", description: "The built files the Kernel serves." },
  sources: { name: "Sources", description: "Pages, actions.json and schema.sql as saved." },
  backups: { name: "Backups", description: "Copies this installation keeps. An export does not carry them." },
};
/** Each part of the app's storage with its share of a limit, largest first. Backups are left out: an export does not carry them. */
export function storageRows(storage: UsageStorage) {
  return storage.parts.filter((part) => part.exported).map((part) => {
    const share = part.limit ? part.bytes / part.limit : null;
    return { ...part, name: storageParts[part.part]?.name ?? part.part, share, tone: share === null ? undefined : share >= 0.95 ? "danger" as const : share >= 0.8 ? "warning" as const : undefined };
  }).sort((a, b) => b.bytes - a.bytes);
}

/** Signed-in accounts active each day of the range, zero on a day with none. */
export function dailyActiveAccounts(summary: UsageSummary) {
  const byDay = new Map((summary.accounts.days).map((row) => [row.day, row.active]));
  return rangeDays(summary.usage.from, summary.usage.to).map((day) => ({ day, value: byDay.get(day) ?? 0 }));
}
