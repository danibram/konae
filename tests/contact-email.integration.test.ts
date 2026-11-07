import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

// "notify": { "to": "owner" } sends to the app's contact email in konae.json.
// A publish fills it with the owner's email when the app has none, the owner
// changes it with contact_email_set, later publishes keep it, and a publish
// from a folder whose konae.json names an address takes that one.
let root: string;
let kernel: Kernel;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "anonymous", statement: "INSERT INTO notes(body) VALUES(?1)", bindings: [{ source: "input", key: "body", type: "string", maxLength: 200 }], notify: { to: "owner", subject: "New note", body: "{input.body}" } },
} };
const payload = (id: string, extra: object = {}) => ({
  ...extra,
  project: { id, name: "Noticed", slug: "noticed", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);", "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Noticed</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-contact-"));
  const port = Number(process.env.KONAE_CONTACT_TEST_PORT) || 19600 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "contact-setup-secret", ownerEmail: "owner@contact.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("the owner sets the app's contact email in konae.json, and a later publish keeps it", async () => {
  const id = "contact_app";
  await kernel.owner.call("project_create", { projectId: id, name: "Noticed" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const read = () => kernel.owner.call<{ contactEmail: string | null; ownerNotices: string[] }>("contact_email_read", { deploymentId: receipt.id });
  const settings = async () => JSON.parse(await readFile(path.join(kernel.data, "apps", id, "konae.json"), "utf8")) as { contactEmail?: string };
  expect(await read()).toEqual({ deploymentId: receipt.id, contactEmail: "owner@contact.test", ownerNotices: ["notes.add"] });

  expect(await kernel.owner.call("contact_email_set", { deploymentId: receipt.id, email: " Team@Contact.test " })).toMatchObject({ contactEmail: "team@contact.test" });
  expect((await settings()).contactEmail).toBe("team@contact.test");
  await expect(kernel.owner.call("contact_email_set", { deploymentId: receipt.id, email: "not an address" })).rejects.toThrow("The contact email is not a valid email address.");

  await release(kernel.owner, payload(id), id);
  expect((await read()).contactEmail).toBe("team@contact.test");

  // A folder's konae.json names its own address, which a publish from it takes.
  await release(kernel.owner, payload(id, { contactEmail: "Folder@Contact.test" }), id);
  expect((await read()).contactEmail).toBe("folder@contact.test");
  await expect(release(kernel.owner, payload(id, { contactEmail: "nobody" }), id)).rejects.toThrow("The contact email is not a valid email address.");

  expect(await kernel.owner.call("contact_email_set", { deploymentId: receipt.id, email: null })).toMatchObject({ contactEmail: null, ownerNotices: ["notes.add"] });
  expect((await settings()).contactEmail).toBeUndefined();
  // An app with no address gets the owner's again from the next publish.
  await release(kernel.owner, payload(id), id);
  expect((await read()).contactEmail).toBe("owner@contact.test");
}, 120_000);
