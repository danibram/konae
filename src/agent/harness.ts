import { Agent } from "@earendil-works/pi-agent-core";
import type { AgentEvent, ThinkingLevel } from "@earendil-works/pi-agent-core";
import { clampThinkingLevel, type Models } from "@earendil-works/pi-ai";
import type { MiniAppProject, ProjectFile } from "../lib/types";
import { isEncoded, textFiles } from "../lib/source-files";
import { capsuleFileFor, createCapsuleWorkspace } from "./capsule-env";
import type {
  AgentKeyStore,
  AgentProposal,
  AgentSessionConfig,
  AgentUiEvent,
  ApplyGateStatus,
  CapsuleFiles,
  CapsuleMutation,
  CapsuleValidator,
  CapsuleWorkspace,
  KonaeAgent,
  LiveDatabase,
  LiveDatabaseExport,
  ValidationResult,
} from "./contracts";
import { createPlatformTools, type AgentPlatformBinding } from "./platform-tools";
export type { AgentPlatformBinding, PlatformConfirmation, PublishOutcome } from "./platform-tools";
import { createCapsuleValidator, type ReleaseFacts, type SqlJsLoader } from "./gate";
import { buildAuth, buildModel, createStreamFn } from "./providers";
import { buildInitialUserMessage, buildSystemPrompt } from "./system-prompt";
import { createCapsuleTools, readLiveDatabase, readValidationFacts, type PreviewFailureReport } from "./tools";

export type KonaeAgentHandle = KonaeAgent & {
  configure(config: AgentSessionConfig): void;
  discard(): void;
  reseed(files: CapsuleFiles, encodings?: MiniAppProject["encodings"]): void;
  retarget(project: MiniAppProject): void;
  dispose(): void;
  initialize?(): Promise<void>;
  assertExecution?(): Promise<void>;
  execution?(callId: string, observesCallId?: string): import("../mcp/client").AgentExecution | null;
  recordDisposition?(status: import("./durable-checkpoints").CheckpointStatus, files: CapsuleFiles, saved?: { revision: number; project: MiniAppProject }): Promise<void>;
};

// Durable 1.5.2 candidate entry points live behind the same lazy editor-agent
// boundary. Shared writes are intentionally gated until the follow-up protocol work.
export { createDurableKonaeAgent, openDurableHistory, openLocalDurableEngine, SHARED_DURABLE_ACTIVATION } from "./durable-engine";

export type KonaeAgentOptions = {
  platform?: AgentPlatformBinding;
  loadSql?: SqlJsLoader;
  models?: Models;
  validator?: CapsuleValidator;
  thinkingLevel?: ThinkingLevel;
  riskyThinkingLevel?: ThinkingLevel;
  liveDatabase?: LiveDatabaseExport;
  previewErrors?: () => PreviewFailureReport[];
  releaseFacts?: () => Promise<ReleaseFacts>;
  sourceFiles?: () => MiniAppProject;
};

export function createKonaeAgent(
  project: MiniAppProject,
  sessionConfig: AgentSessionConfig,
  keyStore: AgentKeyStore,
  options: KonaeAgentOptions = {},
): KonaeAgentHandle {
  return new KonaeAgentSession(project, sessionConfig, keyStore, options);
}

const riskyFiles: ProjectFile[] = ["schema.sql", "actions.json"];

class KonaeAgentSession implements KonaeAgentHandle {
  private readonly workspace: WorkspaceHolder;
  private readonly validator: CapsuleValidator;
  private readonly agent: Agent;
  private readonly listeners = new Set<(event: AgentUiEvent) => void>();
  private readonly unsubscribes: (() => void)[] = [];
  private readonly thinkingLevel: ThinkingLevel;
  private readonly riskyThinkingLevel: ThinkingLevel;
  private project: MiniAppProject;
  private config: AgentSessionConfig;
  private seedFiles: CapsuleFiles;
  private validated: { signature: string; result: ValidationResult; retry?: boolean } | null = null;
  private seeded = false;
  private modelView: CapsuleFiles | null = null;
  private risky = false;
  private aborted = false;
  private runToken = 0;
  private pending: Promise<void> = Promise.resolve();
  private reported = "";

