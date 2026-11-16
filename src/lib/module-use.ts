export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => /miniapp\s*\.\s*files/.test(source) || source.includes("/_konae/files/"));
}
