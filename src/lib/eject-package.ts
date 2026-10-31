import { checksum, type Transfer } from "../mcp/client";
import { normalizeProjectPath } from "./project-files";

/** What `eject_plan` answers. kernel-rs/src/eject.rs builds it. */
export type EjectPlan = {
  planId: string;
  projectId: string;
  expectedRevision: number;
  expiresAt: number | null;
  applied: boolean;
  package: { format: "konae-eject"; size: number; sha256: string; contents: { path: string; size: number; sha256: string }[] };
  effects: Record<"code" | "hostedApp" | "result" | "returning", string>;
  transfer: Transfer;
  receipt?: EjectReceipt;
};

export type EjectReceipt = {
  planId: string;
  projectId: string;
  sourceAuthority: "external";
  sourcesRevision: number;
  packageSha256: string;
  ejectedAt: number;
  ejectedBy: string;
  via: "session" | "token";
  deployment: { id: string; url: string } | null;
  publish: string[];
};

export type EjectFiles = { files: Record<string, string>; directories: string[] };

/**
 * Checks a downloaded code package against the plan before anything is
 * written or the project changes: the whole package's size and SHA-256, the
 * content list, every file's SHA-256, and that every path stays inside the
 * folder it will be written to.
 */
export async function checkEjectPackage(bytes: Uint8Array, plan: Pick<EjectPlan, "projectId" | "expectedRevision" | "package">): Promise<EjectFiles> {
  if (bytes.byteLength !== plan.package.size || await checksum(bytes) !== plan.package.sha256) throw Error("The code package does not match its plan. Nothing was changed.");
  const body = JSON.parse(new TextDecoder().decode(bytes)) as { format?: unknown; version?: unknown; projectId?: unknown; sourcesRevision?: unknown; contents?: unknown; directories?: unknown; files?: unknown };
  if (body.format !== "konae-eject" || body.version !== 1 || body.projectId !== plan.projectId || body.sourcesRevision !== plan.expectedRevision) throw Error("The code package names another project or revision.");
  if (!body.files || typeof body.files !== "object" || !Array.isArray(body.contents) || !Array.isArray(body.directories)) throw Error("The code package is malformed.");
  const files = body.files as Record<string, unknown>;
  const listed = body.contents as { path?: unknown; size?: unknown; sha256?: unknown }[];
  if (listed.length !== Object.keys(files).length) throw Error("The code package's content list does not match its files.");
  const encoder = new TextEncoder();
  const checked: Record<string, string> = {};
  for (const entry of listed) {
    const path = entry.path;
    const text = typeof path === "string" ? files[path] : undefined;
    if (typeof path !== "string" || typeof text !== "string" || normalizeProjectPath(path) !== path) throw Error(`The code package lists an invalid file: ${String(path)}`);
    const data = encoder.encode(text);
    if (data.byteLength !== entry.size || await checksum(data) !== entry.sha256) throw Error(`${path} does not match the code package's content list.`);
    checked[path] = text;
  }
  const directories = (body.directories as unknown[]).filter((value): value is string => typeof value === "string" && normalizeProjectPath(value) === value);
  return { files: checked, directories };
}
