import { Agent } from "@earendil-works/pi-agent-core";
import type { AgentEvent, ThinkingLevel } from "@earendil-works/pi-agent-core";
import { clampThinkingLevel, type Models } from "@earendil-works/pi-ai";
import type { MiniAppProject, ProjectFile } from "../lib/types";
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
  LittleboxAgent,
  LiveDatabase,
  LiveDatabaseExport,
  ValidationResult,
} from "./contracts";
import { createPlatformTools, type AgentPlatformBinding } from "./platform-tools";
export type { AgentPlatformBinding, PlatformConfirmation, PublishOutcome } from "./platform-tools";
import { createCapsuleValidator, type ReleaseFacts, type SqlJsLoader } from "./gate";
import { buildAuth, buildModel, createStreamFn } from "./providers";
import { buildInitialUserMessage, buildSystemPrompt } from "./system-prompt";
import { createCapsuleTools, readLiveDatabase, readReleaseFacts, type PreviewFailureReport } from "./tools";

/**
 * The concrete agent. Beyond the `LittleboxAgent` contract it exposes the four
 * operations the editor needs around a proposal: swap the model without losing
 * the conversation, throw the workspace away, re-seed it after an Apply, and
 * point the session at the Project as it now stands.
 */
export type LittleboxAgentHandle = LittleboxAgent & {
  configure(config: AgentSessionConfig): void;
  /** Throw away the Agent Workspace edits and the transcript. */
  discard(): void;
  /** Re-seed the workspace from the Project after a proposal was applied. */
  reseed(files: CapsuleFiles): void;
  /**
   * Point the session at a Project: a different one starts a new conversation,
   * the same one refreshes the system prompt and re-seeds files changed outside
   * the agent. Nothing else may assume the Project is the one it was built with.
   */
  retarget(project: MiniAppProject): void;
  dispose(): void;
};

export type LittleboxAgentOptions = {
  /** The owner's MCP session (ADR-0022). Without it the agent edits files and nothing else. */
  platform?: AgentPlatformBinding;
  /** Injected sql.js loader for the Validation Gate; defaults to the browser one. */
  loadSql?: SqlJsLoader;
  models?: Models;
  validator?: CapsuleValidator;
  /** Default reasoning effort. Edits are small and mostly visual. */
  thinkingLevel?: ThinkingLevel;
  /** Used from the turn after a change touches schema.sql or actions.json. */
  riskyThinkingLevel?: ThinkingLevel;
  /** The live local database, so the gate can run its migration stage. */
  liveDatabase?: LiveDatabaseExport;
  /** Runtime failures from the live preview, exposed to the model as `preview_errors`. */
  previewErrors?: () => PreviewFailureReport[];
  /** The project's secret names and the installation's hosts, read when the Gate runs. */
  releaseFacts?: () => Promise<ReleaseFacts>;
};

export function createLittleboxAgent(
  project: MiniAppProject,
  sessionConfig: AgentSessionConfig,
  keyStore: AgentKeyStore,
  options: LittleboxAgentOptions = {},
): LittleboxAgentHandle {
  return new LittleboxAgentSession(project, sessionConfig, keyStore, options);
}

const riskyFiles: ProjectFile[] = ["schema.sql", "actions.json"];

class LittleboxAgentSession implements LittleboxAgentHandle {
  private readonly workspace: WorkspaceHolder;
  private readonly validator: CapsuleValidator;
  private readonly agent: Agent;
  private readonly listeners = new Set<(event: AgentUiEvent) => void>();
  private readonly unsubscribes: (() => void)[] = [];
  private readonly thinkingLevel: ThinkingLevel;
  private readonly riskyThinkingLevel: ThinkingLevel;
  private project: MiniAppProject;
  private config: AgentSessionConfig;
  /** The files the workspace was seeded from, so a change made outside the agent is detectable. */
  private seedFiles: CapsuleFiles;
  /**
   * A verdict is current only for the exact file contents it was produced from,
   * whoever produced it: the model's own `validate_capsule` cannot vouch for an
   * edit that landed next to it.
   */
  private validated: { signature: string; result: ValidationResult; retry?: boolean } | null = null;
  private seeded = false;
  /** The five files as the model last saw them, so a change it did not make re-seeds the capsule. */
  private modelView: CapsuleFiles | null = null;
  private risky = false;
  private aborted = false;
  /** Bumped whenever the session is abandoned mid-run: discard, retarget, dispose. */
  private runToken = 0;
  /** A transcript clear the next send must wait for; `reset` throws while a run is active. */
  private pending: Promise<void> = Promise.resolve();
  /** Last error text emitted in this run, so one failure is not reported twice. */
  private reported = "";

