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
export type ExpectedPath = Pick<PathView, "enabled" | "name" | "acceptSharedOrigin">;

export const readPath = (deploymentId: string) => platformMcp.call<PathView>("path_read", { projectId: deploymentId });
export const setPath = (deploymentId: string, change: PathChange, expected: ExpectedPath) => platformMcp.call<PathView>("path_set", { projectId: deploymentId, ...change, expected });
export const clearPath = (deploymentId: string, expected: ExpectedPath) => platformMcp.call<PathView>("path_clear", { projectId: deploymentId, expected });

export function pathNeedsDomain(view: PathView): boolean {
  return !view.domainConfigured;
}

export function isSharedOriginRefusal(error: unknown): boolean {
  return error instanceof McpError && error.refusal?.code === "path.shared-origin";
}

export function pathAddressLabel(index: number): string {
  return index === 0 ? "By id" : "By name";
}
