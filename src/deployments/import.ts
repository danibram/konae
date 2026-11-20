import { verifyServerFolder } from "../kernel/functions";
import { platformMcp, type Transfer } from "../mcp/client";
import { dataManifest, sourcesPayload, type ImportedFolder } from "../lib/import-folder";
import { checkArtifact, checkRelease } from "../lib/size";
import { checkSourceFiles } from "../lib/source-rules";
import type { SchemaChange } from "./release";

export type ImportReplace = "sources" | "data" | "both";

export type ImportBackup = { id: string; label: string; createdAt: number; sizeBytes: number; holds: string[] };

export type ImportResult =
  | { imported: false; replace: ImportReplace; schema: SchemaChange }
  | { imported: true; replace: ImportReplace; backup: ImportBackup; schema: SchemaChange; deployment?: { id: string; url: string } };

export async function importFolder(input: {
  folder: ImportedFolder;
  replace: ImportReplace;
  deploymentId: string;
  project: { id: string; name: string; slug: string };
  visibility: "public" | "private";
  confirmSchema?: string;
}): Promise<ImportResult> {
  const { folder, replace, project } = input;
  const transfers: Transfer[] = [];
  try {
    const args: Record<string, unknown> = { projectId: input.deploymentId, replace, ...(input.confirmSchema ? { confirmSchema: input.confirmSchema } : {}) };
    if (replace !== "data") {
      checkSourceFiles(folder.sources);
      await verifyServerFolder(folder.server ?? {});
      const payload = sourcesPayload(folder, project, input.visibility);
      checkArtifact(payload.artifact);
      const bytes = new TextEncoder().encode(JSON.stringify(payload));
      checkRelease(bytes);
      const transfer = await platformMcp.upload(bytes, project.id);
      transfers.push(transfer);
      args.transferId = transfer.transferId;
    }
    if (replace !== "sources") {
      const manifest = await dataManifest(folder, project.id);
      const components: Record<string, string> = {};
      for (const { name } of manifest.contents) {
        const transfer = await platformMcp.upload(folder.data[name]!, project.id, name);
        transfers.push(transfer);
        components[name] = transfer.transferId;
      }
      args.package = { manifest, components };
    }
    return await platformMcp.call<ImportResult>("project_import", args);
  } finally {
    for (const transfer of transfers) await platformMcp.cancel(transfer.transferId).catch(() => {});
  }
}
