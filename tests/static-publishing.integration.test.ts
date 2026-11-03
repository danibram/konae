import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal } from "../scripts/konae";
import { McpClient } from "../src/mcp/client";
import { routingMessage } from "../src/lib/static-artifact.js";

// The static publishing contract on a real Kernel: a multi-page Capsule built
// by `konae build`, served with pages, 404, extensionless paths, content
// types, HEAD and the Content Security Policy on every page.

setDefaultTimeout(60_000);

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let owner: McpClient;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
const hosts: Record<string, string> = {};
const logo = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0xff, 0xfe]);
const font = new Uint8Array([0x77, 0x4f, 0x46, 0x32, 0x00, 0x01, 0x00, 0x00, 0xc3, 0x28]);
const inline = "document.documentElement.dataset.inline = 'ran';";

const page = (title: string, body: string) =>
  `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${title}</title><link rel="stylesheet" href="/styles.css"></head><body>${body}</body></html>`;
const capsule: Record<string, string | Uint8Array> = {
  "index.html": page("Home", '<img src="/logo.png" alt=""><a href="/about">About</a>'),
  "about.html": page("About", `<script>${inline}</script><script type="module" src="/app.js"></script>`),
  "pages/guide/install.html": page("Install", '<img src="../logo.png" alt="">'),
  "404.html": page("Lost", "<p>No page here.</p>"),
  "styles.css": "@font-face { font-family: Body; src: url(/fonts/body.woff2) } body { font-family: Body }",
  "app.js": "import { greet } from './lib/greet.js';\ngreet();\n",
  "lib/greet.js": "export function greet() { document.body.dataset.greeted = 'yes'; }\n",
  "logo.png": logo,
  "public/fonts/body.woff2": font,
  "public/sitemap.xml": "<?xml version=\"1.0\"?><urlset/>",
  "public/robots.txt": "User-agent: *\n",
  "public/site.webmanifest": "{\"name\":\"Pages\"}",
  "public/data/items.json": "[1,2,3]",
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);",
  "actions.json": "{\"version\":1,\"actions\":{}}",
};

async function capsuleOnDisk(name: string, files: Record<string, string | Uint8Array>) {
  // An app folder: the sources go in app/.
  const directory = path.join(root, "capsules", name);
  for (const [file, contents] of Object.entries(files)) {
    await mkdir(path.dirname(path.join(directory, "app", file)), { recursive: true });
    await writeFile(path.join(directory, "app", file), contents);
  }
  return directory;
}

