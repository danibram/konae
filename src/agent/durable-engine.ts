import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { defineExtension, defineTool, hook, Harness, createRegistry, GenerationTask, ToolTask, ToolResultEntry, ROOT_CONVERSATION_ID, type ToolRegistration } from "@earendil-works/pi-durable";
import type { MiniAppProject } from "../lib/types";
import type { AgentExecution } from "../mcp/client";
import { textFiles } from "../lib/source-files";
import { buildSystemPrompt } from "./system-prompt";
import { createDurableModels, durableModelFor, modelsWithConfiguredModel } from "./durable-models";
import { openDurableStorage, readDurableConversation, uploadDurableDetail, type DurableStorageOptions } from "./durable-storage";
import { boundModelText, redactDurableValue } from "./durable-trace";
import { createCapsuleWorkspace } from "./capsule-env";
import { createCapsuleValidator, type SqlJsLoader } from "./gate";
import { createCapsuleTools, readLiveDatabase, readValidationFacts, type CapsuleToolHooks } from "./tools";
import { createPlatformTools, type AgentPlatformBinding } from "./platform-tools";
import type { AgentKeyStore, AgentSessionConfig, CapsuleFiles, AgentUiEvent, ApplyGateStatus, KonaeAgent, AgentProposal, ValidationResult } from "./contracts";

/** Shared writable Durable execution stays deliberately inactive through #28. */
export const SHARED_DURABLE_ACTIVATION = false;

export type DurableEngineOptions = {
  project: MiniAppProject;
  ownerId?: string;
  /** The Konae conversation UUID; Pi's numeric conversation id is not the owner-route address. */
  conversationId?: string;
  expectedGeneration?: number;
  sourceRevision?: number;
  recoveredFiles?: CapsuleFiles;
  storage?: import("@earendil-works/pi-durable").Storage;
  openStorage?: () => Promise<import("@earendil-works/pi-durable").Storage>;
  fetch?: typeof fetch;
  keyStore: AgentKeyStore;
  sessionConfig: AgentSessionConfig;
  platform?: AgentPlatformBinding;
  onMutation?: (files: CapsuleFiles) => void;
  localOnly?: boolean;
  isCurrent?: () => boolean;
  assertExecution?: () => Promise<void>;
  executionIdentity?: () => Omit<AgentExecution, "callId" | "observesCallId"> | null;
  loadSql?: SqlJsLoader;
  liveDatabase?: CapsuleToolHooks["liveDatabase"];
  previewErrors?: CapsuleToolHooks["previewErrors"];
  releaseFacts?: CapsuleToolHooks["releaseFacts"];
  sourceFiles?: CapsuleToolHooks["sourceFiles"];
  onModelEvent?: (event: any) => void;
  onValidation?: (result: ValidationResult) => void;
  onCheckpointStatus?: (status: import("./durable-checkpoints").CheckpointStatus, files: CapsuleFiles, saved?: { revision: number; project: MiniAppProject }) => void;
};

/** Opens a reader with scheduler recovery disabled; history inspection never starts provider work. */
export async function openDurableHistory(projectId: string, conversationId: string, ownerId?: string) {
  const info = await readDurableConversation(projectId, conversationId, ownerId);
  const storage = await openDurableStorage({ projectId, conversationId, ownerId });
  const nativeBoundary = (storage as any).hydratedSequence as number;
  const reportedBoundary = await (storage as any).lastSequence() as number;
  if (nativeBoundary !== info.lastSeq || reportedBoundary !== info.lastSeq) {
    await storage.close(BACKGROUND_CONTEXT);
    throw new Error("Native conversation replay does not match its reported high-water sequence.");
  }
  const registry = createRegistry();
  const models = createDurableModels(emptyKeyStore());
  const harness = await Harness.open(storage, { models, registry, readOnly: true }, BACKGROUND_CONTEXT);
  return { info, harness, conversation: await harness.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT), nativeSnapshot: (storage as any).nativeSnapshot as import("./durable-storage").NativeBatchSnapshot, close: () => harness.close(BACKGROUND_CONTEXT) };
}

