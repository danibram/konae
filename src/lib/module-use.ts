export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => /miniapp\s*\.\s*files/.test(source) || source.includes("/_konae/files/"));
}

export function sourcesUseAuth(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => /miniapp\s*\.\s*(?:auth|files)/.test(source) || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source));
}
