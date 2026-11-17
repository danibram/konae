import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { sdkRequired, sourcesUseActions, sourcesUseAuth, sourcesUseFiles } from "./module-use";

const cases = JSON.parse(readFileSync("kernel-rs/kernel/src/module-use-cases.json", "utf8")) as Array<{ case: string; sources: string[]; artifact: string[]; usesFiles?: boolean; usesAuth?: boolean }>;

test("file module source detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesFiles !== undefined) {
    expect(sourcesUseFiles(item.sources), `${item.case} in sources`).toBe(item.usesFiles);
    expect(sourcesUseFiles(item.artifact), `${item.case} in artifact`).toBe(item.usesFiles);
  }
});

test("actions module use detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesActions !== undefined) {
    expect(sourcesUseActions(item.sources), `${item.case} in sources`).toBe(item.usesActions);
    if (item.artifact.length) expect(sourcesUseActions(item.artifact), `${item.case} in artifact`).toBe(item.artifact.some((source) => source.includes("miniapp.actions")));
  }
});

test("the SDK follows decided modules or the modules a new app uses", () => {
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, ["<main>Static</main>"], "public")).toBe(false);
  expect(sdkRequired(undefined, ["<main>Static</main>"])).toBe(false);
  expect(sdkRequired(undefined, ['{"version":1,"actions":{"notes.list":{"kind":"query"}}}'], "public")).toBe(true);
  expect(sdkRequired({ actions: { on: false }, auth: { on: false }, files: { on: false } }, ['{"version":1,"actions":{"notes.list":{"kind":"query"}}}'], "public")).toBe(false);
});

test("auth module use detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesAuth !== undefined) {
    expect(sourcesUseAuth(item.sources), `${item.case} in sources`).toBe(item.usesAuth);
    if (item.artifact.length > 0) expect(sourcesUseAuth(item.artifact), `${item.case} in artifact`).toBe(item.usesAuth);
  }
});
