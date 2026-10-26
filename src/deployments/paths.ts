import { McpError, platformMcp } from "../mcp/client";

// The path address of a Deployment (ADR-0024). The Kernel's kernel-rs/src/path.rs
// answers these tools; the words below mirror its refusals and its `next` text.

export type PathView = {
  deploymentId: string;
  enabled: boolean;
  name: string | null;
  acceptSharedOrigin: boolean;
  /** `<domain>/p/<id>` first, then `<domain>/p/<name>`; empty while the path is off or the installation names no domain. */
  addresses: string[];
  /** False when the installation sets no LITTLEBOX_DOMAIN; `next` then says so. */
  domainConfigured: boolean;
  next?: string;
};

/** An omitted name keeps the stored one; null removes it. */
export type PathChange = { enabled: boolean; name?: string | null; acceptSharedOrigin?: boolean };

export const readPath = (deploymentId: string) => platformMcp.call<PathView>("path_read", { deploymentId });
export const setPath = (deploymentId: string, change: PathChange) => platformMcp.call<PathView>("path_set", { deploymentId, ...change });
export const clearPath = (deploymentId: string) => platformMcp.call<PathView>("path_clear", { deploymentId });

/** The installation names no domain, so the editor shows the Kernel's `next` and no switch. */
export function pathNeedsDomain(view: PathView): boolean {
  return !view.domainConfigured;
}

/** True when the Kernel refused a path that would share its origin without the owner's acceptance. */
export function isSharedOriginRefusal(error: unknown): boolean {
  return error instanceof McpError && error.refusal?.code === "path.shared-origin";
}

/** The label of an address: the Kernel lists the id address first and the name second. */
export function pathAddressLabel(index: number): string {
  return index === 0 ? "By id" : "By name";
}
