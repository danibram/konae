import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { McpClient } from "../src/mcp/client";
import { PULL_DIRECTORY, pullLocal } from "./pull";

let root: string | undefined;
afterEach(async () => { if (root) await rm(root, { recursive: true, force: true }); root = undefined; });

describe("data-only pull", () => {
  test("refreshes staged data without reading or replacing local sources and base markers", async () => {
    root = await mkdtemp(path.join(tmpdir(), "konae-data-only-"));
    const data = new TextEncoder().encode("remote database snapshot");
    const sha256 = createHash("sha256").update(data).digest("hex");
    const manifest = {
      format: "konae-package", version: 1, packageId: "pkg_new", createdAt: Date.now(), deploymentId: "dep_1", projectId: "app_1",
      visibility: "private", publishedRevision: 8, sourcesRevision: 42,
      contents: [{ name: "database", size: data.length, sha256 }], excluded: [],
    };
    const calls: string[] = [];
    const client = {
      async call(name: string) {
        calls.push(name);
        if (name === "project_inspect") return { deployment: { id: "dep_1" } };
        if (name === "project_export") return { manifest, downloads: { database: { transferId: "transfer_1", size: data.length, sha256, chunkBytes: 65_536 } } };
        if (name === "transfer_read") return { data: Buffer.from(data).toString("base64"), offset: 0, nextOffset: data.length };
        throw Error(`Unexpected MCP call ${name}`);
      },
      async cancel() {},
    } as unknown as McpClient;

    const before = {
      app: new TextEncoder().encode("local source"), settings: new TextEncoder().encode("local settings"),
      marker: new TextEncoder().encode('{"sourceDigest":"local base"}'),
    };
    await mkdir(path.join(root, "app"));
    await mkdir(path.join(root, ".konae/pull/app"), { recursive: true });
    await mkdir(path.join(root, ".konae/pull/data"), { recursive: true });
    await writeFile(path.join(root, "app/index.js"), before.app);
    await writeFile(path.join(root, "konae.json"), before.settings);
    await writeFile(path.join(root, ".konae/source-revision.json"), before.marker);
    await writeFile(path.join(root, ".konae/pull/app/index.js"), "staged source stays");
    await writeFile(path.join(root, ".konae/pull/data/database.sqlite"), "old snapshot");

    const pulled = await pullLocal(root, client, "http://localhost", "app_1", { dataOnly: true });

    expect(pulled.dataOnly).toBe(true);
    expect(typeof pulled.snapshotId).toBe("string");
    expect(calls).not.toContain("project_sources_read");
    expect(await readFile(path.join(root, "app/index.js"))).toEqual(before.app);
    expect(await readFile(path.join(root, "konae.json"))).toEqual(before.settings);
    expect(await readFile(path.join(root, ".konae/source-revision.json"))).toEqual(before.marker);
    expect(await readFile(path.join(root, PULL_DIRECTORY, "app/index.js"), "utf8")).toBe("staged source stays");
    expect(await readFile(path.join(root, PULL_DIRECTORY, "data/database.sqlite"))).toEqual(data);
  });
});
