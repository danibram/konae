import { expect, test } from "bun:test";
import { parseFunctions, serverFolderEntries, verifyServerFolder, type ServerArtifact } from "../kernel/functions";
import { createTar, readTar } from "./tar";
import { readAppFolder, sourcesPayload, dataManifest } from "./import-folder";
import { createProjectExport } from "./export-project";
import { McpClient, checksum, type Transfer } from "../mcp/client";
import { publishProject } from "../deployments/release";
import { createDefaultManifest } from "../deployments/manifest";
import type { MiniAppProject } from "./types";

const manifest = parseFunctions('{"version":1,"functions":{"orders":{"entry":"functions/orders.ts","mount":"/api/orders","methods":["GET"],"access":"anonymous"}}}');
const serverArtifact: ServerArtifact = { version: 1, abi: "konae-functions/1", manifest, bundles: { orders: 'throw Error("NOT_EVALUATED"); export const handle = () => new Response("PUBLISHED_A");' } };
const project: MiniAppProject = { id: "app", name: "App", slug: "app", manifest: createDefaultManifest(), files: { "functions.json": '{"version":1,"functions":{}}', "package.json": '{"dependencies":{"hono":"4.7.2"}}', "package-lock.json": '{"lockfileVersion":3}' }, directories: [], updatedAt: 0 };
const settings = { id: "app", name: "App", slug: "app", visibility: "public", kernel: "0.12.1", manifest: {}, modules: { functions: { on: true } }, released: { server: { version: 1, abi: "konae-functions/1", manifest }, actionsSource: '{"version":1,"actions":{}}' } };
async function archive() {
  return createTar([{ path: "konae.json", contents: JSON.stringify(settings) }, ...Object.entries(project.files).map(([name, contents]) => ({ path: `app/${name}`, contents })), { path: "dist/index.html", contents: "<html>Public</html>" }, ...await serverFolderEntries(serverArtifact), { path: "data/database.sqlite", contents: "database" }]);
}

test("folder import carries published bundles and snapshot alongside edited sources", async () => {
  const folder = readAppFolder(await archive());
  await verifyServerFolder(folder.server ?? {});
  const payload = sourcesPayload(folder, project, "public");
  expect(payload.project.files).toEqual(project.files);
  expect(payload.serverArtifact).toEqual(serverArtifact);
  expect(payload.initialReleased).toEqual(settings.released);
  expect(JSON.stringify(payload.artifact)).not.toContain("PUBLISHED_A");
  expect((await dataManifest(folder, "target")).released).toEqual(settings.released);
});

test("draft export separates server and static bytes and keeps package lock source", async () => {
  const result = await createProjectExport(project, undefined, { mcp: new McpClient("http://unused.invalid"), build: async () => ({ version: 1, entrypoint: "index.html", files: { "index.html": "<html>Public</html>" }, serverArtifact }) });
  const entries = new Map(readTar(new Uint8Array(await result.blob.arrayBuffer())).map((entry) => [entry.path, new TextDecoder().decode(entry.contents)]));
  expect(entries.get("app/server/orders.mjs")).toBe(serverArtifact.bundles.orders);
  expect(entries.get("app/app/package-lock.json")).toBe(project.files["package-lock.json"]);
  expect([...entries].filter(([path]) => path.startsWith("app/dist/")).every(([, text]) => !text.includes("PUBLISHED_A"))).toBe(true);
});

test("published export keeps server bytes when edited source no longer declares the function", async () => {
  const bytes = await archive();
  class ExportClient extends McpClient {
    override async call<T>(): Promise<T> { return JSON.parse(JSON.stringify({ manifest: { contents: [] }, downloads: {}, folder: { transferId: "folder" }, sourcesAhead: true })); }
    override async download(): Promise<Uint8Array> { return bytes; }
    override async cancel() { return { cancelled: true }; }
  }
  const result = await createProjectExport(project, project.id, { mcp: new ExportClient("http://unused.invalid"), build: async () => { throw Error("must not build edited sources"); } });
  expect(result.sourcesAhead).toBe(true);
  const imported = readAppFolder(new Uint8Array(await result.blob.arrayBuffer()));
  expect(sourcesPayload(imported, project, "public").serverArtifact).toEqual(serverArtifact);
});

test("release upload puts serverArtifact beside artifact and preserves portable source files", async () => {
  const uploaded: unknown[] = [];
  class ReleaseClient extends McpClient {
    override async call<T>(name: string): Promise<T> {
      return JSON.parse(JSON.stringify(name === "release_plan" ? { planId: "plan", projectId: "app", expectedRevision: 0, expectedSourcesRevision: 0, expiresAt: 1 } : { id: "app" }));
    }
    override async upload(bytes: Uint8Array): Promise<Transfer> {
      uploaded.push(JSON.parse(new TextDecoder().decode(bytes)));
      return { transferId: `upload-${uploaded.length}`, size: bytes.length, sha256: await checksum(bytes), chunkBytes: 1024, expiresAt: 1 };
    }
    override async cancel() { return { cancelled: true }; }
  }
  await publishProject(new ReleaseClient("http://unused.invalid"), project, () => ({ project: { ...project, files: { ...project.files, "functions.json": JSON.stringify(manifest) } }, visibility: "public", artifact: { files: { "index.html": "<html>Public</html>" }, serverArtifact } }), async () => true);
  expect(uploaded[0]).toMatchObject({ files: { ...project.files, "functions.json": JSON.stringify(manifest) } });
  expect(uploaded[1]).toMatchObject({ serverArtifact, artifact: { files: { "index.html": "<html>Public</html>" } } });
  expect(JSON.stringify(uploaded[1]).match(/"serverArtifact"/g)?.length).toBe(1);
});
