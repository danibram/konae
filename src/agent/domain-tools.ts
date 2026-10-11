import { Type } from "@earendil-works/pi-ai";
import type { AgentTool } from "./contracts";

export type DomainView = {
  deploymentId: string;
  hostname: string | null;
  status: "none" | "pending" | "active";
  generatedUrl: string;
  url?: string;
  record?: { type: "TXT"; name: string; value: string };
  verifiedAt?: number | null;
  dns?: string;
  next?: string;
};

export type DomainMutationRequest = {
  projectId: string;
  deploymentId: string;
  operation: "set" | "clear";
  hostname: string | null;
  previousHostname: string | null;
};

/** The editor supplies owner-authenticated operations and an owner confirmation dialog. */
export type AgentDomainBinding = {
  deploymentForProject(projectId: string): string | null | Promise<string | null>;
  readDomain(deploymentId: string): Promise<DomainView>;
  setDomain(deploymentId: string, hostname: string): Promise<DomainView>;
  clearDomain(deploymentId: string): Promise<{ deploymentId: string; cleared: string | null; status: "none"; generatedUrl: string }>;
  confirmMutation?(request: DomainMutationRequest): Promise<boolean>;
  onChanged?(): void;
};

export type DomainToolContext = {
  projectId: string;
  /** A tool from an abandoned run cannot act on a newly selected project. */
  isCurrent(): boolean;
  binding: AgentDomainBinding;
};

const routingNotice = "Active proves TXT ownership and Kernel alias registration only. It does not verify A/CNAME routing or TLS. The operator's proxy must terminate TLS and forward Host unchanged. No DNS-provider records were written.";

export function createDomainTools(context: DomainToolContext): AgentTool<any>[] {
  const { binding, projectId } = context;

  function check(signal?: AbortSignal) {
    if (signal?.aborted || !context.isCurrent()) throw new Error("The current project or agent run changed. Ask again in the current project.");
  }

  async function deployment(signal?: AbortSignal) {
    check(signal);
    const id = await binding.deploymentForProject(projectId);
    check(signal);
    if (!id) throw new Error("Publish this project first, then manage its custom domain.");
    return id;
  }

  async function stillBound(id: string, signal?: AbortSignal) {
    if (await deployment(signal) !== id) throw new Error("This project's deployment changed. Read its custom domain again before changing it.");
  }

  async function read(id: string, signal?: AbortSignal) {
    const view = await binding.readDomain(id);
    await stillBound(id, signal);
    if (view.deploymentId !== id) throw new Error("The domain response belongs to another deployment.");
    return view;
  }

  function output(value: DomainView | { deploymentId: string; cleared: string | null; status: "none"; generatedUrl: string }) {
    const summary = "cleared" in value
      ? value.cleared ? `Cleared custom domain ${value.cleared}. The generated hostname keeps answering.` : "No custom domain was set. The generated hostname keeps answering."
      : value.hostname ? `Custom domain ${value.hostname} is ${value.status}.` : "This deployment has no custom domain.";
    return { content: [{ type: "text" as const, text: `${summary}\n${JSON.stringify(value, null, 2)}\n\n${routingNotice}` }], details: value };
  }

  async function mutate(operation: "set" | "clear", hostname: string | null, signal?: AbortSignal) {
    const id = await deployment(signal);
    const before = await read(id, signal);
    if (!binding.confirmMutation) throw new Error("The editor must confirm the owner's intent before changing a custom domain. No domain was changed.");
    const approved = await binding.confirmMutation({ projectId, deploymentId: id, operation, hostname, previousHostname: before.hostname });
    check(signal);
    if (!approved) throw new Error("The owner declined the custom domain change. No domain was changed.");
    const latest = await read(id, signal);
    if (latest.hostname !== before.hostname || latest.status !== before.status) {
      throw new Error("The custom domain changed while waiting for confirmation. Read it again and ask the owner to confirm the current change.");
    }
    await stillBound(id, signal);
    const value = operation === "set" ? await binding.setDomain(id, hostname!) : await binding.clearDomain(id);
    binding.onChanged?.();
    check(signal);
    if (value.deploymentId !== id) throw new Error("The domain response belongs to another deployment.");
    return output(value);
  }

  return [
    {
      name: "domain_read",
      label: "Read custom domain",
      description: "Read the current project's published deployment custom domain and TXT claim. No deployment ID is accepted. Publish the project first if it has no deployment. Active proves ownership and Kernel registration only, not A/CNAME routing or TLS readiness.",
      parameters: Type.Object({}, { additionalProperties: false }),
      execute: async (_id, _params, signal) => output(await read(await deployment(signal), signal)),
    },
    {
      name: "domain_set",
      label: "Set custom domain",
      executionMode: "sequential",
      description: "Only on explicit owner intent, set a hostname on the current project's deployment. The editor asks the owner to confirm. A different hostname removes the previous alias. The first call creates a pending TXT claim; repeating the same hostname rechecks that claim. Show the returned TXT record. Active proves ownership and Kernel registration only, not A/CNAME routing or TLS readiness. Never invent a DNS target or promise DNS-provider writes.",
      parameters: Type.Object({ hostname: Type.String({ minLength: 1, description: "The exact hostname the owner requested, with no scheme or path." }) }, { additionalProperties: false }),
      execute: async (_id, params, signal) => {
        const hostname = (params as { hostname?: unknown } | null)?.hostname;
        if (typeof hostname !== "string" || !hostname.trim()) throw new Error("Name the custom hostname the owner requested.");
        return mutate("set", hostname, signal);
      },
    },
    {
      name: "domain_clear",
      label: "Clear custom domain",
      executionMode: "sequential",
      description: "Only on explicit owner intent, remove the current project's custom domain alias. The editor confirms the exact previous hostname with the owner. The generated hostname keeps answering. This removes no DNS-provider records.",
      parameters: Type.Object({}, { additionalProperties: false }),
      execute: async (_id, _params, signal) => mutate("clear", null, signal),
    },
  ];
}
