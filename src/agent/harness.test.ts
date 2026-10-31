import { describe, expect, test } from "bun:test";
import { createAssistantMessageEventStream, type AssistantMessage, type Model, type Models, type ToolCall } from "@earendil-works/pi-ai";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { cloneStarter, starterProject } from "../lib/starter";
import type { MiniAppProject } from "../lib/types";
import type { AgentSessionConfig, AgentUiEvent, CapsuleFiles, LiveDatabase, ValidationResult } from "./contracts";
import { createKonaeAgent, type KonaeAgentOptions } from "./harness";
import { emptyAgentKeyStore } from "./key-store";
import type { ReleaseFacts } from "./release-check";

// sql.js resolves its own wasm under bun; the browser path uses the bundled url loader.
let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());

const sessionConfig: AgentSessionConfig = { provider: "anthropic", modelId: "fake-model" };

const fakeModel = {
  id: "fake-model",
  name: "Fake",
  provider: "anthropic",
  api: "anthropic-messages",
  baseUrl: "https://example.invalid",
  reasoning: true,
} as unknown as Model<"anthropic-messages">;

const emptyUsage = {
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
  totalTokens: 0,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function assistant(content: AssistantMessage["content"], overrides: Partial<AssistantMessage> = {}): AssistantMessage {
  return {
    role: "assistant",
    content,
    api: "anthropic-messages",
    provider: "anthropic",
    model: "fake-model",
    usage: emptyUsage,
    stopReason: "stop",
    timestamp: Date.now(),
    ...overrides,
  };
}

type Turn = (stream: ReturnType<typeof createAssistantMessageEventStream>) => void;

/** A turn that streams one text block and stops. */
function say(text: string): Turn {
  return (stream) => {
    const partial = assistant([{ type: "text", text: "" }], { stopReason: "pending" });
    stream.push({ type: "start", partial });
    stream.push({ type: "text_start", contentIndex: 0, partial });
    stream.push({ type: "text_delta", contentIndex: 0, delta: text, partial });
    stream.push({ type: "text_end", contentIndex: 0, content: text, partial });
    const message = assistant([{ type: "text", text }]);
    stream.push({ type: "done", reason: "stop", message });
  };
}

/** A turn that asks for tool calls; the loop runs them and comes back for the next turn. */
function call(...calls: { name: string; arguments: Record<string, unknown> }[]): Turn {
  return (stream) => {
    const content = calls.map((entry, index): ToolCall => ({
      type: "toolCall",
      id: `call_${index}`,
      name: entry.name,
      arguments: entry.arguments,
    }));
    const partial = assistant(content, { stopReason: "pending" });
    stream.push({ type: "start", partial });
    stream.push({ type: "done", reason: "toolUse", message: assistant(content, { stopReason: "toolUse" }) });
  };
}

/** A provider failure, encoded the way pi requires: a final message, never a throw. */
function fail(message: string): Turn {
  return (stream) => {
    stream.push({
      type: "error",
      reason: "error",
      error: assistant([], { stopReason: "error", errorMessage: message }),
    });
  };
}

type Harness = {
  models: Models;
  requests: { tools: string[]; systemPrompt: string; messageCount: number; lastMessage: string; reasoning: unknown; apiKey?: string }[];
};

/** The text of the message that carries this turn's request, to see whether the capsule was seeded. */
function messageText(message: unknown): string {
  const content = (message as { content?: unknown } | undefined)?.content;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((block) => (block as { text?: string }).text ?? "").join("");
}

function stubModels(turns: Turn[]): Harness {
  const requests: Harness["requests"] = [];
  const queue = [...turns];
  const models = {
    getModel: (provider: string, id: string) => (provider === "anthropic" ? { ...fakeModel, id } : undefined),
    getModels: () => [fakeModel],
    streamSimple: (_model: Model<never>, context: { systemPrompt?: string; messages: unknown[]; tools?: { name: string }[] }, options?: Record<string, unknown>) => {
      requests.push({
        tools: (context.tools ?? []).map((tool) => tool.name),
        systemPrompt: context.systemPrompt ?? "",
        messageCount: context.messages.length,
        lastMessage: messageText(context.messages[context.messages.length - 1]),
        reasoning: options?.reasoning,
        apiKey: options?.apiKey as string | undefined,
      });
      const stream = createAssistantMessageEventStream();
      const turn = queue.shift() ?? say("done");
      queueMicrotask(() => turn(stream));
      return stream;
    },
  } as unknown as Models;
  return { models, requests };
}

function agentFor(turns: Turn[], options: Partial<KonaeAgentOptions> = {}) {
  const harness = stubModels(turns);
  const events: AgentUiEvent[] = [];
  const agent = createKonaeAgent(starterProject, sessionConfig, emptyAgentKeyStore(), {
    models: harness.models,
    loadSql,
    ...options,
  });
  agent.subscribe((event) => events.push(event));
  return { agent, events, harness };
}

const passing: ValidationResult = { ok: true, errors: [], checked: ["schema", "actions", "migration"] };

function stubValidator(result: ValidationResult) {
  const seen: CapsuleFiles[] = [];
  const databases: LiveDatabase[] = [];
  const facts: ReleaseFacts[] = [];
  return {
    seen,
    databases,
    facts,
    validator: {
      validate: async (files: CapsuleFiles, live: LiveDatabase = { state: "absent" }, known: ReleaseFacts = {}) => {
        seen.push(files);
        databases.push(live);
        facts.push(known);
        return result;
      },
    },
  };
}

const cssEdit = {
  name: "edit",
  arguments: { path: "/workspace/styles.css", old_string: "--ds-orange: #f45d35;", new_string: "--ds-orange: #3157d5;" },
};

describe("konae agent", () => {
  test("seeds the whole capsule in the first message and only the prompt afterwards", async () => {
    const { agent, harness } = agentFor([say("Listo."), say("Listo otra vez.")]);

    await agent.send("ponlo azul");
    await agent.send("y ahora verde");

    const [first, second] = harness.requests;
    expect(first.messageCount).toBe(1);
    expect(second.messageCount).toBe(3);
    expect(first.systemPrompt).toBe(second.systemPrompt);
    expect(first.systemPrompt).toContain("/workspace/actions.json");
  });

  test("maps deltas, tool calls and turn ends to ui events", async () => {
    const { agent, events } = agentFor([call(cssEdit), say("He cambiado el acento.")]);

    await agent.send("ponlo azul");

    expect(events.filter((event) => event.type === "assistant-delta").map((event) => (event as { text: string }).text).join("")).toBe(
      "He cambiado el acento.",
    );
    const start = events.find((event) => event.type === "tool-start") as { id: string; name: string; detail: string };
    expect(start).toEqual({ type: "tool-start", id: "call_0", name: "edit", detail: "styles.css" });
    const end = events.find((event) => event.type === "tool-end") as { id: string; name: string; ok: boolean };
    expect(end.name).toBe("edit");
    // The chip strip keys on the tool call id, so both ends of a call must carry it.
    expect(end.id).toBe(start.id);
    expect(end.ok).toBe(true);
    expect(events.filter((event) => event.type === "turn-end")).toHaveLength(2);
  });

  test("a css-only edit reaches ready through the gate the harness runs itself", async () => {
    const { agent, events } = agentFor([call(cssEdit), say("Hecho.")]);

    await agent.send("ponlo azul");

    const gate = agent.applyGateStatus();
    expect(gate.state).toBe("ready");
    expect(gate.state === "ready" && gate.changedFiles).toEqual(["styles.css"]);
    expect(events.some((event) => event.type === "validation" && event.result.ok)).toBe(true);
    const proposal = agent.proposal();
    expect(proposal?.files["styles.css"]).toContain("#3157d5");
    expect(proposal?.validation?.ok).toBe(true);
    expect(starterProject.files["styles.css"]).toContain("#f45d35");
  });

  test("a failing gate blocks apply", async () => {
    const failing: ValidationResult = { ok: false, errors: [{ stage: "schema", message: "schema.sql does not run." }], checked: [] };
    const { agent } = agentFor([call(cssEdit), say("Hecho.")], { validator: stubValidator(failing).validator });

    await agent.send("rompe el esquema");

    const gate = agent.applyGateStatus();
    expect(gate.state).toBe("blocked");
    expect(gate.state === "blocked" && gate.result.errors[0].stage).toBe("schema");
  });

  test("an edit after the model's own validate_capsule makes that result stale", async () => {
    const secondEdit = {
      name: "edit",
      arguments: { path: "/workspace/index.html", old_string: "Guest", new_string: "invitado" },
    };
    const gate = stubValidator(passing);
    const { agent, events } = agentFor(
      [call(cssEdit), call({ name: "validate_capsule", arguments: {} }), call(secondEdit), say("Hecho.")],
      { validator: gate.validator },
    );

    await agent.send("dos cambios");

    // Once for the model's own call, once for the hard gate on the newer workspace.
    expect(gate.seen).toHaveLength(2);
    expect(gate.seen[0]["index.html"]).toContain("Guest");
    expect(gate.seen[1]["index.html"]).toContain("invitado");
    expect(events.filter((event) => event.type === "validation")).toHaveLength(2);
    expect(agent.proposal()?.changedFiles).toEqual(["styles.css", "index.html"]);
    expect(agent.applyGateStatus().state).toBe("ready");
  });

  test("an edit in the same message as validate_capsule is never reported as validated", async () => {
    const gate = stubValidator(passing);
    const { agent } = agentFor(
      // One assistant message carrying both calls: the reason edits execute sequentially.
      [call(cssEdit, { name: "validate_capsule", arguments: {} }), say("Hecho.")],
      { validator: gate.validator },
    );

    await agent.send("cambia y valida");

    const status = agent.applyGateStatus();
    expect(status.state).toBe("ready");
    // Whatever the model claimed, the verdict that enabled Apply saw the edit.
    expect(gate.seen[gate.seen.length - 1]["styles.css"]).toContain("#3157d5");
    expect(agent.proposal()?.validation?.ok).toBe(true);
  });

  test("both edits of a two-edit message reach the validated proposal", async () => {
    const second = {
      name: "edit",
      arguments: { path: "/workspace/styles.css", old_string: "--ds-bar: 3px;", new_string: "--ds-bar: 4px;" },
    };
    const { agent } = agentFor([call(cssEdit, second), say("Hecho.")]);

    await agent.send("dos cambios en el mismo archivo");

    const proposal = agent.proposal();
    expect(proposal?.files["styles.css"]).toContain("#3157d5");
    expect(proposal?.files["styles.css"]).toContain("--ds-bar: 4px;");
    expect(agent.applyGateStatus().state).toBe("ready");
  });

  test("the gate validates against the live database, not only a fresh one", async () => {
    const gate = stubValidator(passing);
    const live = new Uint8Array([1, 2, 3]);
    const { agent } = agentFor([call(cssEdit, { name: "validate_capsule", arguments: {} }), say("Hecho.")], {
      validator: gate.validator,
      liveDatabase: async () => live,
    });

    await agent.send("ponlo azul");

    expect(gate.databases.length).toBeGreaterThan(0);
    expect(gate.databases.every((database) => database.state === "loaded" && database.bytes === live)).toBe(true);
  });

  test("the gate and validate_capsule both see the project's release facts", async () => {
    const gate = stubValidator(passing);
    const known: ReleaseFacts = { secrets: [{ name: "orders_hook", use: "route" }] };
    const { agent } = agentFor([call(cssEdit, { name: "validate_capsule", arguments: {} }), call({ ...cssEdit, arguments: { ...cssEdit.arguments, old_string: "#3157d5", new_string: "#2147c5" } }), say("Hecho.")], {
      validator: gate.validator,
      releaseFacts: async () => known,
    });

    await agent.send("ponlo azul");

    // One from validate_capsule, one from the gate that re-runs after the last edit.
    expect(gate.facts.length).toBe(2);
    expect(gate.facts.every((facts) => facts === known)).toBe(true);
  });

  test("a gate that cannot run blocks apply instead of waiting forever", async () => {
    const broken = { validate: async () => { throw new Error("wasm fetch failed"); } };
    const { agent, events } = agentFor([call(cssEdit), say("Hecho.")], { validator: broken });

    await agent.send("ponlo azul");

    const status = agent.applyGateStatus();
    expect(status.state).toBe("blocked");
    expect(status.state === "blocked" && status.result.errors[0].message).toContain("could not run");
    expect(events.some((event) => event.type === "error" && event.message.includes("could not run"))).toBe(true);
    expect(agent.proposal()?.validation?.ok).toBe(false);
  });

  test("no changes means no proposal and an empty gate", async () => {
    const { agent } = agentFor([say("No hace falta cambiar nada.")]);

    await agent.send("solo preguntaba");

    expect(agent.proposal()).toBeNull();
    expect(agent.applyGateStatus()).toEqual({ state: "empty" });
  });

  test("a provider failure arrives as an error event, not a rejection", async () => {
    const { agent, events } = agentFor([fail("401 invalid x-api-key")]);

    await agent.send("ponlo azul");

    const errors = events.filter((event) => event.type === "error") as { message: string }[];
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toContain("401");
    expect(agent.applyGateStatus()).toEqual({ state: "empty" });
  });

  test("an unresolvable model id is reported instead of thrown", async () => {
    const { agent, events } = agentFor([say("never sent")]);
    agent.configure({ provider: "anthropic", modelId: "  " });

    await agent.send("ponlo azul");

    const error = events.find((event) => event.type === "error") as { message: string } | undefined;
    expect(error?.message).toContain("No model id");
    expect(events.some((event) => event.type === "assistant-delta")).toBe(false);
  });

  test("escalates the thinking level for the turn after a data-contract edit", async () => {
    const schemaEdit = {
      name: "edit",
      arguments: {
        path: "/workspace/schema.sql",
        old_string: "CREATE TABLE IF NOT EXISTS tasks",
        new_string: "CREATE TABLE IF NOT EXISTS tasks",
      },
    };
    const { agent, harness } = agentFor([call({ ...schemaEdit, arguments: { ...schemaEdit.arguments, new_string: "CREATE TABLE IF NOT EXISTS tasks -- edited" } }), say("Hecho.")]);

    await agent.send("añade una columna");

    expect(harness.requests[0].reasoning).toBe("low");
    expect(harness.requests[1].reasoning).toBe("medium");
  });

  test("discard drops the workspace edits and the transcript", async () => {
    const { agent, harness } = agentFor([call(cssEdit), say("Hecho."), say("Otra vez.")]);

    await agent.send("ponlo azul");
    agent.discard();

    expect(agent.proposal()).toBeNull();
    await agent.send("ponlo azul");
    // The capsule is seeded again, so the transcript starts from one message.
    expect(harness.requests[harness.requests.length - 1].messageCount).toBe(1);
  });

  test("discard during a live run leaves nothing seeded and no transcript", async () => {
    let session: ReturnType<typeof agentFor> | undefined;
    const discardMidRun: Turn = (stream) => {
      session?.agent.discard();
      say("interrumpido")(stream);
    };
    session = agentFor([call(cssEdit), discardMidRun, say("de nuevo")]);

    await session.agent.send("ponlo azul");
    expect(session.agent.proposal()).toBeNull();

    await session.agent.send("otra vez");
    // The capsule is seeded again, because the transcript the model had is gone.
    expect(session.harness.requests[session.harness.requests.length - 1].messageCount).toBe(1);
  });

  test("a file changed outside the agent re-seeds the capsule instead of trusting the transcript", async () => {
    const { agent, harness } = agentFor([call(cssEdit), say("Hecho."), say("Y esto.")]);

    await agent.send("ponlo azul");
    const proposal = agent.proposal();
    // Apply, plus a hand edit to a file the agent never saw.
    agent.reseed({ ...proposal!.files, "app.js": "// written by hand" });

    await agent.send("y ahora otra cosa");
    // The model's memory of app.js is wrong, so the capsule travels again.
    expect(harness.requests[harness.requests.length - 1].lastMessage).toContain("// written by hand");
  });

  /**
   * The bug this pins: only the five original Capsule files were compared, so a
   * hand edit to a nested source file looked like no change and the model kept
   * working from its stale copy.
   */
  test("a nested source file changed outside the agent also re-seeds the capsule", async () => {
    const { agent, harness } = agentFor([call(cssEdit), say("Hecho."), say("Y esto.")]);

    await agent.send("ponlo azul");
    const proposal = agent.proposal();
    agent.reseed({ ...proposal!.files });
    agent.reseed({ ...proposal!.files, "src/lib/helper.ts": "export const byHand = true;" });

    await agent.send("y ahora otra cosa");
    expect(harness.requests[harness.requests.length - 1].lastMessage).toContain("export const byHand = true;");
  });

  /**
   * The bug this pins: `reseed` moved the workspace but not the session's copy of
   * the Project, so a discard restored the pre-Apply Capsule and the next run
   * proposed the old file back over the applied one — with a passing gate.
   */
  test("a discard after an apply restores the applied files, not the ones they replaced", async () => {
    const write = (content: string) => ({
      name: "write",
      arguments: { path: "/workspace/styles.css", content },
    });
    const { agent, harness } = agentFor([
      call(write("/* v2 applied */")), say("Hecho."),
      call(write("/* v3 rejected */")), say("Hecho."),
      say("Y ahora otra cosa."),
    ]);

    await agent.send("cambia el css");
    const applied = agent.proposal();
    expect(applied?.files["styles.css"]).toBe("/* v2 applied */");
    agent.reseed(applied!.files);

    await agent.send("otra vez");
    agent.discard();

    expect(agent.proposal()).toBeNull();
    await agent.send("y ahora otra cosa");
    // The capsule the model is seeded with is the applied one, not the original.
    const seeded = harness.requests[harness.requests.length - 1].lastMessage;
    expect(seeded).toContain("/* v2 applied */");
    expect(seeded).not.toContain("--ds-orange: #f45d35;");
  });

  /**
   * The bug this pins: the editor only retargeted on a Project id or manifest
   * change, so a hand edit never reached the session and Apply merged
   * gate-approved files onto a Capsule the gate had never seen.
   */
  test("a hand edit to the same Project drops the proposal instead of validating around it", async () => {
    const { agent, events, harness } = agentFor([call(cssEdit), say("Hecho."), say("Otra vez.")]);

    await agent.send("ponlo azul");
    expect(agent.applyGateStatus().state).toBe("ready");

    const handEdited = {
      ...starterProject,
      files: { ...starterProject.files, "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);" },
    };
    agent.retarget(handEdited);

    expect(agent.proposal()).toBeNull();
    expect(agent.applyGateStatus()).toEqual({ state: "empty" });
    expect(events.some((event) => event.type === "error" && event.message.includes("changed outside the agent"))).toBe(true);

    await agent.send("otra vez");
    // The model is shown the Capsule as the owner left it, not as it remembers it.
    expect(harness.requests[harness.requests.length - 1].lastMessage).toContain("CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);");
  });

  test("retargeting to another Project cannot apply this one's files to it", async () => {
    const { agent, harness } = agentFor([call(cssEdit), say("Hecho."), say("Nuevo proyecto.")]);

    await agent.send("ponlo azul");
    expect(agent.applyGateStatus().state).toBe("ready");

    const other: MiniAppProject = { ...cloneStarter(), id: "app_other", name: "Notes" };
    agent.retarget(other);

    expect(agent.proposal()).toBeNull();
    expect(agent.applyGateStatus()).toEqual({ state: "empty" });

    await agent.send("cambia algo aquí");
    const last = harness.requests[harness.requests.length - 1];
    expect(last.messageCount).toBe(1);
    expect(agent.proposal()).toBeNull();
  });

  test("a manifest change reaches the system prompt of a session that is already open", async () => {
    const { agent, harness } = agentFor([say("Uno."), say("Dos.")]);

    await agent.send("hola");
    const deployed = cloneStarter();
    deployed.manifest = { ...deployed.manifest, visibility: "public" };
    agent.retarget({ ...deployed, id: starterProject.id, files: starterProject.files });

    await agent.send("otra vez");
    expect(harness.requests[0].systemPrompt).toContain("private deployment");
    expect(harness.requests[harness.requests.length - 1].systemPrompt).toContain("public deployment");
  });

  test("reseed clears the proposal after apply while keeping the conversation", async () => {
    const { agent, harness } = agentFor([call(cssEdit), say("Hecho."), say("Y ya está.")]);

    await agent.send("ponlo azul");
    const applied = agent.proposal();
    expect(applied).not.toBeNull();
    agent.reseed(applied!.files);

    expect(agent.proposal()).toBeNull();
    expect(agent.applyGateStatus()).toEqual({ state: "empty" });

    await agent.send("gracias");
    const last = harness.requests[harness.requests.length - 1];
    expect(last.messageCount).toBeGreaterThan(1);
    // The model already knows those files: it made the change, so nothing is re-seeded.
    expect(last.lastMessage).not.toContain("/workspace/index.html");
  });

  describe("a proposal that breaks a declaration rule is refused with the Kernel's words", () => {
    const starterActions = JSON.parse(starterProject.files["actions.json"]) as { version: 1; actions: Record<string, Record<string, unknown>> };
    /** The starter's actions.json with one action changed. */
    function withAction(name: string, change: Record<string, unknown>): string {
      const next = structuredClone(starterActions);
      next.actions[name] = { ...next.actions[name], ...change };
      return JSON.stringify(next, null, 2);
    }

    test.each([
      [
        "a guard on a query",
        withAction("tasks.list", { guard: "Nothing to list." }),
        "Action tasks.list is a query, so it cannot declare a guard. Declare it on a mutation.",
      ],
      [
        "an HTTP route with callers on an authenticated action",
        withAction("tasks.create", { access: "authenticated", http: { method: "POST", path: "/api/tasks", callers: ["key"] } }),
        "Action tasks.create is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.",
      ],
      [
        "a signal property that is neither an input nor actor",
        withAction("tasks.create", { signal: { name: "task.created", properties: ["title", "owner"] } }),
        "Action tasks.create names signal property owner, which is not one of its input bindings.",
      ],
    ])("%s", async (_, source, kernelWords) => {
      const write = { name: "write", arguments: { path: "/workspace/actions.json", content: source } };
      const { agent, harness } = agentFor([call(write, { name: "validate_capsule", arguments: {} }), say("Listo.")]);

      await agent.send("cambia las acciones");

      // The model read the Kernel's sentence in its own validate_capsule result.
      expect(harness.requests[1].lastMessage).toContain(kernelWords);
      const gate = agent.applyGateStatus();
      expect(gate.state).toBe("blocked");
      expect(gate.state === "blocked" && gate.result.errors.map((error) => error.detail)).toEqual([kernelWords]);
    });
  });
});

describe("agent platform binding", () => {
  const domainTools = [
    { name: "domain_read", description: "Read a domain.", inputSchema: { type: "object", properties: { deploymentId: { type: "string" } }, required: ["deploymentId"] } },
    { name: "domain_clear", description: "Clear a domain.", inputSchema: { type: "object", properties: { deploymentId: { type: "string" } }, required: ["deploymentId"] } },
  ];

  test("retarget resolves the new project's deployment on the next turn", async () => {
    const seen: string[] = [];
    const calls: unknown[] = [];
    const platform = {
      listTools: async () => domainTools,
      call: async (name: string, args: Record<string, unknown>) => { calls.push({ name, args }); return { deploymentId: args.deploymentId, hostname: null }; },
      deploymentForProject: (projectId: string) => { seen.push(projectId); return `deployment-${projectId}`; },
      confirm: async () => { throw new Error("Unexpected confirmation"); },
    };
    const { agent, events } = agentFor([
      call({ name: "domain_read", arguments: {} }), say("Read the domain."),
      call({ name: "domain_read", arguments: {} }), say("Read the other domain."),
    ], { platform });
    await agent.send("Read my domain");
    agent.retarget({ ...starterProject, id: "other-project" });
    await agent.send("Read this domain");
    expect(seen).toEqual([starterProject.id, "other-project"]);
    expect(calls).toEqual([
      { name: "domain_read", args: { deploymentId: `deployment-${starterProject.id}` } },
      { name: "domain_read", args: { deploymentId: "deployment-other-project" } },
    ]);
    expect(events.filter((event) => event.type === "tool-end" && event.name === "domain_read").every((event) => event.type === "tool-end" && event.ok)).toBe(true);
    expect(agent.proposal()).toBeNull();
    expect(agent.applyGateStatus().state).toBe("empty");
    agent.dispose();
  });

  test("retarget during owner confirmation prevents the old run from changing either app", async () => {
    let confirm!: (approved: boolean) => void;
    let entered!: () => void;
    const waiting = new Promise<void>((resolve) => { entered = resolve; });
    const calls: string[] = [];
    const { agent } = agentFor([call({ name: "domain_clear", arguments: {} }), say("Done")], {
      platform: {
        listTools: async () => domainTools,
        call: async (name) => { calls.push(name); return {}; },
        deploymentForProject: (projectId) => `deployment-${projectId}`,
        confirm: () => { entered(); return new Promise<boolean>((resolve) => { confirm = resolve; }); },
      },
    });
    const run = agent.send("Remove my custom domain");
    await waiting;
    agent.retarget({ ...starterProject, id: "other-project" });
    confirm(true);
    await run;
    expect(calls).toEqual([]);
    expect(agent.proposal()).toBeNull();
    agent.dispose();
  });

  test("a session that cannot list the owner's tools still edits files", async () => {
    const { agent, harness } = agentFor([say("Only files here.")], {
      platform: {
        listTools: async () => { throw Object.assign(new Error("Unauthorized"), { status: 401 }); },
        call: async () => { throw new Error("Unexpected call"); },
        deploymentForProject: () => null,
        confirm: async () => false,
      },
    });
    await agent.send("hola");
    expect(harness.requests[0].tools).toEqual(["read", "edit", "write", "validate_capsule", "preview_errors"]);
    agent.dispose();
  });
});
