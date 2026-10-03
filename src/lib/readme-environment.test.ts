import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

// The README's environment table is the operator's list of what the Kernel reads.
// A variable the Kernel reads and the README does not name is a setting nobody can find.

const root = path.resolve(import.meta.dir, "../..");
const kernel = path.join(root, "kernel-rs/src");

test("the README names every LITTLEBOX_ variable the Kernel reads", () => {
  const read = new Set(
    readdirSync(kernel)
      .filter((name) => name.endsWith(".rs"))
      .flatMap((name) => {
        const source = readFileSync(path.join(kernel, name), "utf8").split("#[cfg(test)]")[0];
        return [...source.matchAll(/"(LITTLEBOX_[A-Z0-9_]+)"/g)].map((match) => match[1]);
      }),
  );
  expect(read.size).toBeGreaterThan(30);
  const readme = readFileSync(path.join(root, "README.md"), "utf8");
  expect([...read].filter((name) => !readme.includes(`\`${name}\``)).sort()).toEqual([]);
});
