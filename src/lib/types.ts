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
