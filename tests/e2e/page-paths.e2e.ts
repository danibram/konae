import { readFile } from "node:fs/promises";
import { McpClient } from "../../src/mcp/client";
import { publishProject } from "../../src/deployments/release";
import { assembleStaticArtifact, pageRoutesFromActions } from "../../src/lib/static-artifact.js";
import { expect, test } from "playwright/test";

test("page files load scripts at host and path addresses", async ({ page, baseURL }) => {
  const email = `page-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const signup = await page.request.post(new URL("/_konae/owner/signup", baseURL).href, {
    headers: { "x-forwarded-for": "198.51.100.22" },
    data: { email, password, setupKey: "local-development-key" },
  });
  expect(signup.ok()).toBe(true);
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", baseURL).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const id = `app_${crypto.randomUUID().replaceAll("-", "")}`;
  const slug = `page-${crypto.randomUUID().slice(0, 8)}`;
  const blog = new URL("../../examples/product-fit/blog/", import.meta.url);
  const files = Object.fromEntries(await Promise.all(["schema.sql", "actions.json", "index.html", "post.html", "app.js"].map(async (name) => [name, await readFile(new URL(name, blog), "utf8")])));
  const project = { id, name: "Browser page", slug, manifest: { version: 1, visibility: "public" }, updatedAt: Date.now(), directories: [], files };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(project)));
  let created: { sourcesRevision: number };
  try {
    created = await client.call("project_create", { projectId: id, name: project.name, slug, manifest: project.manifest, sourcesTransferId: transfer.transferId });
  } finally {
    await client.cancel(transfer.transferId).catch(() => {});
  }
  const runtime = await readFile(new URL("../../src/mcp/runtime.js", import.meta.url), "utf8");
  const artifact = await assembleStaticArtifact({
    paths: Object.keys(files),
    read: async (source) => files[source as keyof typeof files],
    compile: async () => ({ styles: "", script: "" }),
    bundle: async (source) => ({ script: files[source as keyof typeof files], styles: "" }),
    runtime,
    pageRoutes: pageRoutesFromActions(files["actions.json"]),
  });
  const published = await publishProject(client, project, () => ({ project, visibility: "public", artifact }), async () => true, undefined, undefined, created.sourcesRevision);
  const receipt = published.receipt as { url: string };
  await client.call("database_execute", { projectId: id, operation: "sql", input: { sql: "INSERT INTO posts VALUES ('hello', 'Hello', 'Summary', 'cover.png', 'Body rendered by the page script', 1, 1)" } });
  const address = new URL(receipt.url);
  const outcomes: string[] = [];
  await page.goto(`${receipt.url}/posts/hello`);
  await expect.soft(page.locator("#body")).toHaveText("Body rendered by the page script");
  outcomes.push(await page.locator("#body").innerText());
  const pathName = `blog-${crypto.randomUUID().slice(0, 8)}`;
  await client.call("path_set", { projectId: id, enabled: true, acceptSharedOrigin: true, name: pathName });
  await page.goto(`http://apps.example.org:${address.port}/p/${pathName}/posts/hello`);
  await expect.soft(page.locator("#body")).toHaveText("Body rendered by the page script");
  outcomes.push(await page.locator("#body").innerText());
  expect.soft(outcomes).toEqual(["Body rendered by the page script", "Body rendered by the page script"]);
});
