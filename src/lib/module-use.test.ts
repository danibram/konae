import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { sourcesUseAuth, sourcesUseFiles } from "./module-use";

const cases = JSON.parse(readFileSync("kernel-rs/kernel/src/module-use-cases.json", "utf8")) as Array<{ case: string; sources: string[]; artifact: string[]; usesFiles?: boolean; usesAuth?: boolean }>;

test("file module source detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesFiles !== undefined) {
    expect(sourcesUseFiles(item.sources), `${item.case} in sources`).toBe(item.usesFiles);
    expect(sourcesUseFiles(item.artifact), `${item.case} in artifact`).toBe(item.usesFiles);
  }
});

test("auth module use detection matches the Kernel cases", () => {
  for (const item of cases) if (item.usesAuth !== undefined) {
    expect(sourcesUseAuth(item.sources), `${item.case} in sources`).toBe(item.usesAuth);
    if (item.artifact.length > 0) expect(sourcesUseAuth(item.artifact), `${item.case} in artifact`).toBe(item.usesAuth);
  }
});
