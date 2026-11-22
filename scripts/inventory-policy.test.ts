import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";

const workspace = path.join(import.meta.dir, "../kernel-rs");
const catalog = readFileSync(path.join(workspace, "kernel/src/inventory.tsv"), "utf8");
const components = new Set(catalog.trimEnd().split("\n").slice(1).flatMap(row => row.split("\t")[2].split("|")));

function missingPolicies(source: string, layout: boolean) {
  const names = [...source.matchAll(/\bconst\s+\w*(?:DATABASE|_FILE)\s*:\s*&str\s*=\s*"([^"]+)"/g)].map(match => match[1]);
  if (layout) {
    for (const match of source.matchAll(/\.join\(\s*"([^"]+)"\s*\)/g)) names.push(match[1]);
  }
  return names.filter(name => !components.has(name));
}

test("declared store filenames and app layout paths have inventory policies", async () => {
  const missing: string[] = [];
  for (const binary of ["kernel", "konae"]) {
    const cwd = path.join(workspace, binary, "src");
    for await (const file of new Bun.Glob("**/*.rs").scan({ cwd })) {
      const source = readFileSync(path.join(cwd, file), "utf8");
      for (const name of missingPolicies(source, file === path.join("app_folders", "layout.rs"))) missing.push(`${binary}/${file}: ${name}`);
    }
  }
  expect(missing.sort()).toEqual([]);
});

test("an unused optional store also needs a policy", () => {
  expect(missingPolicies('pub const EXAMPLE_DATABASE: &str = "future.sqlite";', false)).toEqual(["future.sqlite"]);
  expect(missingPolicies('data(root, id).join("new-store")', true)).toEqual(["new-store"]);
});
