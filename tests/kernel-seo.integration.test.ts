import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { encodeBase64 } from "../src/mcp/client";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";
import { readSite, renderPage } from "../scripts/docs-site";

let root: string;
let kernel: Kernel;
let deployment: { id: string; hostname: string; url: string };

async function curlApp(hostname: string, route: string) {
  const process = Bun.spawn(["curl", "-fsS", "-H", `Host: ${hostname}`, `${kernel.url}${route}`], { stdout: "pipe", stderr: "pipe" });
  const body = await new Response(process.stdout).text();
  const error = await new Response(process.stderr).text();
  if (await process.exited !== 0) throw Error(`curl ${route} failed: ${error}`);
  return body;
}

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-seo-"));
  const port = Number(process.env.KONAE_SEO_TEST_PORT) || 20500 + Math.floor(Math.random() * 80);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "seo-test-secret", ownerEmail: "owner@seo.test", env: { KONAE_DOMAIN: "apps.example.org" } });
  const id = "seo_test_app";
  await kernel.owner.call("project_create", { projectId: id, name: "SEO test" });
  const payload = {
    project: { id, name: "SEO test", slug: "seo-test", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "", "actions.json": JSON.stringify({ version: 1, actions: {} }) } },
    visibility: "public",
    artifact: { version: 1, entrypoint: "index.html", files: {
      "index.html": "<!doctype html><html><head><title>Home</title><meta name=\"description\" content=\"A small app\"></head><body><a href=\"/about\">About</a></body></html>",
      "about.html": "<!doctype html><html><head><title>A \"small\" <app> &amp; more</title><meta property=\"og:title\" content=\"Owner title\"></head><body>About</body></html>",
      "escaped.html": "<!doctype html><html><head><title>A \"small\" <app> &amp; more</title></head><body>Escaped</body></html>",
      "guide/index.html": "<!doctype html><html><head><title>Guide home</title></head><body>Guide</body></html>",
      "untitled.html": "<!doctype html><html><body>No title</body></html>",
      "svg-title.html": "<!doctype html><html><head></head><body><svg><title>Vector label</title></svg></body></html>",
      "robots.txt": "User-agent: special\nDisallow: /private\n",
      "card.png": "image",
    } },
  };
  deployment = (await release(kernel.owner, payload, id)).receipt;
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("seo controls listing, cards, discovery files and canonical validators", async () => {
  const off = await appFetch(kernel, deployment.hostname, "/");
  expect(off.headers.get("x-robots-tag")).toBe("noindex");
  const offState = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  await kernel.owner.call("module_set", { projectId: deployment.id, module: "seo", on: true, options: { image: "card.png" }, expected: offState.modules.seo.state });
  const readback = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  expect(readback.modules.seo.state.on).toBe(true);
  expect(readback.modules.seo.current.canonical.url).toBe(deployment.url);
  await kernel.owner.call("module_set", { projectId: deployment.id, module: "seo", options: { canonical: "domain" }, expected: readback.modules.seo.state });
  const domainChoice = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  expect(domainChoice.modules.seo.current.canonical.reason).toContain("not verified");

  const page = await appFetch(kernel, deployment.hostname, "/about");
  expect(page.headers.get("x-robots-tag")).toBeNull();
  const tag = page.headers.get("etag")!;
  const html = await page.text();
  expect(html.length).toBeLessThan(300_000);
  expect(html).toContain(`href="${deployment.url}/about"`);
  expect(html).toContain(`property="og:url" content="${deployment.url}/about"`);
  expect(html).toContain("property=\"og:title\" content=\"Owner title\"");
  expect(html).toContain("property=\"og:description\"");
  expect(html).toContain(`property="og:image" content="${deployment.url}/card.png"`);
  const asset = await appFetch(kernel, deployment.hostname, "/card.png");
  const assetTag = asset.headers.get("etag")!;
  const unchangedAsset = await appFetch(kernel, deployment.hostname, "/card.png", { headers: { "if-none-match": assetTag } });
  expect(unchangedAsset.status).toBe(304);
  const escaped = await appFetch(kernel, deployment.hostname, "/escaped");
  expect(await escaped.text()).toContain("property=\"og:title\" content=\"A &quot;small&quot; &lt;app&gt; &amp; more\"");
  const untitled = await appFetch(kernel, deployment.hostname, "/untitled");
  expect(untitled.headers.get("x-robots-tag")).toBe("noindex");
  const svgTitle = await appFetch(kernel, deployment.hostname, "/svg-title");
  expect(svgTitle.headers.get("x-robots-tag")).toBe("noindex");

  const withoutExtension = await appFetch(kernel, deployment.hostname, "/about.html");
  expect(await withoutExtension.text()).toContain(`href="${deployment.url}/about"`);
  const directoryIndex = await appFetch(kernel, deployment.hostname, "/guide/");
  const directoryHtml = await directoryIndex.text();
  expect(directoryIndex.status).toBe(200);
  expect(directoryHtml).toContain(`href="${deployment.url}/guide/"`);
  const robots = await appFetch(kernel, deployment.hostname, "/robots.txt");
  const ownRobots = await robots.text();
  expect(ownRobots).toContain("Disallow: /private");
  expect(ownRobots).toContain(`Sitemap: ${deployment.url}/sitemap.xml`);
  await rm(path.join(kernel.data, "apps", deployment.id, "dist", "robots.txt"));
  const generatedRobots = await appFetch(kernel, deployment.hostname, "/robots.txt");
  expect(await generatedRobots.text()).toContain(`Sitemap: ${deployment.url}/sitemap.xml`);
  const sitemap = await appFetch(kernel, deployment.hostname, "/sitemap.xml");
  const xml = await sitemap.text();
  expect(xml).toContain(`${deployment.url}/about`);
  expect(xml).toContain(`${deployment.url}/guide/`);
  expect(xml).not.toContain("untitled");
  expect(xml).not.toContain("svg-title");
  const omitted = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  expect(JSON.stringify(omitted.modules.seo.current.excluded)).toContain("untitled.html");
  expect(JSON.stringify(omitted.modules.seo.current.excluded)).toContain("no title");
  expect(JSON.stringify(omitted.modules.seo.current.excluded)).toContain("svg-title.html");

  const conditional = await appFetch(kernel, deployment.hostname, "/about", { headers: { "if-none-match": tag } });
  expect(conditional.status).toBe(304);
  await kernel.owner.call("domain_set", { projectId: deployment.id, hostname: "verified.example.org" });
  const domainRecord = path.join(kernel.data, "apps", deployment.id, ".konae", "domain.json");
  await mkdir(path.dirname(domainRecord), { recursive: true });
  await writeFile(domainRecord, JSON.stringify({ hostname: "verified.example.org", token: "test", verified: true, createdAt: Date.now() }));
  const verified = await kernel.owner.call<any>("modules_read", { projectId: deployment.id });
  expect(verified.modules.seo.current.canonical.reason).toContain("verified custom domain");
  const afterDomain = await appFetch(kernel, deployment.hostname, "/about", { headers: { "if-none-match": tag } });
  expect(afterDomain.status).toBe(200);
  const customCanonical = `http://verified.example.org:${new URL(kernel.url).port}/about`;
  expect(await afterDomain.text()).toContain(customCanonical);
  await kernel.owner.call("path_set", { projectId: deployment.id, enabled: true, acceptSharedOrigin: true });
  const pathAddress = await appFetch(kernel, "apps.example.org", `/p/${deployment.id}/about`);
  expect(await pathAddress.text()).toContain(customCanonical);
}, 120_000);

