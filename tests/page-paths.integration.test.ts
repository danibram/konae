import { afterAll, beforeAll, expect, test } from "bun:test";
import { readFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

let root: string;
let kernel: Kernel;
let deployment: { id: string; hostname: string; url: string };
let payload: Record<string, any>;

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-page-paths-"));
  const port = 20900 + Math.floor(Math.random() * 80);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "pages-test-secret", ownerEmail: "owner@pages.test", env: { KONAE_DOMAIN: "apps.example.org" } });
  const id = "page_paths_test";
  const directory = path.resolve(import.meta.dir, "../examples/product-fit/blog");
  const files = Object.fromEntries(await Promise.all(["schema.sql", "actions.json", "index.html", "post.html", "app.js"].map(async (name) => [name, await readFile(path.join(directory, name), "utf8")])));
  files["actions.json"] = JSON.stringify({ version: 1, actions: {
    ...JSON.parse(files["actions.json"]).actions,
    "posts.route": { kind: "query", access: "anonymous", statement: "SELECT 'route' AS source LIMIT 1", bindings: [], http: { method: "GET", path: "/posts/route" } },
    "posts.ping": { kind: "query", access: "anonymous", statement: "SELECT 1 AS ok LIMIT 1", bindings: [], http: { method: "GET", path: "/api/ping" } },
    "posts.archive": { kind: "query", access: "anonymous", statement: "SELECT title FROM posts WHERE ?1 = 1 LIMIT 1", bindings: [{ source: "input", key: "year", type: "number" }], page: { path: "/archive/:year", file: "post.html" } },
    "posts.failure": { kind: "query", access: "anonymous", statement: "SELECT CASE WHEN ?1 = 'explode' THEN abs(-9223372036854775808) ELSE title END AS title FROM posts WHERE slug = 'hello' LIMIT 1", bindings: [{ source: "input", key: "slug", type: "string" }], page: { path: "/errors/:slug", file: "post.html" } },
  } });
  await kernel.owner.call("project_create", { projectId: id, name: "Blog" });
  payload = { project: { id, name: "Blog", slug: "blog", manifest: { version: 1, visibility: "public" }, files }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: {
    "index.html": files["index.html"],
    "post.html": files["post.html"],
    "posts/static.html": "<!doctype html><html><head><title>Static file</title></head><body>Static</body></html>",
    "app.js": files["app.js"],
  } } };
  deployment = (await release(kernel.owner, payload, id)).receipt;
  await kernel.owner.call("database_execute", { projectId: id, operation: "sql", input: { sql: "INSERT INTO posts VALUES ('hello', 'Hello <world> & friends', 'A short summary', 'cover.png', 'Post body', 1, 1)" } });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("page paths answer rows, refresh their validator and feed the sitemap", async () => {
  const off = await appFetch(kernel, deployment.hostname, "/posts/hello");
  expect(off.headers.get("x-robots-tag")).toBe("noindex");
  expect(await off.text()).toContain("<title>Hello &lt;world&gt; &amp; friends</title>");
  const state = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  await kernel.owner.call("module_set", { projectId: deployment.id, module: "seo", on: true, expected: state.modules.seo.state });
  const page = await appFetch(kernel, deployment.hostname, "/posts/hello");
  expect(page.status).toBe(200);
  const tag = page.headers.get("etag")!;
  const html = await page.text();
  expect(html).toContain("<title>Hello &lt;world&gt; &amp; friends</title>");
  expect(html).toContain("property=\"og:title\" content=\"Hello &lt;world&gt; &amp; friends\"");
  expect(html).toContain("property=\"og:description\" content=\"A short summary\"");
  expect(html).toContain(`property="og:image" content="${deployment.url}/cover.png"`);
  expect(html).toContain(`property="og:url" content="${deployment.url}/posts/hello"`);
  expect(html).toContain("name=\"konae-page-params\" content=\"{&quot;slug&quot;:&quot;hello&quot;}\"");
  expect(html).toContain("<script type=\"module\"");
  const staticPage = await appFetch(kernel, deployment.hostname, "/posts/static");
  expect(await staticPage.text()).toContain("<title>Static file</title>");
  const route = await appFetch(kernel, deployment.hostname, "/posts/route");
  expect(await route.text()).toContain('"source":"route"');
  const missing = await appFetch(kernel, deployment.hostname, "/posts/missing");
  expect(missing.status).toBe(404);
  const sitemap = await appFetch(kernel, deployment.hostname, "/sitemap.xml");
  expect(await sitemap.text()).toContain(`${deployment.url}/posts/hello`);
  await kernel.owner.call("database_execute", { projectId: deployment.id, operation: "sql", input: { sql: "UPDATE posts SET title = 'Changed' WHERE slug = 'hello'" } });
  const changed = await appFetch(kernel, deployment.hostname, "/posts/hello", { headers: { "if-none-match": tag } });
  expect(changed.status).toBe(200);
  expect(await changed.text()).toContain("<title>Changed</title>");
  await kernel.owner.call("path_set", { projectId: deployment.id, enabled: true, acceptSharedOrigin: true, name: "blog" });
  const address = await appFetch(kernel, "apps.example.org", "/p/blog/posts/hello");
  expect(address.status).toBe(200);
  expect(await address.text()).toContain("<title>Changed</title>");
  await kernel.owner.call("domain_set", { projectId: deployment.id, hostname: "verified.example.org" });
  const domainRecord = path.join(kernel.data, "apps", deployment.id, ".konae", "domain.json");
  await mkdir(path.dirname(domainRecord), { recursive: true });
  await writeFile(domainRecord, JSON.stringify({ hostname: "verified.example.org", token: "test", verified: true, createdAt: Date.now() }));
  await kernel.stop();
  await kernel.start();
  const customDomain = await appFetch(kernel, "verified.example.org", "/posts/hello");
  expect(customDomain.status).toBe(200);
  expect(await customDomain.text()).toContain("http://verified.example.org:");
});

