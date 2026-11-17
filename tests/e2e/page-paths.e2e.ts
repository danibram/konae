import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { McpClient } from "../../src/mcp/client";
import { publishProject } from "../../src/deployments/release";
import { expect, test } from "playwright/test";

let buildRoot: string | undefined;

test.afterEach(async () => {
  if (buildRoot) await rm(buildRoot, { recursive: true, force: true });
  buildRoot = undefined;
});

test("page files load scripts at host and path addresses", async ({ page, baseURL }) => {
  const email = `page-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const response = await fetch(new URL("/_konae/owner/signup", baseURL).href, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": "198.51.100.22" },
    body: JSON.stringify({ email, password, setupKey: "local-development-key" }),
  });
  expect(response.ok).toBe(true);
  const cookieHeader = response.headers.getSetCookie().find((value) => value.startsWith("konae_owner_session="));
  if (cookieHeader) await page.context().addCookies([{ name: "konae_owner_session", value: cookieHeader.match(/^konae_owner_session=([^;]+)/)![1], url: baseURL }]);
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", baseURL).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const id = `app_${crypto.randomUUID().replaceAll("-", "")}`;
  const slug = `page-${crypto.randomUUID().slice(0, 8)}`;
  const blog = new URL("../../examples/product-fit/blog/", import.meta.url);
  const files = Object.fromEntries(await Promise.all(["schema.sql", "actions.json", "index.html", "post.html", "app.js"].map(async (name) => [name, await readFile(new URL(name, blog), "utf8")])));
  buildRoot = await mkdtemp(path.join(tmpdir(), "konae-page-path-build-"));
  const app = path.join(buildRoot, "app");
  await mkdir(app);
  await writeFile(path.join(buildRoot, "konae.json"), JSON.stringify({ manifest: {} }));
  await Promise.all(Object.entries(files).map(([name, contents]) => writeFile(path.join(app, name), contents)));
  const artifact = JSON.parse(execFileSync("bun", ["-e", "import { buildLocal } from './scripts/konae.ts'; const { artifact } = await buildLocal(process.env.KONAE_BUILD_ROOT); process.stdout.write(JSON.stringify(artifact));"], {
    cwd: path.resolve(import.meta.dirname, "../.."),
    env: { ...process.env, KONAE_BUILD_ROOT: buildRoot },
    encoding: "utf8",
  }));
  const project = { id, name: "Browser page", slug, manifest: { version: 1, visibility: "public" }, updatedAt: Date.now(), directories: [], files };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(project)));
  let created: { sourcesRevision: number };
  try {
    created = await client.call("project_create", { projectId: id, name: project.name, slug, manifest: project.manifest, sourcesTransferId: transfer.transferId });
  } finally {
    await client.cancel(transfer.transferId).catch(() => {});
  }
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