  constructor(
    project: MiniAppProject,
    sessionConfig: AgentSessionConfig,
    private readonly keyStore: AgentKeyStore,
    private readonly options: KonaeAgentOptions,
  ) {
    this.project = project;
    this.config = sessionConfig;
    this.thinkingLevel = options.thinkingLevel ?? "low";
    this.riskyThinkingLevel = options.riskyThinkingLevel ?? "medium";
    this.workspace = new WorkspaceHolder(textFiles(project));
    this.seedFiles = { ...textFiles(project) };
    this.validator = options.validator ?? createCapsuleValidator(options.loadSql);
    this.unsubscribes.push(this.workspace.onMutation((mutation) => this.onMutation(mutation)));

    const auth = buildAuth(sessionConfig.provider, keyStore);
    const model = buildModel(sessionConfig, options.models);
    this.agent = new Agent({
      streamFn: createStreamFn(auth, options.models),
      getApiKey: auth.getApiKey,
      initialState: {
        systemPrompt: systemPromptFor(project, sessionConfig),
        model,
        thinkingLevel: clampThinkingLevel(model, this.thinkingLevel) as ThinkingLevel,
        tools: this.capsuleTools(),
      },
      prepareNextTurn: () => (this.risky ? { thinkingLevel: this.escalated() } : undefined),
    });
    this.unsubscribes.push(this.agent.subscribe((event) => this.onAgentEvent(event)));
  }

