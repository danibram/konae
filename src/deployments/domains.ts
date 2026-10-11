import { platformMcp } from "../mcp/client";
import type { DomainRules } from "../lib/domain-name";

export type DomainView = {
  deploymentId: string;
  hostname: string | null;
  status: "none" | "pending" | "active";
  generatedUrl: string;
  url?: string;
  record?: { type: "TXT"; name: string; value: string };
  verifiedAt?: number | null;
  dns?: string;
  rules?: DomainRules;
};

export const readDomain = (deploymentId: string) => platformMcp.call<DomainView>("domain_read", { deploymentId });
export const setDomain = (deploymentId: string, hostname: string) => platformMcp.call<DomainView>("domain_set", { deploymentId, hostname });
export const clearDomain = (deploymentId: string) => platformMcp.call<{ deploymentId: string; cleared: string | null; status: "none"; generatedUrl: string }>("domain_clear", { deploymentId });

export function domainTrafficTarget(generatedUrl: string): { local: boolean; hostname: string } {
  const hostname = new URL(generatedUrl).hostname;
  return { hostname, local: hostname === "localhost" || hostname.endsWith(".localhost") || hostname === "127.0.0.1" || hostname === "[::1]" };
}
