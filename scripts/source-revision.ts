export type SourceRevisionScope = { installation: string; projectId: string };

export function sourceRevisionScope(url: string, projectId: string): SourceRevisionScope {
  return { installation: new URL("/mcp", url).href, projectId };
}

export function matchesSourceRevisionScope(value: Partial<SourceRevisionScope> | undefined, url: string, projectId: string) {
  if (!value?.installation || !value.projectId) return false;
  const expected = sourceRevisionScope(url, projectId);
  return value.installation === expected.installation && value.projectId === expected.projectId;
}
