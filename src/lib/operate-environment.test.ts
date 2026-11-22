import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";


const root = path.resolve(import.meta.dir, "../..");
const crates = ["kernel", "konae"].map((crate) => path.join(root, "kernel-rs", crate, "src"));

test("docs/operate.md names every KONAE_ variable the Kernel reads", () => {
  const read = new Set(
    crates.flatMap((dir) => readdirSync(dir, { recursive: true })
      .filter((name) => name.endsWith(".rs") && !name.split(path.sep).includes("tests")
        && path.basename(name) !== "tests.rs" && !name.endsWith("_tests.rs"))
      .flatMap((name) => {
        const source = readFileSync(path.join(dir, name), "utf8").split("#[cfg(test)]")[0];
        return [...source.matchAll(/"(KONAE_[A-Z0-9_]+)"/g)].map((match) => match[1]);
      })),
  );
  expect(read.size).toBeGreaterThan(30);
  const page = readFileSync(path.join(root, "docs/operate.md"), "utf8");
  expect([...read].filter((name) => !page.includes(`\`${name}\``)).sort()).toEqual([]);
});
