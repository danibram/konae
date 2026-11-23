import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { defaultModuleStates, generatedFiles, type AppSettings } from "./app-folder";

test("module defaults preserve only module choices that already exist", () => {
  expect(defaultModuleStates()).toEqual({});
  expect(defaultModuleStates({ seo: { on: true, canonical: "path", image: "social.png" }, actions: { on: false }, http: { on: false }, calls: { on: false } })).toEqual({
    actions: { on: false },
    calls: { on: false },
    http: { on: false },
    seo: { on: true, canonical: "path", image: "social.png" },
  });
  expect(defaultModuleStates({ files: { on: true } })).toEqual({ files: { on: true, fileAccess: null } });
});

const settings: AppSettings = {
  id: "notes-1", name: "Notes", slug: "notes", visibility: "private",
  signupPolicy: { mode: "invitation" }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1, routing: "pages" },
};
const repo = path.resolve(import.meta.dir, "../..");
const parserSource = readFileSync(path.join(repo, "node_modules/@babel/parser/lib/index.js"), "utf8");
const parserLicense = readFileSync(path.join(repo, "node_modules/@babel/parser/LICENSE"), "utf8");
const generated = (input: Omit<Parameters<typeof generatedFiles>[0], "parserSource" | "parserLicense">) => generatedFiles({ ...input, parserSource, parserLicense });
const folderSettings = (hasFunctions = false) => generated({ settings, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "", localBuilder: "", hasFunctions });

test("the app folder's AGENTS.md tells a person or agent to escape text written into the page", () => {
  const files = generated({ settings, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "", localBuilder: "" });
  const guide = files.find((file) => file.path === "AGENTS.md")?.contents as string;
  expect(guide).toContain("`textContent` or the template's `escapeHtml`, in attributes too");
});

test("the folder's .env.example names the image of the Kernel version its konae.json carries", () => {
  const entries = (kernel: string) => generated({ settings: { ...settings, kernel }, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "", localBuilder: "" });
  const env = (kernel: string) => entries(kernel).find((file) => file.path === ".env.example")?.contents as string;
  expect(env("0.6.0")).toContain("\nKONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:0.6.0\n");
  expect(env("0.7.1")).toContain("\nKONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:0.7.1\n");
  expect(env("0.6.0")).toContain("A release publishes this tag for linux/amd64 and linux/arm64. A local image can replace it.");
  const compose = entries("0.6.0").find((file) => file.path === "compose.yaml")?.contents as string;
  expect(compose).toContain("${KONAE_KERNEL_IMAGE:?");
});

test("the generated compose stays unchanged when the published app has no functions", () => {
  const files = folderSettings();
  const compose = files.find((file) => file.path === "compose.yaml")?.contents as string;
  const env = files.find((file) => file.path === ".env.example")?.contents as string;
  expect(compose).not.toContain("KONAE_FUNCTIONS_RUNNER");
  expect(compose).not.toContain("cgroup:");
  expect(compose).not.toContain("cap_add:");
  expect(compose).not.toContain("/sys/fs/cgroup");
  expect(env).not.toContain("KONAE_FUNCTIONS_");
});

test("the generated compose configures the packaged runner and delegated cgroup for published functions", () => {
  const files = folderSettings(true);
  const compose = files.find((file) => file.path === "compose.yaml")?.contents as string;
  const env = files.find((file) => file.path === ".env.example")?.contents as string;
  expect(compose).toContain("KONAE_FUNCTIONS_RUNNER: ${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner}");
  expect(compose).toContain("KONAE_FUNCTIONS_CGROUP: ${KONAE_FUNCTIONS_CGROUP:-auto}");
  expect(compose).toContain('KONAE_FUNCTIONS_UID: ${KONAE_FUNCTIONS_UID:-10002}');
  expect(compose).toContain('KONAE_FUNCTIONS_GID: ${KONAE_FUNCTIONS_GID:-10002}');
  expect(compose).toContain("cgroup: private");
  expect(compose).toContain("- SYS_ADMIN\n      - SETUID\n      - SETGID");
  expect(compose).toContain("apparmor=unconfined");
  expect(compose).not.toContain("cgroup_parent");
  expect(compose).not.toContain("/sys/fs/cgroup");
  expect(compose.match(/volumes:/g)).toHaveLength(2);
  expect(env).toContain("KONAE_FUNCTIONS_RUNNER=/usr/local/lib/konae/functions-runner");
  expect(env).toContain("KONAE_FUNCTIONS_CGROUP=auto");
  expect(env).toContain("KONAE_FUNCTIONS_UID=10002");
  expect(env).toContain("KONAE_FUNCTIONS_GID=10002");
});

test("the folder's AGENTS.md says when dist/ needs npm install and when it does not", () => {
  const files = generated({ settings, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "", localBuilder: "" });
  const guide = files.find((file) => file.path === "AGENTS.md")?.contents as string;
  const build = JSON.parse(files.find((file) => file.path === "package.json")?.contents as string).scripts.build;
  expect(build).toBe("node scripts/build.mjs");
  expect(guide).toContain("run `npm install` once, then `npm run build`");
  expect(guide).toContain("`dev` writes it without `npm install`");
  expect(guide).toContain("Once that directory exists, the builder runs `npm run build`");
});

test("a generated scripts/build.mjs builds its folder", () => {
  const root = mkdtempSync(path.join(os.tmpdir(), "konae-generated-folder-"));
  try {
    const entries = generated({
      settings,
      actionsSource: `{"version":1,"actions":{}}`,
      schemaSource: "",
      runtime: readFileSync(path.join(repo, "src/mcp/runtime.js"), "utf8"),
      staticArtifact: readFileSync(path.join(repo, "src/lib/static-artifact.js"), "utf8"),
      localBuilder: readFileSync(path.join(repo, "src/lib/local-builder.js"), "utf8"),
    });
    for (const entry of entries) {
      const target = path.join(root, entry.path);
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, entry.contents);
    }
    mkdirSync(path.join(root, "app"), { recursive: true });
    writeFileSync(path.join(root, "konae.json"), JSON.stringify(settings));
    writeFileSync(path.join(root, "app/index.html"), '<!doctype html><main>Folder</main><script type="module" src="app.js"></script>');
    writeFileSync(path.join(root, "app/app.js"), "document.body.dataset.ready = 'yes';");
    symlinkSync(path.join(repo, "node_modules"), path.join(root, "node_modules"), "dir");
    expect(execFileSync("node", ["scripts/build.mjs"], { cwd: root, encoding: "utf8" })).toContain("Built dist/");
    expect(readFileSync(path.join(root, "dist/index.html"), "utf8")).toContain("Folder");
    expect(readFileSync(path.join(root, "dist/app.js"), "utf8")).toContain("dataset.ready");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
