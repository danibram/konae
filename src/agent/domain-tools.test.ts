import { describe, expect, test } from "bun:test";
import { createDomainTools, type AgentDomainBinding, type DomainMutationRequest, type DomainView } from "./domain-tools";

function setup(overrides: Partial<AgentDomainBinding> = {}) {
  let current = true;
  let deploymentId: string | null = "deployment-a";
  let view: DomainView = { deploymentId: "deployment-a", hostname: "old.example.org", status: "active", generatedUrl: "https://generated.example.org" };
  const lookups: string[] = [];
  const mutations: unknown[] = [];
  const confirmations: DomainMutationRequest[] = [];
  let changed = 0;
  const binding: AgentDomainBinding = {
    deploymentForProject: (projectId) => { lookups.push(projectId); return deploymentId; },
    readDomain: async () => ({ ...view }),
    setDomain: async (id, hostname) => {
      mutations.push(["set", id, hostname]);
      return { ...view, deploymentId: id, hostname, status: "pending", record: { type: "TXT", name: `_littlebox.${hostname}`, value: "issued-token" } };
    },
    clearDomain: async (id) => {
      mutations.push(["clear", id]);
      return { deploymentId: id, cleared: "old.example.org", status: "none", generatedUrl: view.generatedUrl };
    },
    confirmMutation: async (request) => { confirmations.push(request); return true; },
    onChanged: () => { changed++; },
    ...overrides,
  };
  const tools = createDomainTools({ projectId: "project-a", isCurrent: () => current, binding });
  const call = (name: string, params: Record<string, unknown> = {}, signal?: AbortSignal) => tools.find((tool) => tool.name === name)!.execute("call", params, signal);
  return {
    tools, call, lookups, mutations, confirmations,
    changed: () => changed,
    abandon: () => { current = false; },
    unpublish: () => { deploymentId = null; },
    replaceDeployment: () => { deploymentId = "deployment-b"; },
    replaceAlias: () => { view = { ...view, hostname: "other.example.org" }; },
  };
}

describe("integrated domain tools", () => {
  test("binds reads to the project and accepts no model-selected deployment ID", async () => {
    const subject = setup();
    const result = await subject.call("domain_read", { deploymentId: "someone-elses-deployment" });
    expect(subject.lookups.every((id) => id === "project-a")).toBe(true);
    expect(result.details).toMatchObject({ deploymentId: "deployment-a" });
    for (const tool of subject.tools) {
      expect(tool.parameters.properties).not.toHaveProperty("deploymentId");
      expect(tool.parameters.additionalProperties).toBe(false);
    }
    expect(JSON.stringify(result.content)).toContain("does not verify A/CNAME routing or TLS");
  });

  test("unpublished projects receive a publish-first error for every operation", async () => {
    const subject = setup();
    subject.unpublish();
    for (const name of ["domain_read", "domain_set", "domain_clear"]) {
      await expect(subject.call(name, { hostname: "app.example.org" })).rejects.toThrow("Publish this project first");
    }
    expect(subject.mutations).toEqual([]);
    expect(subject.confirmations).toEqual([]);
  });

  test("confirms the old alias and exact requested hostname before setting, then reports TXT claim", async () => {
    const subject = setup();
    const result = await subject.call("domain_set", { hostname: "app.example.org" });
    expect(subject.confirmations).toEqual([{
      projectId: "project-a", deploymentId: "deployment-a", operation: "set", hostname: "app.example.org", previousHostname: "old.example.org",
    }]);
    expect(subject.mutations).toEqual([["set", "deployment-a", "app.example.org"]]);
    expect(result.details).toMatchObject({ status: "pending", record: { name: "_littlebox.app.example.org", value: "issued-token" } });
    expect(subject.changed()).toBe(1);
  });

  test("clearing confirms the exact alias and preserves the generated URL", async () => {
    const subject = setup();
    const result = await subject.call("domain_clear");
    expect(subject.confirmations[0]).toMatchObject({ operation: "clear", hostname: null, previousHostname: "old.example.org" });
    expect(subject.mutations).toEqual([["clear", "deployment-a"]]);
    expect(result.details).toMatchObject({ status: "none", generatedUrl: "https://generated.example.org" });
  });

  test("missing or declined owner confirmation blocks mutations", async () => {
    for (const confirmMutation of [undefined, async () => false]) {
      const subject = setup({ confirmMutation });
      await expect(subject.call("domain_clear")).rejects.toThrow();
      expect(subject.mutations).toEqual([]);
      expect(subject.changed()).toBe(0);
    }
  });

  test("an abandoned run cannot read or mutate a newly selected app", async () => {
    const subject = setup();
    subject.abandon();
    for (const name of ["domain_read", "domain_set", "domain_clear"]) {
      await expect(subject.call(name, { hostname: "app.example.org" })).rejects.toThrow("current project or agent run changed");
    }
    expect(subject.lookups).toEqual([]);
  });

  test("retarget, deployment replacement, alias replacement and abort during confirmation block dispatch", async () => {
    for (const change of ["abandon", "replaceDeployment", "replaceAlias", "abort"] as const) {
      const controller = new AbortController();
      const subject = setup({ confirmMutation: async () => {
        if (change === "abort") controller.abort();
        else subject[change]();
        return true;
      } });
      await expect(subject.call("domain_clear", {}, controller.signal)).rejects.toThrow();
      expect(subject.mutations).toEqual([]);
    }
  });

  test("a deployment lookup completing after retarget cannot dispatch", async () => {
    const subject = setup({ deploymentForProject: async () => { subject.abandon(); return "deployment-a"; } });
    await expect(subject.call("domain_set", { hostname: "app.example.org" })).rejects.toThrow("current project or agent run changed");
    expect(subject.mutations).toEqual([]);
  });

  test("the backend remains authoritative and its error is preserved", async () => {
    const subject = setup({ setDomain: async () => { throw new Error("Requires publish permission."); } });
    await expect(subject.call("domain_set", { hostname: "app.example.org" })).rejects.toThrow("Requires publish permission.");
    expect(subject.changed()).toBe(0);
  });

  test("a response for another deployment cannot authorize a change", async () => {
    const subject = setup({ readDomain: async () => ({ deploymentId: "deployment-b", hostname: null, status: "none", generatedUrl: "https://other.example.org" }) });
    await expect(subject.call("domain_clear")).rejects.toThrow("another deployment");
    expect(subject.confirmations).toEqual([]);
    expect(subject.mutations).toEqual([]);
  });
});
