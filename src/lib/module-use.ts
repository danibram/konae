export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/"));
}

export function sourcesUseAuth(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.auth") || source.includes("miniapp.files") || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source));
}

export function sourcesUseActions(sources: Iterable<string> | Record<string, string>): boolean {
  const entries = Symbol.iterator in Object(sources) ? Array.from(sources as Iterable<string>).map((source) => ["", source] as const) : Object.entries(sources as Record<string, string>);
  return entries.some(([name, source]) => {
    if (source.includes("miniapp.actions")) return true;
    if (name === "schema.sql" && /\bCREATE\s+(?:(?:TEMP(?:ORARY)?|UNIQUE)\s+)*(?:TABLE|INDEX)\b/i.test(source)) return true;
    if (name !== "actions.json") return false;
    try {
      const value = JSON.parse(source);
      const actions = value && typeof value === "object" && !Array.isArray(value) ? value.actions : undefined;
      return actions !== null && typeof actions === "object" && !Array.isArray(actions) && Object.keys(actions).length > 0;
    } catch { return false; }
  });
}

export function sdkRequired(modules: Record<string, { on?: boolean }> | undefined, sources: Iterable<string> | Record<string, string>, visibility?: string): boolean {
  const sourceFiles = Symbol.iterator in Object(sources) ? undefined : sources as Record<string, string>;
  const files = sourceFiles ? Object.values(sourceFiles) : Array.from(sources as Iterable<string>);
  const entries = sourceFiles ? Object.entries(sourceFiles) : files.map((source) => ["", source] as const);
  const sdkMentions = entries.some(([name, source]) => name !== "actions.json" && name !== "schema.sql" && /\bminiapp\b/.test(source));
  const browserCall = (name: string) => files.some((source) => new RegExp(`(?:miniapp\\s*\\.\\s*${name}\\b|miniapp\\s*\\[\\s*["']${name}["']\\s*\\]|window\\s*\\[\\s*["']miniapp["']\\s*\\]\\s*\\.\\s*${name}\\b|window\\s*\\[\\s*["']miniapp["']\\s*\\]\\s*\\[\\s*["']${name}["']\\s*\\])`).test(source));
  const on = (name: string, inferred: boolean) => modules?.[name] ? modules[name].on === true : inferred;
  const actionsUsed = entries.some(([name, source]) => {
    if (source.includes("miniapp.actions")) return true;
    if (name === "schema.sql" && /\bCREATE\s+(?:(?:TEMP(?:ORARY)?|UNIQUE)\s+)*(?:TABLE|INDEX)\b/i.test(source)) return true;
    if (name !== "actions.json" && name !== "") return false;
    try {
      const value = JSON.parse(source);
      const actions = value && typeof value === "object" && !Array.isArray(value) ? value.actions : undefined;
      return actions !== null && typeof actions === "object" && !Array.isArray(actions) && Object.keys(actions).length > 0;
    } catch { return false; }
  });
  const authUsed = files.some((source) => source.includes("miniapp.auth") || source.includes("miniapp.files") || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source));
  const filesUsed = files.some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/"));
  const moduleUsesSdk = on("actions", actionsUsed)
    || on("auth", visibility === "private" || authUsed)
    || on("files", filesUsed);
  return browserCall("page")
    || (moduleUsesSdk && sdkMentions);
}
