import { hostedRuntime, type BuildArtifact } from "../runtime/artifact";
import staticArtifactSource from "./static-artifact.js?raw";
import { platformMcp, decodeBase64, type Transfer } from "../mcp/client";
import { createTar, readTar } from "./tar";
import type { MiniAppProject } from "./types";
import type { PackageComponentName } from "./portable-package";
import {
  DIST_FOLDER, KERNEL_VERSION, SETTINGS_FILE, SOURCES_FOLDER, dataEntries, generatedFiles,
  type AppSettings, type FolderEntry,
} from "./app-folder";

type Exported = { manifest: { contents: { name: PackageComponentName }[] }; downloads: Record<string, Transfer>; folder: Transfer; sourcesAhead: boolean };

/**
 * The app folder as one tar. A published app comes from project_export:
 * konae.json, app/ and dist/ as the Kernel keeps them, and data/. An app not
 * published yet takes its sources and a build of them, and has no data. A
 * published app's export never builds the editor's files, so a draft that does
 * not build cannot keep its accounts and rows from leaving.
 */
export async function createProjectExport(
  project: MiniAppProject,
  deploymentId?: string,
  door: { mcp: Pick<typeof platformMcp, "call" | "download" | "cancel">; build: (project: MiniAppProject) => Promise<BuildArtifact> } = { mcp: platformMcp, build: buildDraft },
): Promise<{ blob: Blob; filename: string; sourcesAhead: boolean }> {
  let entries: FolderEntry[];
  let settings: AppSettings;
  let sourcesAhead = false;
  if (deploymentId) {
    const exported = await door.mcp.call<Exported>("project_export", { deploymentId });
    try {
      entries = readTar(await door.mcp.download(exported.folder));
      for (const { name } of exported.manifest.contents) entries.push(...dataEntries(name, await door.mcp.download(exported.downloads[name])));
      sourcesAhead = exported.sourcesAhead;
    } finally {
      for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await door.mcp.cancel(transfer.transferId).catch(() => {});
    }
    settings = JSON.parse(new TextDecoder().decode(entries.find((entry) => entry.path === SETTINGS_FILE)!.contents as Uint8Array));
  } else {
    const artifact = await door.build(project);
    const { visibility, ...manifest } = project.manifest;
    settings = {
      id: project.id, name: project.name, slug: project.slug, visibility,
      signupPolicy: { mode: visibility === "public" ? "open" : "closed", allowlist: [] }, signInMethods: ["password"],
      kernel: KERNEL_VERSION, manifest,
    };
    entries = [
      { path: SETTINGS_FILE, contents: `${JSON.stringify(settings, null, 2)}\n` },
      ...Object.entries(project.files).map(([file, contents]) => ({ path: `${SOURCES_FOLDER}/${file}`, contents })),
      ...Object.entries(artifact.files).map(([file, contents]) => ({
        path: `${DIST_FOLDER}/${file}`,
        contents: artifact.encodings?.[file] === "base64" ? decodeBase64(contents) : contents,
      })),
    ];
  }
  const text = (file: string) => {
    const entry = entries.find((item) => item.path === `${SOURCES_FOLDER}/${file}`);
    return entry === undefined ? "" : typeof entry.contents === "string" ? entry.contents : new TextDecoder().decode(entry.contents);
  };
  entries.push(...generatedFiles({ settings, actionsSource: text("actions.json"), schemaSource: text("schema.sql"), runtime: hostedRuntime, staticArtifact: staticArtifactSource }));
  const folder = safeFilename(project.slug);
  const archive = createTar(entries.map((entry) => ({ path: `${folder}/${entry.path}`, contents: entry.contents })));
  const bytes = new ArrayBuffer(archive.byteLength);
  new Uint8Array(bytes).set(archive);
  return { blob: new Blob([bytes], { type: "application/x-tar" }), filename: `${folder}.tar`, sourcesAhead };
}

/** The build a draft's export carries, loaded only when a draft is exported. */
async function buildDraft(project: MiniAppProject) {
  const { buildProjectInBrowser } = await import("../runtime/browser-build");
  return buildProjectInBrowser(project);
}

/** What the editor and the project list say once an export is saved. */
export function exportedMessage(filename: string, published: boolean, sourcesAhead: boolean) {
  if (!published) return `${filename} saved. It holds the app's files. A draft has no accounts or rows yet.`;
  return sourcesAhead
    ? `${filename} saved. It holds the app's accounts, rows and rules, and your edits; an app started from it runs your last deploy until you deploy again.`
    : `${filename} saved. It holds the app's files, accounts, rows and rules.`;
}

function safeFilename(value: string) {
  const filename = value.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, "");
  return filename || "konae-app";
}
