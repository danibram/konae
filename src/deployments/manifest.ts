import { checkConnects } from "../kernel/calls";
export type DeploymentVisibility = "public" | "private";
export type RuntimeActionAccess = "anonymous" | "authenticated" | "granted";
export type RuntimeActionKind = "query" | "mutation" | "signal" | "call";
export type RuntimeActionDeclaration = { name: string; kind: RuntimeActionKind; access: RuntimeActionAccess };

export type CapsuleManifest = {
  version: 1;
  visibility: DeploymentVisibility;
  routing?: "pages" | "spa";
  connects?: string[];
};

export type DeploymentRecord = {
  id: string;
  projectId: string;
  projectName: string;
  url: string;
  createdAt: number;
  releasedAt?: number;
  actions?: string;
  artifactVersion?: string;
  sourceDigest?: string;
  manifest: CapsuleManifest;
};

export const editorCapabilities = [
  "database:query",
  "database:migrate",
  "database:backup",
  "database:export",
  "files:admin",
  "deployments:manage",
] as const;

export function createDefaultManifest(): CapsuleManifest {
  return {
    version: 1,
    visibility: "private",
  };
}

export function withVisibility(manifest: CapsuleManifest, visibility: DeploymentVisibility): CapsuleManifest {
  return {
    ...manifest,
    visibility,
  };
}

export function normalizeManifest(value: unknown): CapsuleManifest {
  if (!value || typeof value !== "object") return createDefaultManifest();
  const candidate = value as Partial<CapsuleManifest>;
  const visibility = candidate.visibility === "public" ? "public" : "private";
  const connects = checkConnects(candidate.connects);
  return {
    version: 1,
    visibility,
    ...(candidate.routing === "pages" || candidate.routing === "spa" ? { routing: candidate.routing } : {}),
    ...(connects ? { connects } : {}),
  };
}
