import type { AgentTool, AgentToolResult, ExecutionEnv, FileError, FileSystem, Result, Shell } from "@earendil-works/pi-agent-core";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { ProjectFile } from "../lib/types";

export type { AgentTool, AgentToolResult, ExecutionEnv, FileError, FileSystem, Result, Shell };
export type { Api, Model };

/** The five files of a Capsule, exactly as `MiniAppProject.files`. */
export type CapsuleFiles = Record<ProjectFile, string>;

// --- Providers and key custody -------------------------------------------------

export type ProviderId = "anthropic" | "openai" | "openrouter";

export type ProviderModelOption = {
  id: string;
  label: string;
};

export type ProviderConfig = {
  id: ProviderId;
  label: string;
  /** False only for providers reached through a Kernel proxy that owns the key. */
  needsKey: boolean;
  defaultModels: ProviderModelOption[];
  supportsCustomModelId: boolean;
};

/**
 * Browser-local custody of provider keys. Keys live in localStorage under a
 * dedicated namespace and are never written into a Project, a Capsule file or
 * a Deployment.
 */
export interface AgentKeyStore {
  getKey(provider: ProviderId): string | null;
  setKey(provider: ProviderId, key: string): void;
  clearKey(provider: ProviderId): void;
}

// --- Agent Workspace ----------------------------------------------------------

export type CapsuleMutation = {
  file: ProjectFile;
  /** Snapshot of every file after the mutation. */
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
};

export type CapsuleSnapshot = {
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
};

/**
 * The Agent Workspace: a disposable copy of a Project's five files exposed to
 * the agent as an `ExecutionEnv`. `Shell.exec` is not implemented.
 */
export interface CapsuleWorkspace {
  /** Immutable snapshot; the reference changes on every mutation. */
  readonly files: CapsuleFiles;
  readonly env: ExecutionEnv;
  /** Subscribe to workspace writes for live diff rendering. Returns an unsubscribe. */
  onMutation(callback: (mutation: CapsuleMutation) => void): () => void;
  /** Files written since the workspace was created, in first-write order. */
  changedFiles(): ProjectFile[];
  serialize(): CapsuleSnapshot;
}

// --- Validation Gate ----------------------------------------------------------

export type ValidationStage = "schema" | "migration" | "actions";

export type ValidationError = {
  stage: ValidationStage;
  message: string;
  detail?: string;
};

export type ValidationResult = {
  ok: boolean;
  errors: ValidationError[];
  /**
   * The stages that actually ran. A verdict may only be reported as far as this
   * reaches: a stage that was skipped never held, it was never asked.
   */
  checked: ValidationStage[];
};

/**
 * The Validation Gate. Runs entirely in the browser on sql.js: `schema.sql`
 * must execute, a schema change must come with a migration that converges
 * against a clone of the current database, and every Server Action statement
 * must prepare against the migrated schema.
 */
export interface CapsuleValidator {
  validate(files: CapsuleFiles, live?: LiveDatabase): Promise<ValidationResult>;
}

/**
 * What the gate could learn about the live local database. "Absent" and
 * "unreadable" are different answers: a Project with no database yet has nothing
 * to migrate, while a database the gate cannot open is one whose convergence
 * stays unknown.
 */
export type LiveDatabase =
  | { state: "absent" }
  | { state: "loaded"; bytes: Uint8Array }
  | { state: "unreadable"; reason: string };

/**
 * Reads the live local database so the gate can run its `migration` stage. It
 * resolves to undefined when there is no database yet.
 */
export type LiveDatabaseExport = () => Promise<Uint8Array | undefined>;

// --- Agent session ------------------------------------------------------------

export type AgentSessionConfig = {
  provider: ProviderId;
  modelId: string;
  /** Project-specific instructions appended after the cached system prompt. */
  systemAppend?: string;
};

export type AgentUiEvent =
  | { type: "assistant-delta"; text: string }
  /** `id` is the provider's tool call id: two concurrent calls of one tool are distinguishable. */
  | { type: "tool-start"; id: string; name: string; detail: string }
  | { type: "tool-end"; id: string; name: string; ok: boolean; detail: string }
  | { type: "turn-end" }
  | { type: "error"; message: string }
  | { type: "validation"; result: ValidationResult };

export type AgentProposal = {
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
  /** Null until the gate has run against this proposal. */
  validation: ValidationResult | null;
};

/**
 * Whether the current proposal may be applied to the Project. Apply is allowed
 * only in state "ready"; the gate never trusts model behaviour for this.
 */
export type ApplyGateStatus =
  | { state: "empty" }
  | { state: "unvalidated"; changedFiles: ProjectFile[] }
  | { state: "blocked"; changedFiles: ProjectFile[]; result: ValidationResult }
  | { state: "ready"; changedFiles: ProjectFile[]; result: ValidationResult };

export interface LittleboxAgent {
  /** Resolves when the run settles; failures arrive as an "error" event, not a rejection. */
  send(prompt: string): Promise<void>;
  interrupt(): void;
  /** Returns an unsubscribe. */
  subscribe(callback: (event: AgentUiEvent) => void): () => void;
  proposal(): AgentProposal | null;
  applyGateStatus(): ApplyGateStatus;
}
