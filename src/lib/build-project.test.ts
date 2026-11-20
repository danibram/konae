import { expect, test } from "bun:test";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { buildProjectEntries, buildProjectEntriesFromDisk } from "./build-project";

const manifest = (files: string[]) => JSON.stringify({ version: 1, files });
const source = (files: string[]) => ({ files: {
  "build.json": manifest(files),
  ...Object.fromEntries(files.map((file) => [`build/${file}`, `contents of ${file}`])),
} });

test("projects only declared build sources to the folder root and closes relative imports", () => {
  const entries = buildProjectEntries({ files: {
    "build.json": manifest(["package.json", "package-lock.json", "scripts/app-build.mjs", "scripts/helper.mjs"]),
    "build/package.json": '{"scripts":{"build":"node scripts/app-build.mjs"}}',
    "build/package-lock.json": "{}",
    "build/scripts/app-build.mjs": 'import "./helper.mjs";',
    "build/scripts/helper.mjs": "export const built = true;",
    "build/scripts/unlisted.mjs": "this file must stay app source only",
  } });
  expect(entries).toEqual([
    { path: "package.json", contents: '{"scripts":{"build":"node scripts/app-build.mjs"}}' },
    { path: "package-lock.json", contents: "{}" },
    { path: "scripts/app-build.mjs", contents: 'import "./helper.mjs";' },
    { path: "scripts/helper.mjs", contents: "export const built = true;" },
  ]);
});

test("rejects paths that escape or replace app data, release state, or credentials", () => {
  for (const forbidden of [
    "../.env", "/tmp/secrets.json", "C:/app/package.json", "scripts/../konae.json", "app/functions.json",
    "dist/index.html", "server/manifest.json", "data/database.sqlite", "releases/latest.json", ".konae/private.json",
    ".git/config", ".npmrc", "scripts/credentials.json", "scripts/session-token.json", "scripts/private.pem",
  ]) {
    expect(() => buildProjectEntries(source([forbidden]))).toThrow("forbidden or non-normalized path");
  }
});

test("refuses missing build sources and unresolved relative imports", () => {
  const required = ["package.json", "package-lock.json", "scripts/app-build.mjs"];
  expect(() => buildProjectEntries({ files: { "build.json": manifest(required), "build/package.json": '{"scripts":{"build":"node scripts/app-build.mjs"}}', "build/package-lock.json": "{}" } })).toThrow("names missing or invalid source");
  expect(() => buildProjectEntries({ files: {
    "build.json": manifest(required),
    "build/package.json": '{"scripts":{"build":"node scripts/app-build.mjs"}}',
    "build/package-lock.json": "{}",
    "build/scripts/app-build.mjs": 'import "./missing.mjs";',
  } })).toThrow("imports \"./missing.mjs\"");
});

test("refuses a symlink anywhere in the declared build source path", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "konae-build-project-"));
  const outside = await mkdtemp(path.join(os.tmpdir(), "konae-build-outside-"));
  try {
    await mkdir(path.join(root, "build", "scripts"), { recursive: true });
    await writeFile(path.join(root, "build.json"), manifest(["scripts/app-build.mjs"]));
    await writeFile(path.join(outside, "app-build.mjs"), "console.log('outside');");
    await symlink(outside, path.join(root, "build", "scripts", "linked"), "dir");
    await symlink(path.join(outside, "app-build.mjs"), path.join(root, "build", "scripts", "app-build.mjs"));
    await expect(buildProjectEntriesFromDisk(root)).rejects.toThrow("traverses a symbolic link");
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(outside, { recursive: true, force: true });
  }
});
