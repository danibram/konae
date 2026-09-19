import { describe, expect, test } from "bun:test";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { checkActionContract } from "../src/lib/action-contract";
import { buildLocal } from "./littlebox";

const repo = path.resolve(import.meta.dir, "..");

describe("littlebox contract", () => {
  test("writes the contract, its schema and Markdown beside littlebox.json, and never uploads them", async () => {
    const root = await mkdtemp(path.join(tmpdir(), "littlebox-contract-"));
    try {
      await cp(path.join(repo, "examples/product-fit/feed"), root, { recursive: true });
      await writeFile(path.join(root, "littlebox.json"), JSON.stringify({ url: "http://localhost:4173", project: { id: "app_feed", name: "Status feed", slug: "feed", manifest: { version: 1 } } }));
      const run = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), "contract", "--dir", root], { stdout: "pipe", stderr: "pipe" });
      const [code, output] = await Promise.all([run.exited, new Response(run.stdout).text()]);
      expect(code).toBe(0);
      expect(output).toContain("Wrote the Action Contract for 4 actions");
      const contract = JSON.parse(await readFile(path.join(root, "action-contract.json"), "utf8"));
      expect(checkActionContract(contract)).toEqual([]);
      expect(contract.project).toEqual({ id: "app_feed", name: "Status feed" });
      expect(await readFile(path.join(root, "action-contract.md"), "utf8")).toContain("# Action Contract for Status feed");
      expect(JSON.parse(await readFile(path.join(root, "action-contract.schema.json"), "utf8")).$defs.action).toBeDefined();
      const { files } = await buildLocal(root);
      expect(Object.keys(files).filter((file) => file.startsWith("action-contract") || file === "littlebox.json")).toEqual([]);
      expect(files["actions.json"]).toBeDefined();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test("refuses a directory without actions.json", async () => {
    const root = await mkdtemp(path.join(tmpdir(), "littlebox-contract-"));
    try {
      const run = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), "contract", "--dir", root], { stdout: "pipe", stderr: "pipe" });
      const [code, error] = await Promise.all([run.exited, new Response(run.stderr).text()]);
      expect(code).toBe(1);
      expect(error).toContain(`No actions.json in ${root}.`);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
