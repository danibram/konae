import type { CapsuleManifest } from "../deployments/manifest";

export type ProjectFile = string;

export type MiniAppProject = {
  id: string;
  name: string;
  slug: string;
  files: Record<ProjectFile, string>;
  directories: string[];
  manifest: CapsuleManifest;
  updatedAt: number;
};

export type AgentMessage = {
  id: string;
  role: "user" | "agent";
  content: string;
};
