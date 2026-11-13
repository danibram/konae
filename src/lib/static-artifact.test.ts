import { describe, expect, mock, test } from "bun:test";
import * as esbuild from "esbuild";
import { readFileSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal } from "../../scripts/konae";
import { createDefaultManifest, normalizeManifest } from "../deployments/manifest";
import type { MiniAppProject } from "./types";
import { assembleStaticArtifact, basePath, checkRouting, isDocument, pathPrefixPattern, resolveReference, routingMessage, runtimeFile, runtimeLoaderScript, servedPath, splitFragmentHead, stripModulePaths } from "./static-artifact.js";

// esbuild-wasm starts only in a browser. The editor's build runs here on native
// esbuild with the editor's own plugin, which resolves every import from the
// project's files and never from disk.
mock.module("esbuild-wasm", () => ({ initialize: async () => {}, build: esbuild.build }));
const { buildProjectInBrowser } = await import("../runtime/browser-build");

const runtime = "window.miniapp = {};";
const page = (title: string, body = "") => `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${title}</title></head><body>${body}</body></html>`;

/** The module as both doors call it, over files held in memory. */
function assemble(files: Record<string, string | Uint8Array>, options: { routing?: unknown; bundle?: (path: string) => { script: string; styles: string } } = {}) {
  return assembleStaticArtifact({
    paths: Object.keys(files),
    read: async (file) => files[file],
    compile: async () => ({ styles: typeof files["styles.css"] === "string" ? files["styles.css"] : "", script: "console.log('app')" }),
    bundle: async (file) => options.bundle?.(file) ?? { script: `/* bundled ${file} */`, styles: "" },
    runtime,
    routing: options.routing,
  });
}

describe("which Capsule files become pages", () => {
  test("an .html at the root or under pages/ is a page; pages/ is dropped from the served path", () => {
    expect(servedPath("index.html")).toBe("index.html");
    expect(servedPath("about.html")).toBe("about.html");
    expect(servedPath("pages/guide/install.html")).toBe("guide/install.html");
    expect(servedPath("guide/install.html")).toBeNull();
    expect(servedPath("public/robots.txt")).toBe("robots.txt");
    expect(servedPath("styles.css")).toBeNull();
  });

  test("a page that starts with a doctype or <html> is a document; anything else is a fragment", () => {
    expect(isDocument("<!doctype html><p>x</p>")).toBe(true);
    expect(isDocument("  <!-- note -->\n<HTML lang=en>")).toBe(true);
    expect(isDocument("<main id=app></main>")).toBe(false);
  });

  test("a reference resolves against the served path of the page that names it", () => {
    expect(resolveReference("../logo.svg", "guide/install.html")).toBe("logo.svg");
    expect(resolveReference("/styles.css?v=2#x", "guide/install.html")).toBe("styles.css");
    expect(resolveReference("img/a%20b.png", "index.html")).toBe("img/a b.png");
    expect(resolveReference("docs/", "index.html")).toBe("docs/index.html");
    for (const outside of ["https://cdn.example/x.js", "//cdn.example/x.js", "data:image/png;base64,AA", "#top", "mailto:a@b", "/_konae/files/1"]) {
      expect(resolveReference(outside, "index.html")).toBeNull();
    }
  });
});