/** Browser-owned local engine candidate. The editor still selects the legacy adapter until #29/#30 validation. */
export async function openLocalDurableEngine(options: DurableEngineOptions) {
  if (!options.localOnly && !options.storage && !options.openStorage) throw new Error("Shared Durable activation is held for the checkpoint and coordination follow-ups.");
  const files = textFiles(options.project);
  const workspace = createCapsuleWorkspace(files, options.recoveredFiles ?? files);
  const validator = createCapsuleValidator(options.loadSql);
  workspace.onMutation(() => options.onMutation?.(workspace.serialize().files));
  const platformBoundaries = new Map<string, unknown>();
  const tools = createCapsuleTools(workspace, validator, {
    sourceFiles: options.sourceFiles ?? (() => options.project),
    liveDatabase: options.liveDatabase,
    previewErrors: options.previewErrors,
    releaseFacts: options.releaseFacts,
      onValidated: (_files, result) => options.onValidation?.(result),
  });
  const projectId = options.project.id;
  if (options.platform) {
    const platformTools = await createPlatformTools({
      projectId,
      isCurrent: options.isCurrent ?? (() => false),
      binding: options.platform,
      refreshSession: Boolean(options.openStorage),
      onCallBoundary: (callId, boundary) => platformBoundaries.set(callId, boundary),
    });
    tools.push(...platformTools);
  }
  const durableModels = createDurableModels(options.keyStore);
  const model = durableModelFor(options.sessionConfig, durableModels);
  const extension = defineExtension({
    name: "konae-editor",
    tools: tools.map((tool) => durableTool(tool, options, platformBoundaries)) as ToolRegistration[],
    hooks: [
      hook(ToolTask, {
        beforeTool(call) {
          return { arguments: redactDurableValue(call.arguments, currentCredentials(options)) as Record<string, never> };
        },
        afterTool(_call, result) {
          const credentials = currentCredentials(options);
          const output = result.output?.map((part: any) => part.type === "text" ? { ...part, text: redactDurableValue(part.text, credentials) as string } : part);
          return { ...result, ...(output ? { output } : {}), ...(result.details !== undefined ? { details: redactDurableValue(result.details, credentials) as never } : {}) };
        },
      }),
      hook(GenerationTask, {
        async beforeRequest(request) {
          // GenerationTask includes resumed work. Check the authoritative lease
          // immediately before every provider dispatch, not just on Send.
          await options.assertExecution?.();
          const messages = request.messages.map((message: any) => ({ ...message, content: Array.isArray(message.content) ? message.content.filter((part: any) => part.type !== "thinking" && part.type !== "redacted_thinking") : message.content }));
          return { messages };
        },
        afterResponse(message: any) {
          if (Array.isArray(message.content)) message.content = message.content.filter((part: any) => part.type !== "thinking" && part.type !== "redacted_thinking");
        },
      }),
    ],
  });
  const registry = createRegistry();
  registry.install(extension);
  const storage = options.openStorage ? await options.openStorage() : options.storage ?? await openDurableStorage({ projectId, conversationId: "local-only", localOnly: true });
  const harnessModels = modelsWithConfiguredModel(durableModels, options.sessionConfig, options.keyStore, options.onModelEvent);
  const harness = await Harness.open(storage, { models: () => harnessModels, registry, settings: { toolExecution: "sequential" } }, BACKGROUND_CONTEXT);
  const conversation = await harness.root(BACKGROUND_CONTEXT, {
    agent: {
      model: { provider: options.sessionConfig.provider, modelId: model.id },
      thinkingLevel: "off",
      extensions: [extension],
      instructions: `${buildSystemPrompt(options.project)}${options.sessionConfig.systemAppend ? `\n\n${options.sessionConfig.systemAppend}` : ""}`,
    },
  });
  let pendingConfiguration = Promise.resolve();
  if (options.recoveredFiles) {
    const result = await validator.validate(
      workspace.serialize().files,
      await readLiveDatabase({ liveDatabase: options.liveDatabase }),
      await readValidationFacts({ releaseFacts: options.releaseFacts, sourceFiles: options.sourceFiles ?? (() => options.project) }),
    );
    options.onValidation?.(result);
  }
  return {
    conversation,
    harness,
    async send(text: string, requestId = crypto.randomUUID()) {
      await pendingConfiguration;
      const submitted = await conversation.submit({ requestId, type: "input", content: redactDurableValue(text, currentCredentials(options)) as string, entry: { kind: "konae.owner-message" } }, BACKGROUND_CONTEXT);
      const settled = await submitted.wait(BACKGROUND_CONTEXT);
      const changedFiles = workspace.changedFiles();
      if (changedFiles.length) {
        const result = await validator.validate(
          workspace.serialize().files,
          await readLiveDatabase({ liveDatabase: options.liveDatabase }),
          await readValidationFacts({ releaseFacts: options.releaseFacts, sourceFiles: options.sourceFiles ?? (() => options.project) }),
        );
        options.onValidation?.(result);
      }
      if (options.storage || options.openStorage) await appendDurableEntry(conversation, { kind: "konae.checkpoint" });
      return settled;
    },
    async interrupt() { await conversation.abort(BACKGROUND_CONTEXT); },
    async close() { await harness.close(BACKGROUND_CONTEXT); },
    async recordDisposition(status: import("./durable-checkpoints").CheckpointStatus, files: CapsuleFiles, saved?: { revision: number; project: MiniAppProject }) {
      const { proposalDisposition } = await import("./durable-checkpoints");
      if (status === "discarded") {
        options.onCheckpointStatus?.(status, textFiles(options.project));
      } else {
        options.onCheckpointStatus?.(status, files, saved);
      }
      try {
        await appendDurableEntry(conversation, { kind: "konae.proposal-disposition", content: proposalDisposition(status, { files }) });
      } catch (error) {
        if (status === "discarded") options.onCheckpointStatus?.("proposal", files);
        throw error;
      }
      if (status === "discarded") {
        workspace.reset();
        options.onMutation?.(workspace.serialize().files);
        options.onValidation?.({ ok: false, errors: [], checked: [] });
      }
    },
    reseed(project: MiniAppProject, config: AgentSessionConfig = options.sessionConfig) {
      workspace.reseed(textFiles(project));
      const configured = modelsWithConfiguredModel(durableModels, config, options.keyStore);
      const configuredModel = durableModelFor(config, configured);
      pendingConfiguration = conversation.configure({ model: { provider: config.provider, modelId: configuredModel.id }, thinkingLevel: "off", instructions: `${buildSystemPrompt(project)}${config.systemAppend ? `\n\n${config.systemAppend}` : ""}` }, BACKGROUND_CONTEXT).catch((error) => { options.onModelEvent?.({ type: "error", error }); });
    },
    proposal: () => ({ files: workspace.serialize().files, changedFiles: workspace.changedFiles() }),
  };
}

