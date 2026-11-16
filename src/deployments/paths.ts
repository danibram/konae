import { McpError, platformMcp } from "../mcp/client";


export type PathView = {
  deploymentId: string;
  enabled: boolean;
  name: string | null;
  acceptSharedOrigin: boolean;
  addresses: string[];
  domainConfigured: boolean;
  next?: string;
};

export type PathChange = { enabled: boolean; name?: string | null; acceptSharedOrigin?: boolean };

export const readPath = (deploymentId: string) => platformMcp.call<PathView>("path_read", { projectId: deploymentId });
export const setPath = (deploymentId: string, change: PathChange) => platformMcp.call<PathView>("path_set", { projectId: deploymentId, ...change });
export const clearPath = (deploymentId: string) => platformMcp.call<PathView>("path_clear", { projectId: deploymentId });

export function pathNeedsDomain(view: PathView): boolean {
  return !view.domainConfigured;
}

export function isSharedOriginRefusal(error: unknown): boolean {
  return error instanceof McpError && error.refusal?.code === "path.shared-origin";
}

export function pathAddressLabel(index: number): string {
  return index === 0 ? "By id" : "By name";
}