describe("the artifact", () => {
  test("holds every page with the SDK, the files the pages load, and public/ as is", async () => {
    const logo = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x00, 0xff]);
    const artifact = await assemble({
      "index.html": page("Home", '<img src="/logo.png" srcset="/logo.png 1x, img/wide.png 2x"><link rel="alternate" href="/feed.json"><a href="/nowhere">x</a>'),
      "about.html": page("About", '<link rel="stylesheet" href="styles.css"><script type="module" src="/app.js"></script>'),
      "pages/guide/install.html": page("Install", '<link rel="icon" href="../logo.png">'),
      "404.html": page("Not here"),
      "styles.css": "body { background: url('/img/paper.png') } @import 'theme.css';",
      "theme.css": "h1 { color: red }",
      "img/paper.png": "paper",
      "img/wide.png": "wide",
      "logo.png": logo,
      "app.js": "import './lib.js'",
      "lib.js": "export {}",
      "public/robots.txt": "User-agent: *",
      "public/sitemap.xml": "<urlset/>",
      "schema.sql": "CREATE TABLE secret(x)",
      "actions.json": "{}",
      "notes/draft.html": page("Draft"),
    });
    expect(Object.keys(artifact.files).sort()).toEqual([
      "404.html", "about.html", "app.js", "guide/install.html", "img/paper.png", "img/wide.png", "index.html",
      "logo.png", "robots.txt", runtimeFile, "sitemap.xml", "styles.css", "theme.css",
    ]);
    expect(artifact.encodings).toEqual({ "logo.png": "base64" });
    expect(artifact.files["logo.png"]).toBe(btoa(String.fromCharCode(...logo)));
    expect(artifact.files["app.js"]).toBe("/* bundled app.js */");
    expect(artifact.files[runtimeFile]).toBe(runtime);
    expect(artifact.files["about.html"]).toStartWith(`<!doctype html><html lang="en"><head>${runtimeLoaderScript()}<meta charset="utf-8">`);
  });

  test("a page that already loads /runtime.js gets the loader in place of its tag, once", async () => {
    const artifact = await assemble({ "index.html": page("Home", '<script src="/runtime.js"></script><script type="module" src="app.js"></script>'), "app.js": "" });
    expect(artifact.files["index.html"]).not.toContain('<script src="/runtime.js"></script>');
    expect(artifact.files["index.html"].match(/runtime\.js/g)).toHaveLength(1);
    expect(artifact.files["index.html"]).toContain('<script type="module" src="app.js">');
  });

  test("a fragment page runs in the Capsule document with the SDK inline, as before", async () => {
    const artifact = await assemble({ "index.html": "<main id=app></main>", "app.js": "", "styles.css": "p{}" });
    expect(Object.keys(artifact.files)).toEqual(["index.html"]);
    expect(artifact.files["index.html"]).toContain("<script data-konae-runtime>window.miniapp = {};</script>");
    expect(artifact.files["index.html"]).toContain("<style>p{}</style>");
  });

  test("a fragment that opens with a <title> and an icon <link> gets them in the head, and its icon file ships", async () => {
    const fragment = '<title>Starter</title>\n<link rel="icon" href="favicon.svg" type="image/svg+xml">\n<main id=app><svg><title>Close</title></svg></main>';
    const artifact = await assemble({ "index.html": fragment, "favicon.svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>", "app.js": "" });
    const html = artifact.files["index.html"];
    const [head, body] = html.split("</head>");
    expect(head).toContain('<title>Starter</title><link rel="icon" href="favicon.svg" type="image/svg+xml"><style>');
    expect(body).not.toContain("<title>Starter</title>");
    expect(body).not.toContain('rel="icon"');
    expect(body).toContain("<main id=app><svg><title>Close</title></svg></main>");
    expect(artifact.files["favicon.svg"]).toContain("<svg");
  });

  test("a data: SVG icon in a fragment is hoisted and needs no file", async () => {
    const icon = '<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22/%3E">';
    const artifact = await assemble({ "index.html": `${icon}<main id=app></main>`, "app.js": "" });
    expect(artifact.files["index.html"].split("</head>")[0]).toContain(icon);
    expect(Object.keys(artifact.files)).toEqual(["index.html"]);
  });

  test("only the elements a fragment opens with move: a stylesheet link or a later title stays in the body", () => {
    expect(splitFragmentHead('<link rel="stylesheet" href="x.css"><title>A</title><main></main>')).toEqual({ head: "", body: '<link rel="stylesheet" href="x.css"><title>A</title><main></main>' });
    expect(splitFragmentHead("<main></main><title>A</title>")).toEqual({ head: "", body: "<main></main><title>A</title>" });
    expect(splitFragmentHead('<!-- top -->\n<title>A</title><link rel="apple-touch-icon" href="/t.png"><main></main>')).toEqual({ head: '<title>A</title><link rel="apple-touch-icon" href="/t.png">', body: "<main></main>" });
  });

  test("a Capsule without index.html gets the default fragment home page", async () => {
    const artifact = await assemble({ "about.html": page("About"), "app.js": "" });
    expect(artifact.files["index.html"]).toContain("<main id=app></main>");
    expect(artifact.files["about.html"]).toContain("<title>About</title>");
  });
});

