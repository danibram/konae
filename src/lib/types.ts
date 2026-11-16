import type { CapsuleManifest } from "../deployments/manifest";

export type ProjectFile = string;

export type MiniAppProject = {
  id: string;
  name: string;
  slug: string;
  files: Record<ProjectFile, string>;
  encodings?: Record<ProjectFile, "base64">;
  directories: string[];
  manifest: CapsuleManifest;
  modules?: Record<string, { on: boolean; [option: string]: unknown }>;
  updatedAt: number;
};

export type AgentMessage = {
  id: string;
  role: "user" | "agent";
  content: string;
};
