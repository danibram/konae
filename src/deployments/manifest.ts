import { checkConnects } from "../kernel/calls";
export type DeploymentVisibility = "public" | "private";
export type RuntimeActionAccess = "anonymous" | "authenticated" | "granted";
export type RuntimeActionKind = "query" | "mutation" | "signal" | "call";
export type RuntimeActionDeclaration = { name: string; kind: RuntimeActionKind; access: RuntimeActionAccess };

export type CapsuleManifest = {
  version: 1;
  visibility: DeploymentVisibility;
  /** How the Kernel answers a path the artifact does not hold. Absent means "pages". */
  routing?: "pages" | "spa";
  /**
   * Origins the page may reach with fetch, added to its connect-src, such as
   * another app's public GET route. A key never goes to the browser; a call
   * action carries it from the Kernel. See docs/reference/routes-and-calls.md, "Calls between apps".
   */
  connects?: string[];
};

export type DeploymentRecord = {
  id: string;
  projectId: string;
  projectName: string;
  url: string;
  /** The first publish. */
  createdAt: number;
  /** The publish, import or folder start that installed the release it runs. Older Kernels omit it; read it with releasedAt(). */
  releasedAt?: number;
  /** The actions.json of the release it runs. */
  actions?: string;
  /** Names the release. A new publish names a new one. */
  artifactVersion?: string;
  /** The digest of the sources the release was built from (source-digest.ts). Older Kernels and older records omit it. */
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

/** A new project is private until its owner, or the template it was born as, makes it public. */
export function createDefaultManifest(): CapsuleManifest {
  return {
    version: 1,
    visibility: "private",
  };
}

/** The manifest with this visibility, for a project being born: a private app requires a signed-in app account and a public one leaves it optional. */
export function withVisibility(manifest: CapsuleManifest, visibility: DeploymentVisibility): CapsuleManifest {
  return {
    ...manifest,
    visibility,
  };
}

export function normalizeManifest(value: unknown): CapsuleManifest {
  if (!value || typeof value !== "object") return createDefaultManifest();
  const candidate = value as Partial<CapsuleManifest>;
  // Fail closed, as the Kernel does: only a manifest that says public is public.
  const visibility = candidate.visibility === "public" ? "public" : "private";
  const connects = checkConnects(candidate.connects);
  return {
    version: 1,
    visibility,
    ...(candidate.routing === "pages" || candidate.routing === "spa" ? { routing: candidate.routing } : {}),
    ...(connects ? { connects } : {}),
  };
}
