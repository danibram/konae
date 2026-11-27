import { describe, expect, test } from "bun:test";
import { createPlatformTools, databaseCallWrites, isBoundedReadOnlySelect, platformToolPolicy, type AgentPlatformBinding, type McpToolDefinition, type PlatformConfirmation } from "./platform-tools";

const text = { type: "string", minLength: 1 };
const listed: McpToolDefinition[] = [
  { name: "platform_guide", description: "Read a guide topic.", inputSchema: { type: "object", properties: { topic: { type: "string" } }, required: ["topic"] } },
  { name: "project_inspect", description: "Inspect a project.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "domain_read", description: "Read a domain.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "modules_read", description: "Read modules.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "module_set", description: "Set a module.", inputSchema: { type: "object", properties: { projectId: text, module: text, on: { type: "boolean" }, options: { type: "object" }, expected: { type: "object" } }, required: ["projectId", "module", "expected"] } },
  { name: "domain_set", description: "Set a domain.", inputSchema: { type: "object", properties: { projectId: text, hostname: text }, required: ["projectId", "hostname"] } },
  { name: "path_read", description: "Read a path address.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "path_set", description: "Set a path address.", inputSchema: { type: "object", properties: { projectId: text, enabled: { type: "boolean" }, name: text, acceptSharedOrigin: { type: "boolean" } }, required: ["projectId", "enabled"] } },
  { name: "path_clear", description: "Clear a path address.", inputSchema: { type: "object", properties: { projectId: text }, required: ["projectId"] } },
  { name: "database_execute", description: "Run a database operation.", inputSchema: { type: "object", properties: { projectId: text, operation: { type: "string" }, input: { type: "object" } }, required: ["projectId", "operation"] } },
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
    deploymentForProject: (projectId) => projectId,
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
  test("classifies a single SELECT with an optional trailing semicolon as a read", () => {
    expect(databaseCallWrites({ operation: "sql", input: { sql: " SELECT id FROM tasks LIMIT 5;  " } })).toBe(false);
    expect(databaseCallWrites({ operation: "sql", input: { sql: "SELECT id FROM tasks LIMIT 5; DELETE FROM tasks" } })).toBe(true);
    expect(databaseCallWrites({ operation: "sql", input: { sql: "WITH rows AS (SELECT 1) SELECT * FROM rows LIMIT 1" } })).toBe(true);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks LIMIT 50; ")).toBe(true);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks LIMIT 1001")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks -- LIMIT 10")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks /* LIMIT 10 */")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks WHERE title = 'a LIMIT 10 string'")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM (SELECT id FROM tasks LIMIT 10)")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks LIMIT 1+1000")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks LIMIT 1, 1001")).toBe(false);
    expect(isBoundedReadOnlySelect("SELECT id FROM tasks LIMIT 10 OFFSET 40")).toBe(true);
  });

  test("only the allowlisted tools reach the model, never source saves, credentials, secrets or transfers", async () => {
    const tools = await toolsFor(binding().value);
    expect(Object.keys(tools).sort()).toEqual(["database_execute", "domain_read", "domain_set", "module_set", "modules_read", "path_clear", "path_read", "path_set", "platform_guide", "project_inspect"]);
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
      { name: "domain_read", args: { projectId: "p1" } },
    ]);
    await expect(run(tools.domain_read, { projectId: "other-project" })).rejects.toThrow("acts on the open project only. Leave out projectId.");
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
    expect(approve.calls).toEqual([{ name: "domain_set", args: { hostname: "notes.example.org", projectId: "p1" } }]);

    const decline = binding({ confirm: async () => false });
    tools = await toolsFor(decline.value);
    await expect(run(tools.domain_set, { hostname: "notes.example.org" })).rejects.toThrow("The owner declined domain_set. Nothing was changed.");
    expect(decline.calls).toEqual([]);
    await expect(run(tools.module_set, { module: "files", on: false, expected: { on: true } })).rejects.toThrow("The files module cannot be changed by the build agent.");
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
      { name: "path_read", args: { projectId: "p1" } },
      { name: "path_set", args: { enabled: true, name: "spring-menu", projectId: "p1" } },
      { name: "path_clear", args: { projectId: "p1" } },
    ]);
  });

  test("module_set asks the owner, pins the project, and reads can run without confirmation", async () => {
    const { value, calls, confirmations } = binding();
    const tools = await toolsFor(value);
    const state = { on: false, canonical: null, image: null };
    await run(tools.modules_read);
    await run(tools.module_set, { module: "seo", on: true, options: { canonical: "path" }, expected: state });
    expect(confirmations).toEqual([{ projectId: "p1", tool: "module_set", summary: "The agent will turn on Search listing (seo) and set Canonical address to Path address.", arguments: { module: "seo", on: true, options: { canonical: "path" }, expected: state } }]);
    expect(calls).toEqual([
      { name: "modules_read", args: { projectId: "p1" } },
      { name: "module_set", args: { module: "seo", on: true, options: { canonical: "path" }, expected: state, projectId: "p1" } },
      { name: "modules_read", args: { projectId: "p1" } },
    ]);
    const decline = binding({ confirm: async () => false });
    const declinedTools = await toolsFor(decline.value);
    await expect(run(declinedTools.module_set, { module: "seo", on: true, expected: state })).rejects.toThrow("The owner declined module_set. Nothing was changed.");
    expect(decline.calls).toEqual([]);
  });

  test("the build agent can enable Documents, App MCP and File intake with confirmation and read-back", async () => {
    for (const [module, title] of [["documents", "Documents"], ["mcp", "App MCP"], ["file-intake", "File intake"]]) {
      const { value, calls, confirmations } = binding();
      const tools = await toolsFor(value);
      const expected = { on: false };
      await run(tools.modules_read);
      const result = await run(tools.module_set, { module, on: true, expected });
      expect(confirmations).toHaveLength(1);
      expect(confirmations[0]?.summary).toBe(`The agent will turn on ${title} (${module}).`);
      expect(calls).toEqual([
        { name: "modules_read", args: { projectId: "p1" } },
        { name: "module_set", args: { module, on: true, expected, projectId: "p1" } },
        { name: "modules_read", args: { projectId: "p1" } },
      ]);
      expect(result.details).toMatchObject({ outcome: "observed", readBackTool: "modules_read" });
    }
  });

  test("rechecks the operation's current permission after the owner confirmation", async () => {
    const dispatches: string[] = [];
    let canWrite = true;
    const platform = binding({
      confirm: async () => { canWrite = false; return true; },
      callAgent: async (name) => { dispatches.push(name); return { can: { read: true, write: canWrite, publish: false }, projectIds: ["p1"] }; },
    });
    const tools = await toolsFor(platform.value);
    await expect(run(tools.module_set, { module: "seo", on: true, expected: { on: false } })).rejects.toThrow("session_inspect.can.publish is not granted");
    expect(dispatches).toEqual(["session_inspect"]);
    expect(platform.calls).toEqual([]);
  });

  test("auth policy changes ask the owner and name the policy in the confirmation", async () => {
    const { value, confirmations } = binding();
    const tools = await toolsFor(value);
    await run(tools.module_set, { module: "auth", on: true, options: { signupPolicy: { mode: "allowlist", allowlist: ["@example.test"] } } });
    expect(confirmations[0]?.summary).toBe("The agent will turn on App accounts (auth) and set Who can create an account to Allowlist (@example.test).");
  });

  test("a stale module write reads current state before the agent reports the refusal", async () => {
    const { value, calls } = binding({ call: async (name, args) => {
      calls.push({ name, args });
      if (name === "module_set") throw Object.assign(new Error("Module seo changed since it was read. Read modules_read and try again."), { refusal: { code: "module.changed" } });
      return { modules: { seo: { state: { on: true } } } };
    } });
    const tools = await toolsFor(value);
    await expect(run(tools.module_set, { module: "seo", on: false, expected: { on: false } })).rejects.toThrow("Current modules_read state: {");
    expect(calls.map(({ name }) => name)).toEqual(["module_set", "modules_read"]);
  });

  test("a module write reports success when its read-back fails", async () => {
    const { value, calls } = binding({ call: async (name, args) => {
      calls.push({ name, args });
      if (name === "modules_read" && calls.length > 1) throw new Error("read unavailable");
      return { changed: true };
    } });
    const tools = await toolsFor(value);
    const result = await run(tools.module_set, { module: "seo", on: true, expected: { on: false } });
    expect(result.content[0]?.text).toContain('"outcome": "unknown"');
    expect(result.content[0]?.text).toContain("has not been verified");
    expect(calls.map(({ name }) => name)).toEqual(["module_set", "modules_read"]);
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
    let published = true;
    const { value, calls } = binding({ deploymentForProject: () => published ? "p1" : null, confirm: async () => { published = false; return true; } });
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
