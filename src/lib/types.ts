import type { CapsuleManifest } from "../deployments/manifest";

export type ProjectFile = string;

export type DrizzleDatabaseModel = {
  source: string;
  acceptedSource: string;
  revision: string;
};

export type MiniAppProject = {
  id: string;
  name: string;
  slug: string;
  files: Record<ProjectFile, string>;
  directories: string[];
  databaseModel: DrizzleDatabaseModel;
  manifest: CapsuleManifest;
  updatedAt: number;
};

export type AgentMessage = {
  id: string;
  role: "user" | "agent";
  content: string;
};

export type RuntimeRequest = {
  channel: "littlebox-runtime";
  type: "request";
  requestId: string;
  method: string;
  payload?: unknown;
};

export type RuntimeResponse = {
  channel: "littlebox-runtime";
  type: "response";
  requestId: string;
  ok: boolean;
  result?: unknown;
  error?: string;
};
