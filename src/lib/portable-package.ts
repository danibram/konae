// The chunked package that `package_export` returns and a release imports
// as `initialPackage`. kernel-rs/src/package.rs checks the same manifest with
// the same limits; a change here lands there too.

export const packageFormat = "konae-package";
export const packageVersion = 1;

const mib = 1024 * 1024;

/** The largest each component may be. The files archive holds at most 20 MiB of file data plus its headers and index. */
export const packageComponentLimits = {
  database: 256 * mib,
  identity: 64 * mib,
  files: 256 * mib,
  /** Usage and Signals, ADR-0010. A package has it when the Deployment recorded any. */
  usage: 128 * mib,
} as const;

export type PackageComponentName = keyof typeof packageComponentLimits;

/** Where each component sits inside an exported kit, under `konae/package/`. */
export const packageComponentFiles: Record<PackageComponentName, string> = {
  database: "database.sqlite",
  identity: "identity.sqlite",
  files: "files.tar",
  usage: "usage.sqlite",
};

export type PackageContent = { name: PackageComponentName; size: number; sha256: string };

export type PackageManifest = {
  format: typeof packageFormat;
  version: typeof packageVersion;
  packageId: string;
  createdAt: number;
  deploymentId: string;
  projectId: string;
  visibility: "public" | "private";
  signupPolicy?: { mode: "open" | "allowlist" | "closed"; allowlist: string[] };
  /** How people sign in. Absent in packages written before Google sign-in, which mean password only. */
  signInMethods?: ("password" | "google")[];
  publishedRevision: number;
  artifactVersion?: string | null;
  sourcesRevision?: number | null;
  contents: PackageContent[];
  excluded: string[];
  /** The custom domain's hostname, without its token; the destination issues a new one (ADR-0015). */
  domain?: string;
};

/** Refuses a manifest the Kernel would refuse, with the Kernel's reason. */
export function checkPackageManifest(value: unknown): PackageManifest {
  if (!value || typeof value !== "object") throw new Error("This is not a Konae package manifest.");
  const manifest = value as Partial<PackageManifest> & Record<string, unknown>;
  if (manifest.format !== packageFormat) throw new Error("This is not a Konae package manifest.");
  if (manifest.version !== packageVersion) {
    throw new Error(`Package version ${String(manifest.version)} is not supported. This Kernel reads version ${packageVersion}.`);
  }
  if (manifest.visibility !== "public" && manifest.visibility !== "private") throw new Error("The package manifest has no valid visibility.");
  if (!Array.isArray(manifest.contents)) throw new Error("The package manifest lists no database.");
  const seen = new Set<string>();
  for (const content of manifest.contents as unknown[]) {
    const { name, size, sha256 } = (content ?? {}) as Partial<PackageContent>;
    if (typeof name !== "string" || !Object.hasOwn(packageComponentLimits, name)) throw new Error(`Unknown package component ${String(name)}.`);
    if (seen.has(name)) throw new Error(`The manifest lists the ${name} component twice.`);
    seen.add(name);
    const limit = packageComponentLimits[name];
    if (!Number.isSafeInteger(size) || size! <= 0 || size! > limit) throw new Error(`The ${name} component is ${String(size)} bytes; its limit is ${limit} bytes.`);
    if (typeof sha256 !== "string" || !/^[0-9a-f]{64}$/.test(sha256)) throw new Error(`The ${name} component has no valid SHA-256 in the manifest.`);
  }
  if (!seen.has("database")) throw new Error("The package manifest lists no database.");
  return manifest as PackageManifest;
}
