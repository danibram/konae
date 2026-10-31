import type { BuildArtifact } from "../runtime/artifact";
import type { PortableProjectState } from "../deployments/client";
import type { MiniAppProject } from "./types";
import { buildProjectExport } from "./export-project-kit";

export async function createProjectExport(
  project: MiniAppProject,
  artifact: BuildArtifact,
  state?: Uint8Array | PortableProjectState,
): Promise<{ blob: Blob; filename: string }> {
  const { kernelFiles } = await import("./export-project-sources");
  const portableState = state instanceof Uint8Array ? { initialDatabase: state } : state;
  const files = buildProjectExport(project, artifact, kernelFiles, portableState);
  const archive = new ArrayBuffer(files.archive.byteLength);
  new Uint8Array(archive).set(files.archive);
  return {
    blob: new Blob([archive], { type: "application/x-tar" }),
    filename: `${safeFilename(project.slug)}-selfhost.tar`,
  };
}

function safeFilename(value: string) {
  const filename = value.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, "");
  return filename || "konae-app";
}
