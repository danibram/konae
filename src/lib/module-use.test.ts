import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { sourcesUseFiles } from "./module-use";

const cases = JSON.parse(readFileSync("kernel-rs/kernel/src/module-use-cases.json", "utf8")) as Array<{ case: string; sources: string[]; usesFiles: boolean }>;

test("file module source detection matches the Kernel cases", () => {
  for (const item of cases) expect(sourcesUseFiles(item.sources), item.case).toBe(item.usesFiles);
});