test("module_set accepts each public app card file and serves its image URL", async () => {
  const homeHtml = await readFile(path.resolve(import.meta.dir, "../examples/home/index.html"), "utf8");
  const spanishHomeHtml = await readFile(path.resolve(import.meta.dir, "../examples/home/pages/es/index.html"), "utf8");
  const docsApp = path.resolve(import.meta.dir, "../examples/docs-site");
  const docsTemplate = await readFile(path.join(docsApp, "page.template"), "utf8");
  const docs = await readSite();
  const docsHtml = renderPage(docsTemplate, docs, docs.pages.find((page) => page.route === "index")!, "test");
  for (const app of [
    { id: "issue10_home", name: "Konae home", html: homeHtml, spanishHtml: spanishHomeHtml, cardPath: "../examples/home/public/card.png" },
    { id: "issue10_docs", name: "Konae docs", html: docsHtml, cardPath: "../examples/docs-site/public/card.png" },
  ]) {
    const card = encodeBase64(await readFile(path.resolve(import.meta.dir, app.cardPath)));
    await kernel.owner.call("project_create", { projectId: app.id, name: app.name });
    const { receipt } = await release(kernel.owner, {
      project: { id: app.id, name: app.name, slug: app.id, manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "", "actions.json": JSON.stringify({ version: 1, actions: {} }) } },
      visibility: "public",
      artifact: { version: 1, entrypoint: "index.html", files: { "index.html": app.html, ...(app.spanishHtml ? { "es/index.html": app.spanishHtml } : {}), "card.png": card }, encodings: { "card.png": "base64" } },
    }, app.id);
    const offState = await kernel.owner.call<any>("modules_read", { projectId: receipt.id });
    await kernel.owner.call("module_set", { projectId: receipt.id, module: "seo", on: true, options: { image: "card.png" }, expected: offState.modules.seo.state });
    const html = await curlApp(receipt.hostname, "/");
    expect(html).toContain(`property="og:image" content="${receipt.url}/card.png"`);
    expect(html).toContain(app.id === "issue10_home" ? "Describe an app." : "Konae");
    if (app.spanishHtml) {
      const spanishHtml = await curlApp(receipt.hostname, "/es/");
      expect(spanishHtml).toContain('<html lang="es">');
      expect(spanishHtml).toContain("Describe una app.");
    }
    const image = await appFetch(kernel, receipt.hostname, "/card.png");
    expect(image.headers.get("content-type")).toContain("image/png");
  }
});
