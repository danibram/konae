import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

// hostname_set moves a published app to another generated hostname on the
// konae binary. The old name redirects with a 308, stays the app's, and does
// so after a restart; project_inspect reads the new url back.
let root: string;
let kernel: Kernel;

const payload = (id: string, slug: string, page = "Moved") => ({
  project: { id, name: slug, slug, manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "", "actions.json": JSON.stringify({ version: 1, actions: {} }) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": `<!doctype html><main>${page}</main>` } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-hostname-"));
  const port = Number(process.env.KONAE_HOSTNAME_TEST_PORT) || 19700 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "hostname-setup-secret", ownerEmail: "owner@hostname.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("an app moves to a free name, its old name redirects there and stays its own", async () => {
  await kernel.owner.call("project_create", { projectId: "move_app", name: "kamynando2" });
  await kernel.owner.call("project_create", { projectId: "other_app", name: "other" });
  const { receipt } = await release(kernel.owner, payload("move_app", "kamynando2"), "move_app");
  const { receipt: other } = await release(kernel.owner, payload("other_app", "other"), "other_app");
  expect(receipt.hostname).toBe("kamynando2.localhost");

  await expect(kernel.owner.call("hostname_set", { deploymentId: receipt.id, name: "other" })).rejects.toThrow("other.localhost is another app's address on this installation. Choose another name.");
  await expect(kernel.owner.call("hostname_set", { deploymentId: receipt.id, name: "my_app" })).rejects.toThrow("my_app is not a valid name.");

  const moved = await kernel.owner.call<{ hostname: string; url: string; changed: boolean; previousHostnames: string[] }>("hostname_set", { deploymentId: receipt.id, name: "kamynando" });
  expect(moved).toMatchObject({ hostname: "kamynando.localhost", changed: true, previousHostnames: ["kamynando2.localhost"] });
  const inspected = await kernel.owner.call<{ deployment: { url: string; hostname: string } }>("project_inspect", { projectId: "move_app" });
  expect(inspected.deployment).toMatchObject({ hostname: "kamynando.localhost", url: moved.url });

  const page = await appFetch(kernel, "kamynando.localhost", "/");
  expect(page.status).toBe(200);
  expect(await page.text()).toContain("Moved");
  for (const method of ["GET", "POST"]) {
    const old = await appFetch(kernel, "kamynando2.localhost", "/_konae/invitation?token=abc", { method });
    expect(old.status).toBe(308);
    expect(old.headers.get("location")).toBe(`${moved.url}/_konae/invitation?token=abc`);
  }
  await expect(kernel.owner.call("hostname_set", { deploymentId: other.id, name: "kamynando2" })).rejects.toThrow("kamynando2.localhost is another app's address on this installation.");

  // A republish keeps the new name, and a restart keeps the redirect.
  await release(kernel.owner, payload("move_app", "kamynando2", "Again"), "move_app");
  await kernel.stop();
  await kernel.start();
  expect((await appFetch(kernel, "kamynando2.localhost", "/")).status).toBe(308);
  const again = await appFetch(kernel, "kamynando.localhost", "/");
  expect(again.status).toBe(200);
  expect(await again.text()).toContain("Again");
}, 120_000);
