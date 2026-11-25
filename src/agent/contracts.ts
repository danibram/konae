import type { AgentTool, AgentToolResult, ExecutionEnv, FileError, FileSystem, Result, Shell } from "@earendil-works/pi-agent-core";
import type { RefusalCode, RefusalHint } from "../kernel/refusal";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { MiniAppProject, ProjectFile } from "../lib/types";
import type { ReleaseFacts } from "./release-check";
import type { SourceFiles } from "../lib/source-files";

export type { AgentTool, AgentToolResult, ExecutionEnv, FileError, FileSystem, Result, Shell };
export type { Api, Model };

export type CapsuleFiles = Record<ProjectFile, string>;


export type ProviderId = "anthropic" | "openai" | "openrouter";

export type ProviderModelOption = {
  id: string;
  label: string;
};

export type ProviderConfig = {
  id: ProviderId;
  label: string;
  defaultModels: ProviderModelOption[];
  supportsCustomModelId: boolean;
  keyPage?: string;
};

export interface AgentKeyStore {
  getKey(provider: ProviderId): string | null;
  setKey(provider: ProviderId, key: string): void;
  clearKey(provider: ProviderId): void;
}


export type CapsuleMutation = {
  file: ProjectFile;
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
};

export type CapsuleSnapshot = {
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
};

export interface CapsuleWorkspace {
  readonly files: CapsuleFiles;
  readonly env: ExecutionEnv;
  onMutation(callback: (mutation: CapsuleMutation) => void): () => void;
  changedFiles(): ProjectFile[];
  serialize(): CapsuleSnapshot;
  reset(): void;
  reseed(files: CapsuleFiles): void;
  /** Updates confirmed saved files as the comparison base without replacing scratch. */
  adoptBaseline(files: CapsuleFiles): void;
}


export type ValidationStage = "sources" | "schema" | "preview-rows" | "actions";

export type ValidationFacts = ReleaseFacts & {
  sourceFiles?: SourceFiles;
  serverArtifact?: unknown;
  staticPaths?: string[];
  modules?: Record<string, { on: boolean; [option: string]: unknown }>;
};

export type ValidationError = {
  stage: ValidationStage;
  message: string;
  detail?: string;
  code?: RefusalCode;
  hint?: RefusalHint;
};

export type ValidationWarning = {
  stage: ValidationStage;
  message: string;
  detail?: string;
  owner?: true;
  code?: RefusalCode;
  hint?: RefusalHint;
};

export type ValidationResult = {
  ok: boolean;
  errors: ValidationError[];
  warnings?: ValidationWarning[];
  checked: ValidationStage[];
  functionPreparation?: { status: "accepted" | "pending"; scratchRevision?: string; buildFingerprint?: string };
};

export interface CapsuleValidator {
  validate(files: CapsuleFiles, live?: LiveDatabase, facts?: ValidationFacts): Promise<ValidationResult>;
}

export type LiveDatabase =
  | { state: "absent" }
  | { state: "loaded"; bytes: Uint8Array }
  | { state: "unreadable"; reason: string };

export type LiveDatabaseExport = () => Promise<Uint8Array | undefined>;


export type AgentSessionConfig = {
  provider: ProviderId;
  modelId: string;
  systemAppend?: string;
};

/** A provider prompt was rejected before Durable admitted it to native storage. */
export class AgentSubmissionNotAdmitted extends Error {
  readonly submission = "not-admitted";
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "AgentSubmissionNotAdmitted";
  }
}

export function isAgentSubmissionNotAdmitted(error: unknown): error is AgentSubmissionNotAdmitted {
  return error instanceof AgentSubmissionNotAdmitted;
}

export type AgentUiEvent =
  | { type: "assistant-delta"; text: string }
  /** `id` is the provider's tool call id: two concurrent calls of one tool are distinguishable. */
  | { type: "tool-start"; id: string; name: string; detail: string }
  | { type: "tool-end"; id: string; name: string; ok: boolean; detail: string }
  | { type: "turn-end" }
  | { type: "error"; message: string; submission?: "not-admitted" }
  | { type: "validation"; result: ValidationResult };

export type AgentProposal = {
  files: CapsuleFiles;
  changedFiles: ProjectFile[];
  encodings?: Record<ProjectFile, "base64">;
  validation: ValidationResult | null;
};

export type ApplyGateStatus =
  | { state: "empty" }
  | { state: "unvalidated"; changedFiles: ProjectFile[] }
  | { state: "blocked"; changedFiles: ProjectFile[]; result: ValidationResult }
  | { state: "ready"; changedFiles: ProjectFile[]; result: ValidationResult };

export interface KonaeAgent {
  send(prompt: string): Promise<void>;
  interrupt(): void;
  subscribe(callback: (event: AgentUiEvent) => void): () => void;
  proposal(): AgentProposal | null;
  applyGateStatus(): ApplyGateStatus;
  /** Accepts a source baseline only after the host has read it back through its authoritative source door. */
  adoptSavedBaseline?(project: MiniAppProject, revision: number): void;
}
