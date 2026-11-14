import { checkConnects } from "../kernel/calls";
export type DeploymentVisibility = "public" | "private";
export const fileAccessMessage = 'manifest.fileAccess must be "app-users" when set.';

export function checkFileAccess(value: object): void {
  if (Object.hasOwn(value, "fileAccess") && (value as { fileAccess?: unknown }).fileAccess !== "app-users") {
    throw new Error(fileAccessMessage);
  }
}
export type RuntimeActionAccess = "anonymous" | "authenticated" | "granted";
export type RuntimeActionKind = "query" | "mutation" | "signal" | "call";

export type RuntimeActionDeclaration = {
  name: string;
  kind: RuntimeActionKind;
  access: RuntimeActionAccess;
};

export type CapsuleManifest = {
  version: 1;
  visibility: DeploymentVisibility;
  /** How the Kernel answers a path the artifact does not hold. Absent means "pages". */
  routing?: "pages" | "spa";
  fileAccess?: "app-users";
  /**
   * Origins the page may reach with fetch, added to its connect-src, such as
   * another app's public GET route. A key never goes to the browser; a call
   * action carries it from the Kernel. See docs/mcp.md, "Calls between apps".
   */
  connects?: string[];
  runtime: {
    database: {
      mode: "declared-actions";
      actions: RuntimeActionDeclaration[];
    };
    auth: {
      mode: "optional" | "required";
    };
    files: {
      access: "deployment";
    };
  };
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

const starterActions: RuntimeActionDeclaration[] = [
  { name: "tasks.list", kind: "query", access: "anonymous" },
  { name: "tasks.create", kind: "mutation", access: "anonymous" },
  { name: "tasks.toggle", kind: "mutation", access: "anonymous" },
];

/** A new project is private until its owner, or the template it was born as, makes it public. */
export function createDefaultManifest(): CapsuleManifest {
  return {
    version: 1,
    visibility: "private",
    runtime: {
      database: { mode: "declared-actions", actions: starterActions.map((action) => ({ ...action })) },
      auth: { mode: "required" },
      files: { access: "deployment" },
    },
  };
}

/** The manifest with this visibility, for a project being born: a private app requires a signed-in app account and a public one leaves it optional. */
export function withVisibility(manifest: CapsuleManifest, visibility: DeploymentVisibility): CapsuleManifest {
  return {
    ...manifest,
    visibility,
    runtime: { ...manifest.runtime, auth: { mode: visibility === "private" ? "required" : "optional" } },
  };
}

export function normalizeManifest(value: unknown): CapsuleManifest {
  if (!value || typeof value !== "object") return createDefaultManifest();
  const candidate = value as Partial<CapsuleManifest>;
  // Fail closed, as the Kernel does: only a manifest that says public is public.
  const visibility = candidate.visibility === "public" ? "public" : "private";
  const actions = candidate.runtime?.database?.actions;
  const normalizedActions = Array.isArray(actions)
    ? actions.flatMap(normalizeAction)
    : starterActions.map((action) => ({ ...action }));

  assertActions(normalizedActions);
  const connects = checkConnects(candidate.connects);
  checkFileAccess(candidate);
  return {
    version: 1,
    visibility,
    ...(candidate.routing === "pages" || candidate.routing === "spa" ? { routing: candidate.routing } : {}),
    ...(connects ? { connects } : {}),
    ...(candidate.fileAccess ? { fileAccess: candidate.fileAccess } : {}),
    runtime: {
      database: { mode: "declared-actions", actions: normalizedActions },
      auth: { mode: visibility === "private" || candidate.runtime?.auth?.mode === "required" ? "required" : "optional" },
      files: { access: "deployment" },
    },
  };
}

export function createDeployment(
  project: { id: string; name: string; slug: string; manifest: CapsuleManifest },
  visibility: DeploymentVisibility,
  options: { id?: string; now?: number } = {},
): DeploymentRecord {
  const id = options.id ?? crypto.randomUUID();
  const manifest = normalizeManifest({ ...project.manifest, visibility });
  if (visibility === "private") manifest.runtime.auth.mode = "required";
  return {
    id,
    projectId: project.id,
    projectName: project.name,
    url: `https://${project.slug}-${id.replaceAll("-", "").slice(0, 4)}.konae.app`,
    createdAt: options.now ?? Date.now(),
    manifest,
  };
}

function normalizeAction(value: unknown): RuntimeActionDeclaration[] {
  if (!value || typeof value !== "object") return [];
  const action = value as Partial<RuntimeActionDeclaration>;
  if (typeof action.name !== "string" || !/^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/.test(action.name)) return [];
  if (action.kind !== "query" && action.kind !== "mutation" && action.kind !== "signal" && action.kind !== "call") return [];
  if (action.access !== "anonymous" && action.access !== "authenticated" && action.access !== "granted") return [];
  return [{ name: action.name, kind: action.kind, access: action.access }];
}

function assertActions(actions: RuntimeActionDeclaration[]) {
  if (actions.length > 50) throw new Error("A deployment may declare at most 50 runtime actions.");
  if (new Set(actions.map((action) => action.name)).size !== actions.length) {
    throw new Error("Runtime action names must be unique.");
  }
}
