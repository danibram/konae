import { describe, expect, test } from "bun:test";
import { createPlatformTools, databaseCallWrites, platformToolPolicy, type AgentPlatformBinding, type McpToolDefinition, type PlatformConfirmation } from "./platform-tools";

const text = { type: "string", minLength: 1 };
const listed: McpToolDefinition[] = [
  { name: "platform_guide", description: "Read a guide topic.", inputSchema: { type: "object", properties: { topic: { type: "string" } }, required: ["topic"] } },
  { name: "project_inspect", description: "Inspect a project.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "domain_read", description: "Read a domain.", inputSchema: { type: "object", properties: { deploymentId: text }, required: ["deploymentId"] } },
  { name: "domain_set", description: "Set a domain.", inputSchema: { type: "object", properties: { deploymentId: text, hostname: text }, required: ["deploymentId", "hostname"] } },
  { name: "path_read", description: "Read a path address.", inputSchema: { type: "object", properties: { deploymentId: text }, required: ["deploymentId"] } },
  { name: "path_set", description: "Set a path address.", inputSchema: { type: "object", properties: { deploymentId: text, enabled: { type: "boolean" }, name: text, acceptSharedOrigin: { type: "boolean" } }, required: ["deploymentId", "enabled"] } },
  { name: "path_clear", description: "Clear a path address.", inputSchema: { type: "object", properties: { deploymentId: text }, required: ["deploymentId"] } },
  { name: "database_execute", description: "Run a database operation.", inputSchema: { type: "object", properties: { deploymentId: text, operation: { type: "string" }, input: { type: "object" } }, required: ["deploymentId", "operation"] } },
  { name: "project_sources_save", description: "Save sources.", inputSchema: { type: "object", properties: { projectId: text, transferId: text, expectedRevision: { type: "integer" } } } },
  { name: "token_create", description: "Delegate a credential.", inputSchema: { type: "object", properties: {} } },
  { name: "secret_set", description: "Set a secret.", inputSchema: { type: "object", properties: { projectId: text, name: text, value: text } } },
  { name: "transfer_begin", description: "Begin a transfer.", inputSchema: { type: "object", properties: {} } },
];

function binding(overrides: Partial<AgentPlatformBinding> = {}) {
  const calls: { name: string; args: Record<string, unknown> }[] = [];
  const confirmations: PlatformConfirmation[] = [];
  const value: AgentPlatformBinding = {
    listTools: async () => listed,
    call: async (name, args) => { calls.push({ name, args }); return { ok: name }; },
    deploymentForProject: (projectId) => `deployment-${projectId}`,
    confirm: async (request) => { confirmations.push(request); return true; },
    ...overrides,
  };
  return { value, calls, confirmations };
}

async function toolsFor(platform: AgentPlatformBinding, isCurrent = () => true) {
  const tools = await createPlatformTools({ projectId: "p1", isCurrent, binding: platform });
  return Object.fromEntries(tools.map((tool) => [tool.name, tool]));
}

const run = (tool: { execute: (...args: any[]) => Promise<any> } | undefined, params: Record<string, unknown> = {}) => tool!.execute("call", params, undefined);

describe("the build agent's tools from the owner's MCP", () => {
  test("only the allowlisted tools reach the model, never source saves, credentials, secrets or transfers", async () => {
    const tools = await toolsFor(binding().value);
    expect(Object.keys(tools).sort()).toEqual(["database_execute", "domain_read", "domain_set", "path_clear", "path_read", "path_set", "platform_guide", "project_inspect"]);
    for (const name of ["project_sources_save", "project_sources_read", "token_create", "api_keys_create", "secret_set", "transfer_begin", "project_export"]) {
      expect(platformToolPolicy[name]).toBeUndefined();
    }
  });

  test("the model never sees or names a project or deployment; the open one is filled in", async () => {
    const { value, calls } = binding();
    const tools = await toolsFor(value);
    const schema = tools.domain_set!.parameters as { properties: Record<string, unknown>; required: string[] };
    expect(Object.keys(schema.properties)).toEqual(["hostname"]);
    expect(schema.required).toEqual(["hostname"]);
    expect("required" in (tools.project_inspect!.parameters as object)).toBe(false);
    await run(tools.project_inspect);
    await run(tools.domain_read);
    expect(calls).toEqual([
      { name: "project_inspect", args: { projectId: "p1" } },
      { name: "domain_read", args: { deploymentId: "deployment-p1" } },
    ]);
    await expect(run(tools.domain_read, { deploymentId: "deployment-other" })).rejects.toThrow("acts on the open project only. Leave out deploymentId.");
    await expect(run(tools.project_inspect, { projectId: "p2" })).rejects.toThrow("Leave out projectId.");
    expect(calls).toHaveLength(2);
  });

  test("a tool that needs the published app says to publish first", async () => {
    const { value, calls } = binding({ deploymentForProject: () => null });
    const tools = await toolsFor(value);
    await expect(run(tools.domain_read)).rejects.toThrow("Publish this project first.");
    await run(tools.project_inspect);
    expect(calls.map((call) => call.name)).toEqual(["project_inspect"]);
  });

  test("a change waits for the owner, and a refusal leaves everything as it was", async () => {
    const approve = binding();
    let tools = await toolsFor(approve.value);
    await run(tools.domain_set, { hostname: "notes.example.org" });
    expect(approve.confirmations).toEqual([{ projectId: "p1", tool: "domain_set", summary: "The agent will set notes.example.org as this app's domain and check its TXT record.", arguments: { hostname: "notes.example.org" } }]);
    expect(approve.calls).toEqual([{ name: "domain_set", args: { hostname: "notes.example.org", deploymentId: "deployment-p1" } }]);

    const decline = binding({ confirm: async () => false });
    tools = await toolsFor(decline.value);
    await expect(run(tools.domain_set, { hostname: "notes.example.org" })).rejects.toThrow("The owner declined domain_set. Nothing was changed.");
    expect(decline.calls).toEqual([]);
  });

  test("the path address reads without asking, and every change asks the owner first", async () => {
    const { value, calls, confirmations } = binding();
    const tools = await toolsFor(value);
    await run(tools.path_read);
    expect(confirmations).toEqual([]);
    await run(tools.path_set, { enabled: true, name: "spring-menu" });
    await run(tools.path_clear);
    expect(confirmations.map((request) => request.tool)).toEqual(["path_set", "path_clear"]);
    expect(confirmations[0]!.summary).toBe("The agent will turn on this app's path address with the name spring-menu.");
    expect(confirmations[1]!.summary).toBe("The agent will turn off this app's path address and remove its name. The generated address and any custom domain keep working.");
    expect(calls).toEqual([
      { name: "path_read", args: { deploymentId: "deployment-p1" } },
      { name: "path_set", args: { enabled: true, name: "spring-menu", deploymentId: "deployment-p1" } },
      { name: "path_clear", args: { deploymentId: "deployment-p1" } },
    ]);
  });

  test("accepting the shared origin is said in the confirmation", async () => {
    const { value, confirmations } = binding();
    const tools = await toolsFor(value);
    await run(tools.path_set, { enabled: true, acceptSharedOrigin: true });
    expect(confirmations[0]!.summary).toBe("The agent will turn on this app's path address. The path shares its origin with the home and every other app on this installation, so they can use this app's signed-in sessions.");
  });

  test("database_execute reads without asking and asks before it writes", async () => {
    expect(databaseCallWrites({ operation: "browse" })).toBe(false);
    expect(databaseCallWrites({ operation: "sql", input: { sql: "SELECT id FROM tasks LIMIT 5;" } })).toBe(false);
    expect(databaseCallWrites({ operation: "sql", input: { sql: "DELETE FROM tasks" } })).toBe(true);
    expect(databaseCallWrites({ operation: "sql", input: { sql: "SELECT 1; DELETE FROM tasks" } })).toBe(true);
    expect(databaseCallWrites({ operation: "delete-row" })).toBe(true);
    expect(databaseCallWrites({ operation: "create-backup" })).toBe(true);
    const { value, confirmations } = binding();
    const tools = await toolsFor(value);
    await run(tools.database_execute, { operation: "sql", input: { sql: "SELECT id FROM tasks LIMIT 5" } });
    expect(confirmations).toEqual([]);
    await run(tools.database_execute, { operation: "delete-row", input: { table: "tasks", rowid: 1 } });
    expect(confirmations.map((request) => request.tool)).toEqual(["database_execute"]);
  });

  test("a project switched while the owner decides runs nothing", async () => {
    let current = true;
    const { value, calls } = binding({ confirm: async () => { current = false; return true; } });
    const tools = await toolsFor(value, () => current);
    await expect(run(tools.domain_set, { hostname: "notes.example.org" })).rejects.toThrow("The current project or agent run changed.");
    expect(calls).toEqual([]);
  });

  test("a deployment replaced while the owner decides runs nothing", async () => {
    let id = "deployment-a";
    const { value, calls } = binding({ deploymentForProject: () => id, confirm: async () => { id = "deployment-b"; return true; } });
    const tools = await toolsFor(value);
    await expect(run(tools.domain_set, { hostname: "notes.example.org" })).rejects.toThrow("deployment changed while waiting for the owner");
    expect(calls).toEqual([]);
  });

  test("an ended session says to sign in, and a Kernel refusal keeps its words and code", async () => {
    const signedOut = binding({ call: async () => { throw Object.assign(new Error("Unauthorized"), { status: 401 }); } });
    let tools = await toolsFor(signedOut.value);
    await expect(run(tools.domain_read)).rejects.toThrow("The owner's session ended. Ask the owner to sign in again");
    const refused = binding({ call: async () => { throw Object.assign(new Error("Hostname is taken."), { refusal: { code: "domain-taken", hint: { fix: "Choose another hostname." } } }); } });
    tools = await toolsFor(refused.value);
    await expect(run(tools.domain_read)).rejects.toThrow("Hostname is taken. (domain-taken) Choose another hostname.");
  });

  test("publish opens the owner's Deploy dialog and reports what the owner chose", async () => {
    const outcomes = [
      { status: "published", url: "https://notes.example.org", visibility: "private" },
      { status: "declined" },
      { status: "failed", error: "Every query names its LIMIT." },
    ] as const;
    const opened: string[] = [];
    for (const outcome of outcomes) {
      const { value } = binding({ publish: async (projectId) => { opened.push(projectId); return outcome; } });
      const tools = await toolsFor(value);
      const result = run(tools.publish);
      if (outcome.status === "published") expect((await result).content[0].text).toContain("Published as a private app at https://notes.example.org.");
      else await expect(result).rejects.toThrow(outcome.status === "declined" ? "closed the Deploy dialog" : "Publishing was refused: Every query names its LIMIT.");
    }
    expect(opened).toEqual(["p1", "p1", "p1"]);
  });
});