  subscribe(callback: (event: AgentUiEvent) => void) {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  async send(prompt: string): Promise<void> {
    const text = prompt.trim();
    if (!text) return;
    await this.pending;
    this.risky = false;
    this.aborted = false;
    this.reported = "";
    if (!this.applyConfig()) return;
    const token = ++this.runToken;
    const tools = await this.toolsForRun(token);
    if (token !== this.runToken) return;
    this.agent.state.tools = tools;
    try {
      await this.agent.prompt(this.seeded ? text : this.seedMessage(text));
    } catch (error) {
      this.report(describe(error));
      return;
    }
    if (token !== this.runToken) return;
    const failure = this.agent.state.errorMessage;
    if (failure && !this.aborted) this.report(failure);
    await this.runGate(token);
  }

  interrupt() {
    this.agent.abort();
    void this.agent.waitForIdle();
  }

  proposal(): AgentProposal | null {
    const changedFiles = this.workspace.changedFiles();
    if (changedFiles.length === 0) return null;
    return { files: this.workspace.serialize().files, changedFiles, validation: this.currentValidation() };
  }

  applyGateStatus(): ApplyGateStatus {
    const changedFiles = this.workspace.changedFiles();
    if (changedFiles.length === 0) return { state: "empty" };
    const result = this.currentValidation();
    if (!result) return { state: "unvalidated", changedFiles };
    return result.ok ? { state: "ready", changedFiles, result } : { state: "blocked", changedFiles, result };
  }

  configure(config: AgentSessionConfig) {
    this.config = config;
    const auth = buildAuth(config.provider, this.keyStore);
    this.agent.streamFunction = createStreamFn(auth, this.options.models);
    this.agent.getApiKey = auth.getApiKey;
    this.agent.state.systemPrompt = systemPromptFor(this.project, config);
    try {
      this.agent.state.model = buildModel(config, this.options.models);
    } catch {
      // A half-typed custom model id is not an error yet; send() reports it.
    }
  }

  discard() {
    this.agent.abort();
    this.runToken += 1;
    this.replaceWorkspace(textFiles(this.project));
    this.seeded = false;
    this.modelView = null;
    this.risky = false;
    this.clearTranscriptWhenIdle();
  }

  reseed(files: CapsuleFiles, encodings?: MiniAppProject["encodings"]) {
    this.project = { ...this.project, files, encodings };
    const text = textFiles(this.project);
    this.replaceWorkspace(text);
    this.forgetSeedIfUnseen(text);
  }

  retarget(project: MiniAppProject) {
    const switched = project.id !== this.project.id;
    this.project = project;
    this.agent.state.systemPrompt = systemPromptFor(project, this.config);
    const text = textFiles(project);
    if (switched) {
      this.agent.abort();
      this.runToken += 1;
      this.replaceWorkspace(text);
      this.seeded = false;
      this.modelView = null;
      this.risky = false;
      this.clearTranscriptWhenIdle();
      return;
    }
    if (sameFiles(text, this.seedFiles)) return;
    const dropped = this.workspace.changedFiles().length > 0;
    this.replaceWorkspace(text);
    this.forgetSeedIfUnseen(text);
    if (dropped) {
      this.report("The Capsule changed outside the agent, so the pending proposal was dropped. Ask again to redo it against the current files.");
    }
  }

  dispose() {
    this.agent.abort();
    this.runToken += 1;
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.listeners.clear();
  }


  private capsuleTools() {
    return createCapsuleTools(this.workspace, this.validator, {
      onValidated: (files, result) => this.recordValidation(files, result),
      liveDatabase: this.options.liveDatabase,
      previewErrors: this.options.previewErrors,
      releaseFacts: this.options.releaseFacts,
      sourceFiles: this.options.sourceFiles,
    });
  }

  private async toolsForRun(token: number) {
    const tools = this.capsuleTools();
    const binding = this.options.platform;
    if (!binding) return tools;
    const projectId = this.project.id;
    try {
      return [...tools, ...await createPlatformTools({
        projectId,
        isCurrent: () => this.project.id === projectId && this.runToken === token,
        binding,
      })];
    } catch {
      return tools;
    }
  }

  private clearTranscriptWhenIdle() {
    this.pending = this.agent.waitForIdle().then(
      () => this.clearTranscript(),
      () => this.clearTranscript(),
    );
  }

  private clearTranscript() {
    try {
      this.agent.reset();
    } catch {
      // A new run started in the meantime; its transcript is the one that matters.
    }
  }

  private replaceWorkspace(files: CapsuleFiles) {
    this.workspace.replace(files);
    this.seedFiles = { ...files };
    this.validated = null;
  }

  private forgetSeedIfUnseen(files: CapsuleFiles) {
    if (this.modelView && sameFiles(files, this.modelView)) return;
    this.seeded = false;
    this.modelView = null;
  }

  private seedMessage(text: string): string {
    const files = this.workspace.serialize().files;
    this.seeded = true;
    this.modelView = files;
    return buildInitialUserMessage({ ...this.project, files }, text, Object.keys(this.project.files).filter((path) => isEncoded(this.project, path)));
  }

  private escalated(): ThinkingLevel {
    return clampThinkingLevel(this.agent.state.model, this.riskyThinkingLevel) as ThinkingLevel;
  }

  private applyConfig(): boolean {
    try {
      const model = buildModel(this.config, this.options.models);
      this.agent.state.model = model;
      this.agent.state.thinkingLevel = clampThinkingLevel(model, this.thinkingLevel) as ThinkingLevel;
      return true;
    } catch (error) {
      this.report(describe(error));
      return false;
    }
  }

  private onMutation(mutation: CapsuleMutation) {
    this.modelView = mutation.files;
    if (riskyFiles.includes(mutation.file)) this.risky = true;
  }

  private onAgentEvent(event: AgentEvent) {
    switch (event.type) {
      case "message_update": {
        const inner = event.assistantMessageEvent;
        if (inner.type === "text_delta") this.emit({ type: "assistant-delta", text: inner.delta });
        return;
      }
      case "tool_execution_start":
        this.emit({ type: "tool-start", id: event.toolCallId, name: event.toolName, detail: describeArgs(event.args) });
        return;
      case "tool_execution_end": {
        this.emit({
          type: "tool-end",
          id: event.toolCallId,
          name: event.toolName,
          ok: !event.isError,
          detail: resultText(event.result),
        });
        return;
      }
      case "turn_end": {
        const message = event.message;
        if (message.role === "assistant" && message.stopReason === "aborted") this.aborted = true;
        if (message.role === "assistant" && message.stopReason === "error") {
          this.report(message.errorMessage ?? "The model request failed.");
        }
        this.emit({ type: "turn-end" });
        return;
      }
      default:
        return;
    }
  }

  private recordValidation(files: CapsuleFiles, result: ValidationResult) {
    this.validated = { signature: signatureOf(files), result };
    this.emit({ type: "validation", result });
  }

  private async runGate(token: number) {
    if (this.workspace.changedFiles().length === 0) return;
    const signature = this.signature();
    const known = this.validated;
    if (known && known.signature === signature && !known.retry) return;
    const files = this.workspace.serialize().files;
    try {
      const result = await this.validator.validate(files, await this.liveDatabase(), await readValidationFacts({ releaseFacts: this.options.releaseFacts, sourceFiles: this.options.sourceFiles }));
      if (!this.stillCurrent(token, signature)) return;
      this.validated = { signature, result };
      this.emit({ type: "validation", result });
    } catch (error) {
      const message = `The Validation Gate could not run: ${describe(error)}`;
      if (this.stillCurrent(token, signature)) {
        const result: ValidationResult = { ok: false, errors: [{ stage: "schema", message }], checked: [] };
        this.validated = { signature, result, retry: true };
        this.emit({ type: "validation", result });
      }
      this.report(message);
    }
  }

  private stillCurrent(token: number, signature: string): boolean {
    return token === this.runToken && signature === this.signature();
  }

  private liveDatabase(): Promise<LiveDatabase> {
    return readLiveDatabase({ liveDatabase: this.options.liveDatabase });
  }

  private currentValidation(): ValidationResult | null {
    return this.validated && this.validated.signature === this.signature() ? this.validated.result : null;
  }

  private signature(): string {
    return signatureOf(this.workspace.files);
  }

  private report(message: string) {
    if (!message || message === this.reported) return;
    this.reported = message;
    this.emit({ type: "error", message });
  }

  private emit(event: AgentUiEvent) {
    for (const listener of this.listeners) listener(event);
  }
}

class WorkspaceHolder implements CapsuleWorkspace {
  private workspace: CapsuleWorkspace;
  private unsubscribe: () => void;
  private readonly listeners = new Set<(mutation: CapsuleMutation) => void>();