  constructor(
    project: MiniAppProject,
    sessionConfig: AgentSessionConfig,
    private readonly keyStore: AgentKeyStore,
    private readonly options: LittleboxAgentOptions,
  ) {
    this.project = project;
    this.config = sessionConfig;
    this.thinkingLevel = options.thinkingLevel ?? "low";
    this.riskyThinkingLevel = options.riskyThinkingLevel ?? "medium";
    this.workspace = new WorkspaceHolder(project.files);
    this.seedFiles = { ...project.files };
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
      // Lever 6: escalate only once the change reaches the data contract.
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
    // A discard clears the transcript once the aborted run lands; never prompt across it.
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
    // Discarded or retargeted mid-run: the workspace and transcript are no longer this run's.
    if (token !== this.runToken) return;
    // A stop the owner asked for is not an error; the partial edits still get gated.
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
    this.replaceWorkspace(this.project.files);
    this.seeded = false;
    this.modelView = null;
    this.risky = false;
    this.clearTranscriptWhenIdle();
  }

  reseed(files: CapsuleFiles) {
    // The Project moved: the session's copy of it has to move too, or a later
    // discard restores the workspace from the Capsule as it was before the Apply
    // and quietly proposes the old files back over the applied ones.
    this.project = { ...this.project, files };
    this.replaceWorkspace(files);
    this.forgetSeedIfUnseen(files);
  }

  retarget(project: MiniAppProject) {
    const switched = project.id !== this.project.id;
    this.project = project;
    this.agent.state.systemPrompt = systemPromptFor(project, this.config);
    if (switched) {
      // Another Capsule entirely: nothing of this conversation may leak into it.
      this.agent.abort();
      this.runToken += 1;
      this.replaceWorkspace(project.files);
      this.seeded = false;
      this.modelView = null;
      this.risky = false;
      this.clearTranscriptWhenIdle();
      return;
    }
    if (sameFiles(project.files, this.seedFiles)) return;
    // The Project moved under the workspace (an Apply, a hand edit, a reset).
    const dropped = this.workspace.changedFiles().length > 0;
    this.replaceWorkspace(project.files);
    this.forgetSeedIfUnseen(project.files);
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

  // --- internals ---------------------------------------------------------------

  private capsuleTools() {
    return createCapsuleTools(this.workspace, this.validator, {
      onValidated: (files, result) => this.recordValidation(files, result),
      liveDatabase: this.options.liveDatabase,
      previewErrors: this.options.previewErrors,
      releaseFacts: this.options.releaseFacts,
    });
  }

  /**
   * The workspace tools, then the owner's MCP tools as the session lists them
   * now. A session that cannot list them (signed out, Kernel down) leaves the
   * agent its files; the owner signs in and the next turn gets them back.
   */
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

  /** The model only knows the capsule it was shown plus its own edits; anything else must be re-seeded. */
  private forgetSeedIfUnseen(files: CapsuleFiles) {
    if (this.modelView && sameFiles(files, this.modelView)) return;
    this.seeded = false;
    this.modelView = null;
  }

  private seedMessage(text: string): string {
    const files = this.workspace.serialize().files;
    // Set before awaiting the run: a discard during it must be able to reset this.
    this.seeded = true;
    this.modelView = files;
    return buildInitialUserMessage({ ...this.project, files }, text);
  }

  private escalated(): ThinkingLevel {
    return clampThinkingLevel(this.agent.state.model, this.riskyThinkingLevel) as ThinkingLevel;
  }

  /** Re-resolves the model so a provider or model change mid-conversation takes effect. */
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
    // Only tools mutate the workspace, so this is exactly what the model knows about.
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

  /**
   * The model's own `validate_capsule` counts as the gate — but only for the
   * files that call actually saw, which is why the tool reports them.
   */
  private recordValidation(files: CapsuleFiles, result: ValidationResult) {
    this.validated = { signature: signatureOf(files), result };
    this.emit({ type: "validation", result });
  }

  /**
   * The hard gate: Apply never trusts the model, so a settled run always ends
   * with a validation of the workspace as it actually stands.
   */
  private async runGate(token: number) {
    if (this.workspace.changedFiles().length === 0) return;
    const signature = this.signature();
    const known = this.validated;
    if (known && known.signature === signature && !known.retry) return;
    const files = this.workspace.serialize().files;
    try {
      const result = await this.validator.validate(files, await this.liveDatabase(), await readReleaseFacts({ releaseFacts: this.options.releaseFacts }));
      if (!this.stillCurrent(token, signature)) return;
      this.validated = { signature, result };
      this.emit({ type: "validation", result });
    } catch (error) {
      const message = `The Validation Gate could not run: ${describe(error)}`;
      // A gate that could not run is not a pass: record the verdict so Apply stays
      // shut and the footer says so instead of spinning forever. `retry` lets the
      // next settled run try again on the same files.
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

/**
 * A `CapsuleWorkspace` whose backing workspace can be replaced, so the tools and
 * the transcript survive an Apply or a Discard without being rebuilt.
 */
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

/**
 * Identity of a Capsule's contents: a validation is only current for one of
 * these. Every file counts, nested source files included, and a missing file
 * differs from an empty one.
 */
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

/** Tool chip detail: the Capsule file the call is about, if it names one. */
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
