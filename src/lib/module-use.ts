export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/"));
}

export function sourcesUseAuth(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.auth") || source.includes("miniapp.files") || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source));
}

export function sourcesUseActions(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => {
    if (source.includes("miniapp.actions")) return true;
    if (/\bCREATE\s+(?:TEMP(?:ORARY)?\s+)?TABLE\b/i.test(source) || /\bCREATE\s+(?:UNIQUE\s+)?INDEX\b/i.test(source)) return true;
    try {
      const actions = JSON.parse(source)?.actions;
      return actions !== null && typeof actions === "object" && Object.keys(actions).length > 0;
    } catch {
      return false;
    }
  });
}

export function sdkRequired(modules: Record<string, { on?: boolean }> | undefined, sources: Iterable<string>, visibility?: string): boolean {
  const files = Array.from(sources);
  const on = (name: string, inferred: boolean) => modules?.[name] ? modules[name].on === true : inferred;
  return on("actions", sourcesUseActions(files))
    || on("auth", visibility === "private" || sourcesUseAuth(files))
    || on("files", sourcesUseFiles(files));
}
