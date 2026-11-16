export function sourcesUseFiles(sources: Iterable<string>): boolean {
  return Array.from(sources).some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/"));
}
