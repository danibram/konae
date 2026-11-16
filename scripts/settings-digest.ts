import { createHash } from "node:crypto";
import { sdkContract } from "../src/sdk/contract";

export function settingsDigest(value: unknown): string {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

export function settingsMatchAfterLocalModuleDecision(local: unknown, remote: unknown): boolean {
  if (!local || typeof local !== "object" || Array.isArray(local) || !remote || typeof remote !== "object" || Array.isArray(remote)) return false;
  const localRecord = local as Record<string, unknown>;
  const remoteRecord = remote as Record<string, unknown>;
  if (Object.hasOwn(remoteRecord, "modules") || !matchesLegacyModuleDefaults(localRecord.modules)) return false;
  const { modules: _modules, ...localSettings } = localRecord;
  return settingsDigest(localSettings) === settingsDigest(remoteRecord);
}

function matchesLegacyModuleDefaults(value: unknown): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const modules = value as Record<string, unknown>;
  const switchable = sdkContract.modules.filter((module) => module.switch);
  if (Object.keys(modules).length !== switchable.length) return false;
  return switchable.every((module) => {
    const state = modules[module.name];
    if (!state || typeof state !== "object" || Array.isArray(state)) return false;
    const record = state as Record<string, unknown>;
    if (typeof record.on !== "boolean" || (module.name !== "files" && record.on)) return false;
    for (const option of module.options) {
      if (module.name === "files" && option.key === "fileAccess" && record[option.key] === "app-users") continue;
      if (record[option.key] !== option.default) return false;
    }
    return Object.keys(record).every((key) => key === "on" || module.options.some((option) => option.key === key));
  });
}

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${canonical(object[key])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}
