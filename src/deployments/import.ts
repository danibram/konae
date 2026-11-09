import { platformMcp, type Transfer } from "../mcp/client";
import { dataManifest, sourcesPayload, type ImportedFolder } from "../lib/import-folder";
import type { SchemaChange } from "./release";

/** What an import replaces in the app. Nothing is merged. */
export type ImportReplace = "sources" | "data" | "both";

export type ImportBackup = { id: string; label: string; createdAt: number; sizeBytes: number; holds: string[] };

/**
 * `project_import`'s answer. `imported` is false when the schema change loses
 * data and needs the owner's confirmation; then nothing changed.
 */
export type ImportResult =
  | { imported: false; replace: ImportReplace; schema: SchemaChange }
  | { imported: true; replace: ImportReplace; backup: ImportBackup; schema: SchemaChange; deployment?: { id: string; url: string } };

/**
 * Imports a folder into the published app: its sources as a release, its
 * data from its export, or both. The Kernel backs up what it replaces first.
 * Call again with the plan's confirmation when the first answer asks for one.
 */
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
    const args: Record<string, unknown> = { deploymentId: input.deploymentId, replace, ...(input.confirmSchema ? { confirmSchema: input.confirmSchema } : {}) };
    if (replace !== "data") {
      const payload = sourcesPayload(folder, project, input.visibility);
      const transfer = await platformMcp.upload(new TextEncoder().encode(JSON.stringify(payload)), project.id);
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
