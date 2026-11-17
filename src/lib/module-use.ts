export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/"));
}

export function sourcesUseAuth(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.auth") || source.includes("miniapp.files") || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source));
}
