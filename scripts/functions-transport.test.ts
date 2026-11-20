import { afterEach, expect, test } from "bun:test";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { parseFunctions, serverFolderEntries } from "../src/kernel/functions";
import { createTar } from "../src/lib/tar";
import { McpClient, checksum, encodeBase64 } from "../src/mcp/client";
import { pullLocal, PULL_DIRECTORY } from "./pull";
import { rehearsePulledSnapshot } from "./rehearse";
import type { AppSettings } from "../src/lib/app-folder";

const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))); });

test("pull stages published server bytes and rehearsal carries the candidate server folder", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-function-pull-")); roots.push(root);
  const manifest = parseFunctions('{"version":1,"functions":{"main":{"entry":"functions/main.ts","mount":"/api","methods":["GET"],"access":"anonymous"}}}');
  const bundle = 'throw Error("NEVER_RUN"); export const handle = () => new Response("published");';
  const serverEntries = await serverFolderEntries({ version: 1, abi: "konae-functions/1", manifest, bundles: { main: bundle } });
  const settings: AppSettings = { id: "app", name: "App", slug: "app", kernel: "0.12.1", visibility: "public", modules: { functions: { on: true } }, manifest: {}, released: { server: JSON.parse(serverEntries[0].contents) } };
  const sources = { "functions.json": '{"version":1,"functions":{}}', "index.html": "<html>draft</html>" };
  await mkdir(path.join(root, "app"));
  for (const [name, contents] of Object.entries(sources)) await writeFile(path.join(root, "app", name), contents);
  await writeFile(path.join(root, "konae.json"), JSON.stringify(settings));
  const archive = createTar([{ path: "konae.json", contents: JSON.stringify(settings) }, ...Object.entries(sources).map(([name, contents]) => ({ path: `app/${name}`, contents })), { path: "dist/index.html", contents: "<html>published</html>" }, ...serverEntries]);
  const database = new TextEncoder().encode("database-snapshot");
  const dataHash = await checksum(database);
  const folderHash = await checksum(archive);
  class Client extends McpClient {
    override async call<T>(name: string, input: object = {}): Promise<T> {
      const args = JSON.parse(JSON.stringify(input));
      const transfer = (id: string, bytes: Uint8Array, sha256: string) => ({ transferId: id, size: bytes.length, sha256, chunkBytes: 1024 * 1024, expiresAt: 1 });
      let value: unknown;
      if (name === "project_inspect") value = { deployment: { id: "app" } };
      else if (name === "project_sources_read") value = { revision: 1 };
      else if (name === "project_export") value = { manifest: { format: "konae-package", version: 1, packageId: "p", createdAt: 0, deploymentId: "app", projectId: "app", visibility: "public", publishedRevision: 1, sourcesRevision: 1, contents: [{ name: "database", size: database.length, sha256: dataHash }], excluded: [] }, downloads: { database: transfer("database", database, dataHash) }, folder: transfer("folder", archive, folderHash) };
      else if (name === "transfer_read") { const bytes = args.transferId === "folder" ? archive : database; value = { offset: 0, nextOffset: bytes.length, data: encodeBase64(bytes) }; }
      else throw Error(`Unexpected ${name}`);
      return JSON.parse(JSON.stringify(value));
    }
    override async cancel() { return { cancelled: true }; }
  }
  const pulled = await pullLocal(root, new Client("http://unused.invalid"), "http://source.invalid", "app");
  expect(pulled.sourcesInstalled).toBe(true);
  expect(await readFile(path.join(root, PULL_DIRECTORY, "server/main.mjs"), "utf8")).toBe(bundle);
  await mkdir(path.join(root, "dist"));
  await writeFile(path.join(root, "dist/index.html"), "<html>candidate</html>");
  for (const entry of serverEntries) { await mkdir(path.dirname(path.join(root, entry.path)), { recursive: true }); await writeFile(path.join(root, entry.path), entry.contents); }
  let invoked = false;
  await rehearsePulledSnapshot(root, settings, async (scratch) => {
    invoked = true;
    expect(await readFile(path.join(scratch, "server/main.mjs"), "utf8")).toBe(bundle);
    expect(await readFile(path.join(scratch, "app/functions.json"), "utf8")).toBe(sources["functions.json"]);
  });
  expect(invoked).toBe(true);
});