  constructor(seed: CapsuleFiles) {
    this.workspace = createCapsuleWorkspace(seed);
    this.unsubscribe = this.forward();
  }

  replace(seed: CapsuleFiles) {
    this.unsubscribe();
    this.workspace = createCapsuleWorkspace(seed);
    this.unsubscribe = this.forward();
  }

  get files() {
    return this.workspace.files;
  }

  get env() {
    return this.workspace.env;
  }

  onMutation(callback: (mutation: CapsuleMutation) => void) {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  changedFiles() {
    return this.workspace.changedFiles();
  }

  serialize() {
    return this.workspace.serialize();
  }

  reset() {
    this.workspace.reset();
  }

  reseed(files: CapsuleFiles) {
    this.workspace.reseed(files);
  }

  private forward() {
    return this.workspace.onMutation((mutation) => {
      for (const listener of this.listeners) listener(mutation);
    });
  }
}

function systemPromptFor(project: MiniAppProject, config: AgentSessionConfig): string {
  const append = config.systemAppend?.trim();
  return append ? `${buildSystemPrompt(project)}\n\n# Project instructions\n\n${append}` : buildSystemPrompt(project);
}

function signatureOf(files: CapsuleFiles): string {
  return JSON.stringify(Object.keys(files).sort().map((file) => [file, files[file]]));
}

function sameFiles(left: CapsuleFiles, right: CapsuleFiles): boolean {
  const names = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const file of names) {
    if (Object.hasOwn(left, file) !== Object.hasOwn(right, file) || left[file] !== right[file]) return false;
  }
  return true;
}

function describeArgs(args: unknown): string {
  const path = (args as { path?: unknown } | undefined)?.path;
  if (typeof path !== "string") return "";
  return capsuleFileFor(path) ?? path;
}

function resultText(result: unknown): string {
  const content = (result as { content?: unknown } | undefined)?.content;
  if (!Array.isArray(content)) return "";
  const first = content.find((block) => (block as { type?: string }).type === "text") as { text?: string } | undefined;
  return firstLine(first?.text ?? "");
}

function firstLine(text: string): string {
  const line = text.split("\n", 1)[0].trim();
  return line.length > 120 ? `${line.slice(0, 117)}…` : line;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