/** KonaeAgent adapter used only by the explicit local Durable staging opt-in. */
export function createDurableKonaeAgent(project: MiniAppProject, sessionConfig: AgentSessionConfig, keyStore: AgentKeyStore, options: Omit<DurableEngineOptions, "project" | "sessionConfig" | "keyStore" | "localOnly">): KonaeAgent & { initialize(): Promise<void>; dispose(): void; configure(config: AgentSessionConfig): void; discard(): void; reseed(files: CapsuleFiles, encodings?: MiniAppProject["encodings"]): void; retarget(next: MiniAppProject): void; assertExecution(): Promise<void>; exportLocalHistory(): Promise<{ version: 1; createdAt: number; title: string; records: any[] }>; execution(callId: string, observesCallId?: string): AgentExecution | null; recordDisposition?(status: import("./durable-checkpoints").CheckpointStatus, files: CapsuleFiles, saved?: { revision: number; project: MiniAppProject }): Promise<void> } {
  let currentProject = project;
  const modelConfig = { ...sessionConfig };
  let engine: Awaited<ReturnType<typeof openLocalDurableEngine>> | null = null;
  let opening: Promise<Awaited<ReturnType<typeof openLocalDurableEngine>>> | null = null;
  let validation: ValidationResult | null = null;
  let validationSignature = "";
  let serial = Promise.resolve();
  const listeners = new Set<(event: AgentUiEvent) => void>();
  const emit = (event: AgentUiEvent) => listeners.forEach((listener) => listener(event));
  let runToken = 0;

  const ensure = async () => {
    if (engine) return engine;
    if (!opening) {
      const token = runToken;
      opening = openLocalDurableEngine({
        ...options,
        project: currentProject,
        sessionConfig: modelConfig,
        keyStore,
        localOnly: !(options.storage || options.openStorage),
        // Stable project/session predicate: tool bindings survive multiple turns;
        // the per-send token below only suppresses stale UI updates.
        isCurrent: () => options.isCurrent?.() !== false,
        onModelEvent: (event) => {
          if (event.type === "text_delta") emit({ type: "assistant-delta", text: event.delta });
          if (event.type === "toolcall_end") {
            const call = event.toolCall;
            emit({ type: "tool-start", id: call.id, name: call.name, detail: safeString(call.arguments) });
          }
          if (event.type === "error") emit({ type: "error", message: safeString(event.error) });
        },
        onValidation: (result) => {
          validation = result;
          validationSignature = signature(engine?.proposal().files ?? options.recoveredFiles ?? textFiles(currentProject));
          options.onValidation?.(result);
          emit({ type: "validation", result });
        },
        onMutation: (files) => options.onMutation?.(files),
      }).then(async (value) => {
        if (token !== runToken) { await value.close(); throw new Error("The current project or agent run changed. Ask again in the current project."); }
        engine = value;
        return value;
      }).finally(() => { opening = null; });
    }
    return opening;
  };

  const proposal = (): AgentProposal | null => {
    const current = engine?.proposal();
    if (!current || current.changedFiles.length === 0) return null;
    const result = signature(current.files) === validationSignature ? validation : null;
    return { ...current, validation: result };
  };
  const applyGateStatus = (): ApplyGateStatus => {
    const current = proposal();
    if (!current) return { state: "empty" };
    if (!current.validation) return { state: "unvalidated", changedFiles: current.changedFiles };
    return current.validation.ok
      ? { state: "ready", changedFiles: current.changedFiles, result: current.validation }
      : { state: "blocked", changedFiles: current.changedFiles, result: current.validation };
  };

  return {
    subscribe(callback) { listeners.add(callback); return () => listeners.delete(callback); },
    async initialize() {
      await ensure();
      if (validation) emit({ type: "validation", result: validation });
    },
    proposal,
    applyGateStatus,
    async recordDisposition(status, files, saved) {
      const run = serial.then(async () => {
        await options.assertExecution?.();
        const current = await ensure();
        await current.recordDisposition(status, files, saved);
      });
      serial = run.then(() => undefined, () => undefined);
      await run;
    },
    async assertExecution() { await options.assertExecution?.(); },
    async exportLocalHistory() {
      if (options.openStorage || options.storage || options.ownerId) throw new Error("Only a browser-local conversation can use the explicit local attachment flow.");
      const current = await ensure();
      const records: any[] = [];
      let cursor: any;
      do {
        const page = await current.conversation.entries({}, 50, cursor, BACKGROUND_CONTEXT);
        records.push(...page.items);
        cursor = page.next;
      } while (cursor);
      records.reverse();
      const context = await current.conversation.context(BACKGROUND_CONTEXT);
      const candidate = proposal();
      const candidateFiles = candidate ? { ...candidate.files } : undefined;
      const currentFiles = { ...currentProject.files };
      // A local transcript can contain task IDs or opaque details that this
      // versioned owner envelope cannot faithfully reattach to native tasks.
      // Preserve files and messages, but keep such a copy inspection-only.
      const hasTaskRecords = records.some((entry) => entry && (entry.byTaskId !== undefined || entry.kind === "task" || entry.kind === "submission"));
      const hasOpaqueDetails = records.some((entry) => JSON.stringify(entry).includes('"kind":"immutable-detail"'));
      const reasons = [
        ...(hasTaskRecords ? ["Local native task state is not preserved by this attachment format."] : []),
        ...(hasOpaqueDetails ? ["Some pre-attachment tool details are stored as opaque blobs and are not copied with this attachment."] : []),
      ];
      const exportValue = {
        version: 1 as const, createdAt: Date.now(), title: "Local browser conversation", records,
        modelContext: { messages: Array.isArray((context as any)?.messages) ? (context as any).messages : [] },
        files: { current: currentFiles, ...(currentProject.encodings ? { encodings: currentProject.encodings } : {}), ...(candidateFiles ? { candidate: candidateFiles, ...(currentProject.encodings ? { candidateEncodings: currentProject.encodings } : {}) } : {}), changedFiles: candidate ? [...new Set([...Object.keys(currentFiles), ...Object.keys(candidateFiles!)])].filter((path) => currentFiles[path] !== candidateFiles![path]).sort() : [] },
        continuation: { eligible: reasons.length === 0, reasons },
      };
      const encoded = JSON.stringify(exportValue);
      if (new TextEncoder().encode(encoded).byteLength > 4 * 1024 * 1024) throw new Error("This local conversation is too large for a safe browser handoff. Download it or shorten it before attaching.");
      return exportValue;
    },
    execution(callId: string, observesCallId?: string) {
      const identity = options.executionIdentity?.();
      return identity ? { ...identity, callId, ...(observesCallId ? { observesCallId } : {}) } : null;
    },
    async send(prompt, requestId = crypto.randomUUID()) {
      await options.assertExecution?.();
      const token = ++runToken;
      const run = serial.then(async () => {
        validation = null;
        validationSignature = "";
        let current: Awaited<ReturnType<typeof openLocalDurableEngine>>;
        try { current = await ensure(); }
        catch (error) { if (token === runToken) { emit({ type: "error", message: safeString(error) }); emit({ type: "turn-end" }); } return; }
        if (token !== runToken) return;
        const before = new Set<string>();
        try {
          const page = await current.conversation.entries({}, 50, undefined, BACKGROUND_CONTEXT);
          for (const entry of page.items) before.add(String(entry.id));
        } catch { /* a new conversation has no entries yet */ }
        try {
          const settled = await current.send(prompt, requestId);
          const page = await current.conversation.entries({}, 50, undefined, BACKGROUND_CONTEXT);
          for (const entry of page.items) {
            if (before.has(String(entry.id)) || !ToolResultEntry.is(entry)) continue;
            const message = entry.model?.[0] as any;
            if (message?.role !== "toolResult") continue;
            const detail = Array.isArray(message.content) ? message.content.filter((part: any) => part.type === "text").map((part: any) => part.text).join("\n") : safeString(message.content);
            emit({ type: "tool-end", id: message.toolCallId, name: message.toolName, ok: !message.isError, detail });
          }
          if (settled.status === "unanswered") emit({ type: "error", message: safeString(settled.reason) });
        } catch (error) { emit({ type: "error", message: safeString(error) }); }
        if (token === runToken) emit({ type: "turn-end" });
      });
      serial = run.then(() => undefined, () => undefined);
      await run;
    },
    interrupt() {
      runToken += 1;
      if (engine) {
        const stopped = engine;
        void stopped.interrupt().then(async () => {
          if (options.openStorage && engine === stopped) {
            await stopped.close();
            engine = null;
          }
          emit({ type: "turn-end" });
        }).catch((error) => emit({ type: "error", message: safeString(error) }));
      }
    },
    configure(config) {
      Object.assign(modelConfig, config);
      if (engine) void engine.conversation.configure({ model: { provider: config.provider, modelId: config.modelId }, thinkingLevel: "off", instructions: `${buildSystemPrompt(currentProject)}${config.systemAppend ? `\n\n${config.systemAppend}` : ""}` }, BACKGROUND_CONTEXT).catch((error) => emit({ type: "error", message: safeString(error) }));
    },
    discard() {
      runToken += 1;
      validation = null;
      validationSignature = "";
      const old = engine;
      engine = null;
      if (old) void old.close();
    },
    reseed(files, encodings) {
      const next = { ...currentProject, files: { ...currentProject.files, ...files }, encodings };
      if (JSON.stringify(currentProject.files) === JSON.stringify(next.files) && JSON.stringify(currentProject.encodings ?? {}) === JSON.stringify(next.encodings ?? {})) return;
      currentProject = next;
      validation = null;
      validationSignature = "";
      engine?.reseed(currentProject, modelConfig);
    },
    retarget(next) {
      const changed = currentProject.id !== next.id || JSON.stringify(currentProject.files) !== JSON.stringify(next.files) || JSON.stringify(currentProject.encodings ?? {}) !== JSON.stringify(next.encodings ?? {});
      currentProject = next;
      if (changed) this.discard();
    },
    dispose() {
      runToken += 1;
      listeners.clear();
      const old = engine;
      engine = null;
      if (old) void old.close();
    },
  };
}