describe("module path comments", () => {
  test("drops a line that names a Capsule file, from either builder, and keeps every other comment", () => {
    const held = new Set(["app.js", "lib/a.js", "lib/b.css"]);
    expect(stripModulePaths("// lib/a.js\nvar a = 1;\n\n// konae:app.js\nconsole.log(a);\n", held)).toBe("var a = 1;\n\nconsole.log(a);\n");
    expect(stripModulePaths("/* lib/b.css */\np { margin: 0 }\n", held)).toBe("p { margin: 0 }\n");
    const kept = "// ../../tmp/other.js\n// see app.js\n/*! license */\nx();\n";
    expect(stripModulePaths(kept, held)).toBe(kept);
  });
});

describe("refusals", () => {
  const refused = (files: Record<string, string>, message: string) => expect(assemble(files)).rejects.toThrow(message);

  test("a reference to a file the Capsule does not hold names the page and the path", async () => {
    await refused({ "about.html": page("About", '<img src="/photos/fog.jpg">') }, "about.html references /photos/fog.jpg, but the Capsule holds no photos/fog.jpg.");
    await refused({ "index.html": page("Home", '<link rel="stylesheet" href="site.css">'), "site.css": "a { background: url(gone.png) }" },
      "site.css references gone.png, but the Capsule holds no gone.png.");
    await refused({ "pages/guide/a.html": page("A", '<script src="../../x.js"></script>') }, "guide/a.html references ../../x.js, which leaves the Capsule.");
  });

  test("two files that would publish at one path", async () => {
    await refused({ "about.html": page("A"), "pages/about.html": page("B") }, "about.html and pages/about.html would both publish at /about.html. Keep one.");
    await refused({ "index.html": page("A"), "public/index.html": "x" }, "index.html and public/index.html would both publish at /index.html. Keep one.");
  });

  test("a path the Kernel keeps, and the SDK's name", async () => {
    await refused({ "public/_konae/x.txt": "x" }, "public/_konae/x.txt would publish at /_konae/x.txt, under /_konae, which the Kernel keeps.");
    await refused({ "pages/oauth/callback.html": page("x") }, "pages/oauth/callback.html would publish at /oauth/callback.html, under /oauth, which the Kernel keeps.");
    await refused({ "index.html": page("x"), "runtime.js": "" }, "The Capsule holds runtime.js, the name the build gives the Konae SDK. Rename it.");
    await refused({ "public/runtime.js": "" }, "public/runtime.js would publish at /runtime.js, where the build puts the Konae SDK. Rename it.");
  });

  test("a module that is not JavaScript, or that imports CSS", async () => {
    await refused({ "index.html": page("x", '<script type="module" src="/src/main.ts"></script>'), "src/main.ts": "" },
      "index.html loads /src/main.ts as a module. A page loads JavaScript, so name a .js or .mjs file and let the build compile it.");
    await expect(assemble({ "index.html": page("x", '<script type="module" src="app.js"></script>'), "app.js": "" }, { bundle: () => ({ script: "", styles: "p{}" }) }))
      .rejects.toThrow("app.js imports CSS. Link the stylesheet from the page instead.");
  });

  test("a whole document that loads no script, when nothing else runs the entry", async () => {
    await refused({ "index.html": page("App", '<div id="root"></div><script src="/runtime.js"></script>'), "src/main.tsx": "", "src/main.ts": "" },
      'index.html is a whole document that loads no script, so src/main.tsx never runs and the page stays empty. Drop its doctype and its <html>, <head> and <body> tags so the build puts src/main.tsx into it, or add a <script type="module" src> that names a .js file importing src/main.tsx.');
    await refused({ "pages/index.html": page("App"), "about.html": page("About"), "app.js": "" },
      'pages/index.html is a whole document that loads no script, so app.js never runs and the page stays empty. Drop its doctype and its <html>, <head> and <body> tags so the build puts app.js into it, or add <script type="module" src="app.js"></script> to it.');
  });

  test("a whole document with no script passes while the app runs elsewhere, or when there is no app", async () => {
    const passes = (files: Record<string, string>) => expect(assemble(files)).resolves.toBeDefined();
    // A fragment, or the default fragment a Capsule without index.html gets, runs the entry.
    await passes({ "index.html": "<div id=root></div>", "404.html": page("Not found"), "app.js": "" });
    await passes({ "about.html": page("About"), "app.js": "" });
    // A document that loads its own script, inline or by src, and a static 404 beside it.
    await passes({ "index.html": page("App", '<script type="module" src="app.js"></script>'), "404.html": page("Not found"), "app.js": "" });
    await passes({ "index.html": page("App", "<script>document.title = 'x'</script>"), "app.js": "" });
    // No entry: a page with no script is a static page.
    await passes({ "index.html": page("Chores") });
  });

  test("routing other than pages or spa", async () => {
    expect(checkRouting(undefined)).toBe("pages");
    expect(checkRouting("spa")).toBe("spa");
    expect(() => checkRouting("history")).toThrow(routingMessage);
    expect(routingMessage).toBe('Routing must be "pages" or "spa".');
    await expect(assemble({ "index.html": page("x") }, { routing: "SPA" })).rejects.toThrow(routingMessage);
  });
});

