import type { CapsuleManifest } from "../deployments/manifest";

export type ProjectFile = string;

export type MiniAppProject = {
  id: string;
  name: string;
  slug: string;
  files: Record<ProjectFile, string>;
  /** The files that are not UTF-8 text, whose value in `files` is base64 (src/lib/source-files.ts). */
  encodings?: Record<ProjectFile, "base64">;
  directories: string[];
  manifest: CapsuleManifest;
  updatedAt: number;
};

export type AgentMessage = {
  id: string;
  role: "user" | "agent";
  content: string;
};