test("release refuses a page whose file is absent from the Build Artifact", async () => {
  const broken = JSON.parse(JSON.stringify(payload));
  delete broken.artifact.files["post.html"];
  await expect(release(kernel.owner, broken, deployment.id)).rejects.toThrow(/names post\.html as its page file, but the Build Artifact does not contain it\./);
});

test("invalid bindings are missing pages, and untitled rows leave the file alone", async () => {
  const tooLong = await appFetch(kernel, deployment.hostname, `/posts/${"x".repeat(121)}`);
  expect(tooLong.status).toBe(404);
  const invalidNumber = await appFetch(kernel, deployment.hostname, "/archive/nope");
  expect(invalidNumber.status).toBe(404);
  await kernel.owner.call("database_execute", { projectId: deployment.id, operation: "sql", input: { sql: "INSERT INTO posts VALUES ('untitled', NULL, 'Summary', 'cover.png', 'Body', 1, 1)" } });
  const untitled = await appFetch(kernel, deployment.hostname, "/posts/untitled");
  expect(untitled.status).toBe(200);
  expect(untitled.headers.get("x-robots-tag")).toBe("noindex");
  const untitledHtml = await untitled.text();
  expect(untitledHtml).toContain('<meta name="konae-page-params" content="{&quot;slug&quot;:&quot;untitled&quot;}">');
  expect(untitledHtml).toContain(payload.artifact.files["post.html"].replace("</head>", '<meta name="konae-page-params" content="{&quot;slug&quot;:&quot;untitled&quot;}"></head>'));
  const empty = await appFetch(kernel, deployment.hostname, "/posts/empty");
  await kernel.owner.call("database_execute", { projectId: deployment.id, operation: "sql", input: { sql: "INSERT INTO posts VALUES ('empty', '', 'Summary', 'cover.png', 'Body', 1, 1)" } });
  expect(empty.status).toBe(404);
  const emptyTitle = await appFetch(kernel, deployment.hostname, "/posts/empty");
  expect(emptyTitle.headers.get("x-robots-tag")).toBe("noindex");
  expect(await emptyTitle.text()).toContain('<meta name="konae-page-params" content="{&quot;slug&quot;:&quot;empty&quot;}">');
  const failure = await appFetch(kernel, deployment.hostname, "/errors/explode");
  expect(failure.status).toBe(500);
  expect(await failure.text()).toBe("The page query failed.");
});

test("page queries do not spend the anonymous action limit or stall another caller", async () => {
  const headers = { "x-forwarded-for": "198.18.1.4" };
  const baselineStart = performance.now();
  const baseline = await fetch(`${kernel.url}/api/ping`, { headers: { host: deployment.hostname, "x-forwarded-for": "198.18.1.6" } });
  const baselineMs = performance.now() - baselineStart;
  const start = performance.now();
  const workload = Promise.all(Array.from({ length: 20 }, () => fetch(`${kernel.url}/posts/hello`, { headers: { ...headers, host: deployment.hostname } })));
  const otherStart = performance.now();
  const other = await fetch(`${kernel.url}/api/ping`, { headers: { host: deployment.hostname, "x-forwarded-for": "198.18.1.5" } });
  const otherMs = performance.now() - otherStart;
  const responses = await workload;
  const elapsed = performance.now() - start;
  console.log(`page-load requests=${responses.length} statuses=${new Set(responses.map((response) => response.status)).size} elapsedMs=${elapsed.toFixed(1)} ping=${other.status} pingMs=${otherMs.toFixed(1)} baselinePing=${baseline.status} baselinePingMs=${baselineMs.toFixed(1)}`);
  expect(responses.every((response) => response.status === 200)).toBe(true);
  expect(other.status).toBe(200);
});