describe("the manifest keeps the routing mode", () => {
  test("pages and spa survive normalization; another value is dropped there and refused by the build", () => {
    expect(normalizeManifest({ ...createDefaultManifest(), routing: "spa" }).routing).toBe("spa");
    expect(normalizeManifest({ ...createDefaultManifest(), routing: "pages" }).routing).toBe("pages");
    expect(normalizeManifest({ ...createDefaultManifest(), routing: "history" }).routing).toBeUndefined();
    expect(normalizeManifest(createDefaultManifest()).routing).toBeUndefined();
  });
});

describe("the CLI and the editor build the same artifact", () => {
  const capsule = {
    "index.html": page("Home", '<link rel="stylesheet" href="/styles.css"><a href="/about">About</a>'),
    "about.html": page("About", '<script src="/runtime.js"></script><script type="module" src="/app.js"></script>'),
    "pages/guide/install.html": page("Install", '<link rel="stylesheet" href="../styles.css">'),
    "styles.css": "body { color: #222 }",
    "app.js": "const greeting = 'hi';\ndocument.title = greeting;\n",
    "public/robots.txt": "User-agent: *\n",
    "schema.sql": "",
    "actions.json": "{\"version\":1,\"actions\":{}}",
  };

  async function onDisk(files: Record<string, string>) {
    // An app folder: konae.json at the root, the sources in app/.
    const root = await mkdtemp(path.join(tmpdir(), "konae-static-"));
    for (const [file, contents] of Object.entries(files)) {
      const target = file === "konae.json" ? path.join(root, file) : path.join(root, "app", file);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, contents);
    }
    return root;
  }

  function project(files: Record<string, string>): MiniAppProject {
    return { files, manifest: createDefaultManifest() } as unknown as MiniAppProject;
  }

  test("the same pages, files and SDK from both doors", async () => {
    const root = await onDisk(capsule);
    try {
      const cli = (await buildLocal(root)).artifact;
      const editor = await buildProjectInBrowser(project(capsule));
      expect(Object.keys(cli.files).sort()).toEqual(["about.html", "app.js", "guide/install.html", "index.html", "robots.txt", "runtime.js", "styles.css"]);
      expect(Object.keys(editor.files).sort()).toEqual(Object.keys(cli.files).sort());
      for (const file of ["index.html", "about.html", "guide/install.html", "robots.txt", "styles.css", "runtime.js", "app.js"]) {
        expect(editor.files[file]).toBe(cli.files[file]);
      }
      expect(cli.files["index.html"]).toContain(`<head>${runtimeLoaderScript()}`);
      expect(cli.files["app.js"]).toContain("document.title = greeting");
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("the same Capsule built from two folders gives byte-identical files, with no folder named", async () => {
    const modular = {
      "index.html": "<main id=app></main>",
      "styles.css": "body { color: #222 }",
      "app.js": 'import { greet } from "./lib/greet.js";\nimport "./lib/card.css";\n//! Kept: a legal comment.\ndocument.title = greet("Ana");\n',
      "lib/greet.js": "export const greet = (name) => `Hello, ${name}`;\n",
      "lib/card.css": ".card { padding: 4px }\n",
    };
    const first = await onDisk(modular);
    const second = await onDisk(modular);
    try {
      const one = JSON.stringify((await buildLocal(first)).artifact);
      const two = JSON.stringify((await buildLocal(second)).artifact);
      expect(one).toBe(two);
      expect(one).not.toContain(path.basename(first));
      expect(one).not.toContain(path.basename(second));
      for (const comment of ["// app.js", "// lib/greet.js", "/* lib/card.css */"]) expect(one).not.toContain(comment);
      expect(one).toContain("Kept: a legal comment.");
      expect(one).toContain(".card {");
      // The editor names modules with its own namespace; its page carries the same script and styles.
      const editor = await buildProjectInBrowser(project(modular));
      expect(editor.files["index.html"]).toBe(JSON.parse(one).files["index.html"]);
    } finally {
      await rm(first, { recursive: true, force: true });
      await rm(second, { recursive: true, force: true });
    }
  });

  test("both refuse a missing reference with the same words", async () => {
    const broken = { ...capsule, "about.html": page("About", '<img src="/team.jpg"><script type="module" src="/app.js"></script>') };
    const root = await onDisk(broken);
    try {
      const message = "about.html references /team.jpg, but the Capsule holds no team.jpg.";
      await expect(buildLocal(root)).rejects.toThrow(message);
      await expect(buildProjectInBrowser(project(broken))).rejects.toThrow(message);
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("the CLI carries a binary file as base64; the editor holds text only and refuses the same reference", async () => {
    const withImage = { ...capsule, "about.html": page("About", '<img src="/team.png"><script type="module" src="/app.js"></script>') };
    const root = await onDisk(withImage);
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff]);
    await writeFile(path.join(root, "app", "team.png"), png);
    try {
      const cli = (await buildLocal(root)).artifact;
      expect(cli.encodings).toEqual({ "team.png": "base64" });
      expect(new Uint8Array(Buffer.from(cli.files["team.png"], "base64"))).toEqual(png);
      await expect(buildProjectInBrowser(project(withImage))).rejects.toThrow("about.html references /team.png, but the Capsule holds no team.png.");
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("both run src/main.tsx in a fragment page, and refuse a whole document that never loads it with the same words", async () => {
    const typed = { "index.html": "<div id=root></div>", "src/main.tsx": "const count: number = 2;\ndocument.title = String(count);\n" };
    const root = await onDisk(typed);
    try {
      const cli = (await buildLocal(root)).artifact;
      const editor = await buildProjectInBrowser(project(typed));
      expect(cli.files["index.html"]).toContain("document.title = String(count)");
      expect(editor.files["index.html"]).toContain("document.title = String(count)");
    } finally { await rm(root, { recursive: true, force: true }); }

    const blank = { ...typed, "index.html": page("App", '<div id="root"></div>') };
    const broken = await onDisk(blank);
    try {
      const message = "index.html is a whole document that loads no script, so src/main.tsx never runs and the page stays empty.";
      await expect(buildLocal(broken)).rejects.toThrow(message);
      await expect(buildProjectInBrowser(project(blank))).rejects.toThrow(message);
    } finally { await rm(broken, { recursive: true, force: true }); }
  });

  test("both refuse a routing value the Kernel would refuse", async () => {
    const root = await onDisk({ ...capsule, "konae.json": JSON.stringify({ id: "p", name: "p", slug: "p", visibility: "private", signupPolicy: { mode: "closed" }, signInMethods: ["password"], kernel: "0.1.0", manifest: { routing: "history" } }) });
    try {
      await expect(buildLocal(root)).rejects.toThrow(routingMessage);
      await expect(buildProjectInBrowser({ files: capsule, manifest: { ...createDefaultManifest(), routing: "history" } } as unknown as MiniAppProject)).rejects.toThrow(routingMessage);
    } finally { await rm(root, { recursive: true, force: true }); }
  });
});

describe("a path address finds its base path, and the SDK loader writes its tag under it", () => {
  test("basePath reads the first segment after /p/ and is empty everywhere else", () => {
    expect(basePath("/")).toBe("");
    expect(basePath("/p/notes")).toBe("/p/notes");
    expect(basePath("/p/notes/")).toBe("/p/notes");
    expect(basePath("/p/0b8c2f1e-7a7b-4c4e-9d1f-2a3b4c5d6e7f/about/deep.html")).toBe("/p/0b8c2f1e-7a7b-4c4e-9d1f-2a3b4c5d6e7f");
    expect(basePath("/p/")).toBe("");
    expect(basePath("/pages/notes")).toBe("");
    expect(basePath("/notes/p/x")).toBe("");
  });

  test("the loader writes the SDK tag under the base path, and at the root with none", () => {
    const script = runtimeLoaderScript();
    expect(script.startsWith("<script>") && script.endsWith("</script>")).toBe(true);
    const body = script.slice("<script>".length, -"</script>".length);
    expect(body).not.toContain("</script");
    const written = (pathname: string) => {
      const out: string[] = [];
      new Function("location", "document", body)({ pathname }, { write: (text: string) => out.push(text) });
      return out;
    };
    expect(written("/")).toEqual(['<script src="/runtime.js"></script>']);
    expect(written("/about.html")).toEqual(['<script src="/runtime.js"></script>']);
    expect(written("/p/notes")).toEqual(['<script src="/p/notes/runtime.js"></script>']);
    expect(written("/p/notes/guide/install.html")).toEqual(['<script src="/p/notes/runtime.js"></script>']);
  });

  test("the loader platform_guide gives a client without a shell is the one konae build writes", () => {
    const guide = readFileSync(path.resolve(import.meta.dir, "../../kernel-rs/konae/src/guide.rs"), "utf8");
    const copy = /const SDK_LOADER: &str = r#"(.*)"#;/.exec(guide)?.[1];
    expect(copy).toBe(runtimeLoaderScript());
  });

  test("the SDK and every inline page that builds a Kernel URL carry the same prefix rule", () => {
    const root = path.resolve(import.meta.dir, "../..");
    const rule = `/${pathPrefixPattern.source}/`;
    for (const file of ["src/mcp/runtime.js", "src/lib/static-artifact.js", "kernel-rs/kernel/src/accounts.rs"]) {
      expect(readFileSync(path.join(root, file), "utf8")).toContain(rule);
    }
  });
});

describe("the Validation Gate refuses a page that never runs the app, as the build does", () => {
  test("a whole-document index.html with no script beside app.js fails the Gate with the build's sentence", async () => {
    const { default: initSqlJs } = await import("sql.js");
    const { createCapsuleValidator } = await import("../agent/gate");
    const { starterProject } = await import("./starter");
    const validator = createCapsuleValidator(() => initSqlJs());
    const files = { ...starterProject.files, "index.html": page("App", '<div id="app"></div>') };
    const message = 'index.html is a whole document that loads no script, so app.js never runs and the page stays empty. Drop its doctype and its <html>, <head> and <body> tags so the build puts app.js into it, or add <script type="module" src="app.js"></script> to it.';
    const result = await validator.validate(files);
    expect(result.ok).toBe(false);
    expect(result.errors).toContainEqual({ stage: "actions", message });
    await expect(assemble(files)).rejects.toThrow(message);
    expect((await validator.validate(starterProject.files)).ok).toBe(true);
  });
});