async function release(id: string, artifact: object, manifest: Record<string, unknown> = {}) {
  const project = { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public", ...manifest } };
  await owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const files = { "schema.sql": capsule["schema.sql"], "actions.json": capsule["actions.json"] };
  const transfer = await owner.upload(new TextEncoder().encode(JSON.stringify({ project: { ...project, files }, visibility: "public", artifact })), id);
  try {
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    const receipt = await owner.call<{ hostname: string }>("release_apply", { planId: plan.planId });
    hosts[id] = receipt.hostname;
    return receipt;
  } finally { await owner.cancel(transfer.transferId); }
}

function get(id: string, pathname: string, method = "GET") {
  return fetch(url + pathname, { method, redirect: "manual", headers: { host: hosts[id] } });
}

async function title(response: Response) {
  return (await response.text()).match(/<title>([^<]*)<\/title>/)?.[1];
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-static-"));
  const port = Number(process.env.KONAE_STATIC_TEST_PORT ?? 19810 + Math.floor(Math.random() * 40));
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn(["kernel-rs/target/debug/konae"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
      KONAE_DATA_DIR: path.join(root, "data"), KONAE_ADMIN_KEY: setupKey, KONAE_HOST_SUFFIX: "localhost",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    if (kernel.exitCode !== null) throw Error("The static publishing Kernel stopped before startup. Set KONAE_STATIC_TEST_PORT to a free port.");
    try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
    if (!ready) await Bun.sleep(100);
  }
  expect(ready).toBe(true);
  const signup = await fetch(`${url}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@static.test", password, setupKey }),
  });
  expect(signup.status).toBe(200);
  const cookie = signup.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const { artifact } = await buildLocal(await capsuleOnDisk("pages", capsule));
  await release("static_pages", artifact);
}, 120_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

describe("a multi-page artifact under pages routing", () => {
  test("each page answers at its file, and an extensionless path answers with its .html page", async () => {
    for (const [pathname, expected] of [
      ["/", "Home"], ["/index.html", "Home"], ["/about", "About"], ["/about.html", "About"],
      ["/guide/install", "Install"], ["/guide/install.html", "Install"],
    ]) {
      const response = await get("static_pages", pathname);
      expect(response.status, pathname).toBe(200);
      expect(response.headers.get("content-type"), pathname).toBe("text/html; charset=utf-8");
      expect(await title(response), pathname).toBe(expected);
    }
  });

  test("a path the artifact does not hold answers 404 with 404.html, never the home page", async () => {
    for (const pathname of ["/missing-page", "/about/", "/photos/2026/fog.jpg", "/guide", "/feed"]) {
      const response = await get("static_pages", pathname);
      expect(response.status, pathname).toBe(404);
      expect(await title(response), pathname).toBe("Lost");
    }
  });

  test("a path under /_konae is never an app file", async () => {
    const response = await get("static_pages", "/_konae/about.html");
    expect(response.status).not.toBe(200);
    expect(await response.text()).not.toContain("<title>About</title>");
  });

  test("every file answers with its content type, and binary files keep their bytes", async () => {
    for (const [pathname, type] of [
      ["/styles.css", "text/css; charset=utf-8"], ["/app.js", "text/javascript; charset=utf-8"], ["/runtime.js", "text/javascript; charset=utf-8"],
      ["/logo.png", "image/png"], ["/fonts/body.woff2", "font/woff2"], ["/sitemap.xml", "application/xml; charset=utf-8"],
      ["/robots.txt", "text/plain; charset=utf-8"], ["/site.webmanifest", "application/manifest+json"], ["/data/items.json", "application/json"],
    ]) {
      const response = await get("static_pages", pathname);
      expect(response.status, pathname).toBe(200);
      expect(response.headers.get("content-type"), pathname).toBe(type);
      expect(response.headers.get("x-content-type-options"), pathname).toBe("nosniff");
    }
    expect(new Uint8Array(await (await get("static_pages", "/logo.png")).arrayBuffer())).toEqual(logo);
    expect(new Uint8Array(await (await get("static_pages", "/fonts/body.woff2")).arrayBuffer())).toEqual(font);
    expect(await (await get("static_pages", "/app.js")).text()).toContain("greeted");
  });

  test("Capsule files no page loads are not published", async () => {
    for (const pathname of ["/schema.sql", "/actions.json", "/pages/guide/install.html", "/public/robots.txt"]) {
      expect((await get("static_pages", pathname)).status, pathname).toBe(404);
    }
  });

  test("HEAD on a static file answers its headers with no body", async () => {
    const response = await get("static_pages", "/styles.css", "HEAD");
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/css; charset=utf-8");
    expect(Number(response.headers.get("content-length"))).toBe((capsule["styles.css"] as string).length);
    expect(await response.text()).toBe("");
    const missing = await get("static_pages", "/missing-page", "HEAD");
    expect(missing.status).toBe(404);
    expect(await missing.text()).toBe("");
  });

  test("the second page runs under the app's Content Security Policy, its inline script allowed by hash", async () => {
    const response = await get("static_pages", "/about");
    const policy = response.headers.get("content-security-policy") ?? "";
    const hash = `'sha256-${createHash("sha256").update(inline).digest("base64")}'`;
    // The page's own script and the runtime loader the build puts in its head are both listed by hash.
    expect(policy).toContain("script-src 'self' 'sha256-");
    expect(policy).toContain(hash);
    expect(policy).not.toContain("'unsafe-inline' 'sha256");
    expect(policy).toContain("frame-ancestors 'none'");
    expect(policy).toContain("connect-src 'self'");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    const notFound = await get("static_pages", "/missing-page");
    expect(notFound.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
  });
});

describe("other answers for a missing path", () => {
  test("with no 404.html the Kernel answers its own small 404 page", async () => {
    const { artifact } = await buildLocal(await capsuleOnDisk("bare", { "index.html": page("Bare home", ""), "styles.css": "", "actions.json": capsule["actions.json"] }));
    await release("static_bare", artifact);
    const response = await get("static_bare", "/missing-page");
    expect(response.status).toBe(404);
    expect(response.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(response.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
    const body = await response.text();
    expect(body).toContain("<title>Page not found</title>");
    expect(body).not.toContain("Bare home");
  });

  test('"routing": "spa" serves index.html for every path the artifact does not hold', async () => {
    const files = { "index.html": page("Single page", ""), "styles.css": "", "actions.json": capsule["actions.json"] };
    const directory = await capsuleOnDisk("spa", files);
    await writeFile(path.join(directory, "konae.json"), JSON.stringify({ id: "static_spa", name: "spa", slug: "spa", visibility: "public", signupPolicy: { mode: "open" }, signInMethods: ["password"], kernel: "0.1.0", manifest: { routing: "spa" } }));
    const { artifact } = await buildLocal(directory);
    await release("static_spa", artifact, { routing: "spa" });
    for (const pathname of ["/", "/boards/7", "/about", "/about/"]) {
      const response = await get("static_spa", pathname);
      expect(response.status, pathname).toBe(200);
      expect(await title(response), pathname).toBe("Single page");
    }
    expect((await get("static_spa", "/styles.css")).headers.get("content-type")).toBe("text/css; charset=utf-8");
  });
});

describe("the Kernel refuses at release_plan", () => {
  test("a routing value other than pages or spa, with the same words as the build", async () => {
    const artifact = { version: 1, entrypoint: "index.html", files: { "index.html": page("x", "") } };
    await expect(release("static_bad_routing", artifact, { routing: "history" })).rejects.toThrow(routingMessage);
  });

  test("an encoding that is not base64, names no file, or does not decode", async () => {
    const files = { "index.html": page("x", ""), "logo.png": "not base64!" };
    await expect(release("static_bad_encoding_1", { version: 1, entrypoint: "index.html", files, encodings: { "logo.png": "base64" } }))
      .rejects.toThrow("Artifact file logo.png is marked base64 but is not valid base64.");
    await expect(release("static_bad_encoding_2", { version: 1, entrypoint: "index.html", files, encodings: { "logo.png": "hex" } }))
      .rejects.toThrow("The artifact encodes logo.png as hex. The only encoding is base64.");
    await expect(release("static_bad_encoding_3", { version: 1, entrypoint: "index.html", files, encodings: { "gone.png": "base64" } }))
      .rejects.toThrow("The artifact names an encoding for gone.png, which it does not hold.");
  });
});
