import type { DatabaseMigrationPlan, PlanDatabaseChangeInput } from "./contracts";
import { platformMcp } from "../mcp/client";

export async function planDatabaseChange(input: PlanDatabaseChangeInput, options: { signal?: AbortSignal } = {}): Promise<DatabaseMigrationPlan> {
  return platformMcp.call<DatabaseMigrationPlan>("schema_plan", input, options.signal);
}
