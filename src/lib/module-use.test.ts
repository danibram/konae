import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import * as esbuild from "esbuild";
import { sdkRequired, sourcesUseActions, sourcesUseAuth, sourcesUseCalls, sourcesUseFiles, sourcesUseHttp } from "./module-use";

const cases = JSON.parse(readFileSync("kernel-rs/kernel/src/module-use-cases.json", "utf8")) as Array<{ case: string; sources: string[]; sourceFiles?: Record<string, string>; artifact: string[]; usesFiles?: boolean; usesAuth?: boolean; usesActions?: boolean; usesHttp?: boolean; usesCalls?: boolean }>;

test("file module source detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesFiles !== undefined) {
    expect(sourcesUseFiles(item.sources), `${item.case} in sources`).toBe(item.usesFiles);
    expect(sourcesUseFiles(item.artifact), `${item.case} in artifact`).toBe(item.usesFiles);
  }
});

test("actions module use detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesActions !== undefined) {
    expect(sourcesUseActions(item.sourceFiles ?? item.sources), `${item.case} in sources`).toBe(item.usesActions);
    if (item.artifact.length) expect(sourcesUseActions(item.artifact), `${item.case} in artifact`).toBe(item.artifact.some((source) => source.includes("miniapp.actions")));
  }
});

test("http and calls use detection matches the Kernel cases", () => {
  for (const item of cases) {
    if (item.usesHttp !== undefined) expect(sourcesUseHttp(item.sourceFiles ?? item.sources), item.case).toBe(item.usesHttp);
    if (item.usesCalls !== undefined) expect(sourcesUseCalls(item.sourceFiles ?? item.sources), item.case).toBe(item.usesCalls);
  }
});

test("the SDK follows decided modules or the modules a new app uses", () => {
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, ["<main>Static</main>"], "public")).toBe(false);
  expect(sdkRequired(undefined, ["<main>Static</main>"])).toBe(false);
  expect(sdkRequired({ actions: { on: true } }, { "actions.json": '{"version":1,"actions":{"notes.list":{"kind":"query"}}}' }, "public")).toBe(false);
  expect(sdkRequired({ actions: { on: true } }, { "app.js": 'miniapp.actions.call("notes.list")' }, "public")).toBe(true);
  expect(sdkRequired({ actions: { on: true }, auth: { on: true }, files: { on: true } }, { "app.js": 'const { actions } = window.miniapp; actions.call("notes.list");' }, "public")).toBe(true);
  expect(sdkRequired({ actions: { on: true }, auth: { on: true }, files: { on: true } }, { "app.js": 'const api = window.miniapp; api.actions.call("notes.list");' }, "public")).toBe(true);
  expect(sdkRequired({ actions: { on: true }, auth: { on: true }, files: { on: true } }, { "app.js": 'window.miniapp?.actions.call("notes.list");' }, "public")).toBe(true);
  expect(sdkRequired({ auth: { on: true } }, { "app.js": 'window["miniapp"].auth.session()' }, "public")).toBe(true);
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, { "actions.json": '{"version":1,"actions":{"notes.list":{"kind":"query"}}}' }, "public")).toBe(false);
  expect(sdkRequired(undefined, ["miniapp.page.url('/about')"], "public")).toBe(true);
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, ["miniapp.page.url('/about')"], "public")).toBe(true);
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, ['window["miniapp"].page.url("/about")'], "public")).toBe(true);
});

test("sdkRequired stays self-contained when embedded and minified", async () => {
  const slot: { value?: typeof sdkRequired } = {};
  const { code } = await esbuild.transform(`slot.value = (${sdkRequired.toString()})`, { minify: true, target: "es2022" });
  new Function("slot", code)(slot);
  const embedded = slot.value!;
  expect(embedded({ actions: { on: true } }, { "app.js": 'miniapp.actions.run("notes.list")' }, "public")).toBe(true);
  expect(embedded({ auth: { on: true } }, { "app.js": 'window["miniapp"].auth.session()' }, "private")).toBe(true);
  expect(embedded({ files: { on: true } }, { "app.js": 'miniapp.files.list()' }, "public")).toBe(true);
  expect(embedded({ actions: { on: false }, auth: { on: false }, files: { on: false } }, { "index.html": "<main>Static</main>" }, "public")).toBe(false);
});

test("auth module use detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesAuth !== undefined) {
    expect(sourcesUseAuth(item.sources), `${item.case} in sources`).toBe(item.usesAuth);
    if (item.artifact.length > 0) expect(sourcesUseAuth(item.artifact), `${item.case} in artifact`).toBe(item.usesAuth);
  }
});
