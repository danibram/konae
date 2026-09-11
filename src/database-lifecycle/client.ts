import type { DatabaseMigrationPlan, PlanDatabaseChangeInput } from "./contracts";

const adminKey = import.meta.env.VITE_LITTLEBOX_ADMIN_KEY ?? "local-development-key";

export async function planDatabaseChange(input: PlanDatabaseChangeInput, options: { signal?: AbortSignal } = {}): Promise<DatabaseMigrationPlan> {
  const response = await fetch("/_littlebox/admin/database-lifecycle/plan", {
    method: "POST",
    headers: { "content-type": "application/json", "x-littlebox-admin-key": adminKey },
    body: JSON.stringify(input),
    signal: options.signal,
  });
  const raw = await response.text();
  const body = (() => {
    try { return JSON.parse(raw) as { result?: DatabaseMigrationPlan; error?: string }; } catch { return {}; }
  })();
  if (!response.ok || !body.result) throw new Error(body.error ?? (raw.trim() || "Could not generate the Drizzle migration."));
  return body.result;
}
