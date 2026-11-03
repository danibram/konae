import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

// The environment table in docs/operate.md is the operator's list of what the Kernel reads.
// A variable the Kernel reads and the page does not name is a setting nobody can find.

const root = path.resolve(import.meta.dir, "../..");
const crates = ["kernel", "konae"].map((crate) => path.join(root, "kernel-rs", crate, "src"));

test("docs/operate.md names every KONAE_ variable the Kernel reads", () => {
  const read = new Set(
    crates.flatMap((dir) => readdirSync(dir)
      .filter((name) => name.endsWith(".rs"))
      .flatMap((name) => {
        const source = readFileSync(path.join(dir, name), "utf8").split("#[cfg(test)]")[0];
        return [...source.matchAll(/"(KONAE_[A-Z0-9_]+)"/g)].map((match) => match[1]);
      })),
  );
  expect(read.size).toBeGreaterThan(30);
  const page = readFileSync(path.join(root, "docs/operate.md"), "utf8");
  expect([...read].filter((name) => !page.includes(`\`${name}\``)).sort()).toEqual([]);
});
