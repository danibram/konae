import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

let root: string;
let kernel: Kernel;

const payload = (id: string) => ({
  project: { id, name: id, slug: id, manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "", "actions.json": JSON.stringify({ version: 1, actions: {} }) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Settings</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-expected-settings-"));
  const port = Number(process.env.KONAE_EXPECTED_SETTINGS_TEST_PORT) || 19800 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "expected-settings-secret", ownerEmail: "owner@settings.test", env: { KONAE_DOMAIN: "apps.example.org" } });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

async function app(id: string) {
  await kernel.owner.call("project_create", { projectId: id, name: id });
  return (await release(kernel.owner, payload(id), id)).receipt;
}

test("contact_email_set refuses an old read", async () => {
  const { id } = await app("expected_contact");
  const first = await kernel.owner.call<{ contactEmail: string | null }>("contact_email_read", { projectId: id });
  await kernel.owner.call("contact_email_set", { projectId: id, email: "second@settings.test" });
  await expect(kernel.owner.call("contact_email_set", { projectId: id, email: "stale@settings.test", expected: first.contactEmail })).rejects.toMatchObject({ refusal: { code: "setting.changed" }, message: expect.stringContaining("second@settings.test") });
  expect((await kernel.owner.call<{ contactEmail: string | null }>("contact_email_read", { projectId: id })).contactEmail).toBe("second@settings.test");
}, 120_000);

test("domain_set refuses an old read", async () => {
  const { id } = await app("expected_domain");
  const first = await kernel.owner.call<{ hostname: string | null }>("domain_read", { projectId: id });
  await kernel.owner.call("domain_set", { projectId: id, hostname: "first.example.org" });
  await expect(kernel.owner.call("domain_set", { projectId: id, hostname: "second.example.org", expected: first.hostname })).rejects.toMatchObject({ refusal: { code: "setting.changed" }, message: expect.stringContaining("first.example.org") });
  expect((await kernel.owner.call<{ hostname: string | null }>("domain_read", { projectId: id })).hostname).toBe("first.example.org");
}, 120_000);

test("hostname_set refuses an old read", async () => {
  const { id } = await app("expected_hostname");
  const first = await kernel.owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId: id });
  await kernel.owner.call("hostname_set", { projectId: id, name: "first-name" });
  await expect(kernel.owner.call("hostname_set", { projectId: id, name: "second-name", expected: first.deployment.hostname })).rejects.toMatchObject({ refusal: { code: "setting.changed" }, message: expect.stringContaining("first-name.localhost") });
  expect((await kernel.owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId: id })).deployment.hostname).toBe("first-name.localhost");
}, 120_000);

test("path_set refuses an old read", async () => {
  const { id } = await app("expected_path");
  const first = await kernel.owner.call<{ enabled: boolean; name: string | null; acceptSharedOrigin: boolean }>("path_read", { projectId: id });
  await kernel.owner.call("path_set", { projectId: id, enabled: false, name: "first-path" });
  await expect(kernel.owner.call("path_set", { projectId: id, enabled: false, name: "second-path", expected: { enabled: first.enabled, name: first.name, acceptSharedOrigin: first.acceptSharedOrigin } })).rejects.toMatchObject({ refusal: { code: "setting.changed" }, message: expect.stringContaining("first-path") });
  expect((await kernel.owner.call<{ name: string | null }>("path_read", { projectId: id })).name).toBe("first-path");
}, 120_000);

test("maintenance_set refuses an old read", async () => {
  const { id } = await app("expected_maintenance");
  const first = await kernel.owner.call<{ maintenance: boolean }>("maintenance_read", { projectId: id });
  await kernel.owner.call("maintenance_set", { projectId: id, on: true });
  await expect(kernel.owner.call("maintenance_set", { projectId: id, on: false, expected: first.maintenance })).rejects.toMatchObject({ refusal: { code: "setting.changed" }, message: expect.stringContaining("true") });
  expect((await kernel.owner.call<{ maintenance: boolean }>("maintenance_read", { projectId: id })).maintenance).toBe(true);
}, 120_000);