function signature(files: CapsuleFiles) { return JSON.stringify(files); }

async function appendDurableEntry(conversation: any, input: { kind: string; content?: string }) {
  const submission = await conversation.submit({ type: "write", entry: { kind: input.kind, ...(input.content ? { model: [{ role: "user", content: input.content, timestamp: Date.now() }] } : {}) } }, BACKGROUND_CONTEXT);
  const settled = await submission.wait(BACKGROUND_CONTEXT);
  if (settled.status !== "done") throw new Error(`Conversation lifecycle write was not stored: ${"reason" in settled ? String(settled.reason) : settled.status}.`);
}
function safeString(value: unknown) {
  return typeof value === "string" ? value : JSON.stringify(redactDurableValue(value)) ?? String(value);
}

function durableTool(tool: any, options: DurableEngineOptions, platformBoundaries: Map<string, unknown>): ToolRegistration {
  return defineTool({
    name: tool.name,
    description: tool.description,
    parameters: tool.parameters as never,
    replay: "unsafe",
    executionMode: tool.executionMode === "sequential" ? "sequential" : undefined,
    outputLimits: { maxBytes: 80_000, retain: "head" },
    async execute(args, api, context) {
      await options.assertExecution?.();
      if (options.isCurrent?.() === false) throw new Error("The current project or agent execution changed. Continue again from the current saved conversation.");
      const knownCredentials = ["anthropic", "openai", "openrouter"].flatMap((provider) => {
        try { const value = options.keyStore.getKey(provider as AgentSessionConfig["provider"]); return value ? [value] : []; }
        catch { return []; }
      });
      const safeArgs = redactDurableValue(args, knownCredentials);
      let original: unknown;
      const effectiveArgs = safeArgs;
      let result: any;
      try {
        result = await tool.execute(api.callId, args, undefined);
        original = redactDurableValue(result, knownCredentials);
      } catch (error) {
        original = redactDurableValue(serializeError(error), knownCredentials);
        result = { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], details: original, isError: true };
      }
      const outputSource = (result.content ?? []).filter((part: any) => part.type === "text").map((part: any) => part.text).join("\n");
      const bounded = boundModelText(outputSource || result.output || "", 20_000, knownCredentials);
      const payload = {
        original: redactDurableValue(original, knownCredentials),
        effectiveArguments: redactDurableValue(platformBoundaries.get(api.callId) ?? effectiveArgs, knownCredentials),
        callId: api.callId,
        taskId: api.taskId,
        truncation: { method: bounded.method, originalLength: bounded.originalLength, retainedLength: bounded.retainedLength, truncated: bounded.truncated, redacted: bounded.redacted },
      };
      const detail = options.localOnly
        ? payload
        : await uploadDurableDetail({ projectId: options.project.id, conversationId: options.conversationId ?? String(api.conversationId), ownerId: options.ownerId }, payload, options.fetch ?? fetch);
      if (bounded.truncated || bounded.redacted) api.diagnostic({ severity: "info", code: bounded.truncated ? "konae.output_truncated" : "konae.output_redacted", message: `Stored model output ${bounded.truncated ? "was bounded" : "was redacted"}; original structured details are stored separately.` });
      return {
        output: [{ type: "text", text: bounded.text }],
        details: detail as never,
        isError: Boolean(result.isError),
      };
    },
  });
}

function serializeError(error: unknown) {
  if (!error || typeof error !== "object") return { message: String(error) };
  const value = error as Record<string, unknown>;
  const refusal = value.refusal;
  return {
    name: typeof value.name === "string" ? value.name : "Error",
    message: typeof value.message === "string" ? value.message : String(error),
    ...(refusal && typeof refusal === "object" ? { refusal: redactDurableValue(refusal) } : {}),
    ...(typeof value.code === "string" ? { code: value.code } : {}),
    ...(typeof value.stack === "string" ? { stack: value.stack } : {}),
  };
}

function emptyKeyStore(): AgentKeyStore {
  return { getKey: () => null, setKey() {}, clearKey() {} };
}

function currentCredentials(options: DurableEngineOptions) {
  return ["anthropic", "openai", "openrouter"].flatMap((provider) => {
    try { const value = options.keyStore.getKey(provider as AgentSessionConfig["provider"]); return value ? [value] : []; }
    catch { return []; }
  });
}
