import { describe, expect, test } from "bun:test";
import { createEditTool, createReadTool, createWriteTool, getOrThrow } from "@earendil-works/pi-agent-core";
import type { ProjectFile } from "../lib/types";
import { capsuleFileFor, capsuleFileNames, createCapsuleWorkspace } from "./capsule-env";
import type { CapsuleFiles, CapsuleMutation } from "./contracts";

const seed: CapsuleFiles = {
  "index.html": "<main>hello</main>",
  "styles.css": "body { color: #24251f; }",
  "app.js": "const a = 1;\nconst b = 2;\n",
  "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);",
  "actions.json": '{ "version": 1, "actions": {} }',
};

function workspace() {
  return createCapsuleWorkspace(seed);
}

describe("capsule paths", () => {
  test("resolves any safe file inside the workspace and rejects paths outside it", () => {
    expect(capsuleFileFor("/workspace/app.js")).toBe("app.js");
    expect(capsuleFileFor("app.js")).toBe("app.js");
    expect(capsuleFileFor("./styles.css")).toBe("styles.css");
    expect(capsuleFileFor("/workspace/nested/../schema.sql")).toBe("schema.sql");
    expect(capsuleFileFor("/workspace/src/features/tasks.ts")).toBe("src/features/tasks.ts");
    expect(capsuleFileFor("/etc/passwd")).toBeNull();
    expect(capsuleFileFor("/workspace")).toBeNull();
    expect(capsuleFileFor("")).toBeNull();
  });
});

describe("capsule filesystem", () => {
  test("reads files, line ranges and bytes", async () => {
    const env = workspace().env;
    expect(env.cwd).toBe("/workspace");
    expect(getOrThrow(await env.readTextFile("app.js"))).toBe(seed["app.js"]);
    expect(getOrThrow(await env.readTextLines("/workspace/app.js", { maxLines: 1 }))).toEqual(["const a = 1;"]);
    expect(new TextDecoder().decode(getOrThrow(await env.readBinaryFile("/workspace/index.html")))).toBe(seed["index.html"]);
  });

  test("reports missing, invalid and directory paths as Results instead of throwing", async () => {
    const env = workspace().env;
    const missing = await env.readTextFile("/workspace/readme.md");
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.error.code).toBe("not_found");

    const directory = await env.readTextFile("/workspace");
    expect(directory.ok).toBe(false);
    if (!directory.ok) expect(directory.error.code).toBe("is_directory");

    const invalid = await env.readTextFile("   ");
    expect(invalid.ok).toBe(false);
    if (!invalid.ok) expect(invalid.error.code).toBe("invalid");

    const outside = await env.writeFile("/etc/passwd", "x");
    expect(outside.ok).toBe(false);
    if (!outside.ok) expect(outside.error.code).toBe("not_supported");
  });

  test("describes the workspace through info, list, exists and canonicalPath", async () => {
    const env = workspace().env;
    const root = getOrThrow(await env.listDir("/workspace"));
    expect(root.map((entry) => entry.name)).toEqual([...capsuleFileNames].sort());
    expect(root.every((entry) => entry.kind === "file")).toBe(true);

    expect(getOrThrow(await env.listDir("/")).map((entry) => entry.path)).toEqual(["/workspace"]);

    const info = getOrThrow(await env.fileInfo("/workspace/styles.css"));
    expect(info).toMatchObject({ name: "styles.css", path: "/workspace/styles.css", kind: "file" });
    expect(info.size).toBe(seed["styles.css"].length);

    expect(getOrThrow(await env.fileInfo("/workspace")).kind).toBe("directory");
    expect(getOrThrow(await env.exists("/workspace/app.js"))).toBe(true);
    expect(getOrThrow(await env.exists("/workspace/other.js"))).toBe(false);
    expect(getOrThrow(await env.canonicalPath("app.js"))).toBe("/workspace/app.js");
    expect(getOrThrow(await env.absolutePath("../app.js"))).toBe("/app.js");
    expect(getOrThrow(await env.joinPath(["/workspace", "schema.sql"]))).toBe("/workspace/schema.sql");

    const notDirectory = await env.listDir("/workspace/app.js");
    expect(notDirectory.ok).toBe(false);
    if (!notDirectory.ok) expect(notDirectory.error.code).toBe("not_directory");
  });

  test("accepts nested directories but refuses unsupported structural operations", async () => {
    const env = workspace().env;
    for (const result of [
      await env.renameFile("/workspace/app.js", "/workspace/main.js"),
      await env.remove("/workspace/app.js"),
      await env.createTempDir(),
      await env.createTempFile(),
    ]) {
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.error.code).toBe("not_supported");
    }
    expect(getOrThrow(await env.createDir("/workspace"))).toBeUndefined();
    expect(getOrThrow(await env.createDir("/workspace/lib"))).toBeUndefined();
  });

  test("has no shell", async () => {
    const result = await workspace().env.exec("ls");
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.code).toBe("shell_unavailable");
      expect(result.error.message).toContain("no shell in Littlebox");
    }
  });

  test("honours abort signals", async () => {
    const env = workspace().env;
    const controller = new AbortController();
    controller.abort();
    const result = await env.readTextFile("app.js", controller.signal);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error.code).toBe("aborted");
  });
});

describe("capsule mutations", () => {
  test("writes are proposals: the seed is never touched and files is a fresh snapshot", async () => {
    const capsule = workspace();
    const before = capsule.files;
    getOrThrow(await capsule.env.writeFile("/workspace/styles.css", "body { color: red; }"));
    expect(capsule.files).not.toBe(before);
    expect(before["styles.css"]).toBe(seed["styles.css"]);
    expect(capsule.files["styles.css"]).toBe("body { color: red; }");
    expect(capsule.files["app.js"]).toBe(seed["app.js"]);
    expect(seed["styles.css"]).toBe("body { color: #24251f; }");
  });

  test("appendFile extends the file", async () => {
    const capsule = workspace();
    getOrThrow(await capsule.env.appendFile("styles.css", "\n.extra { display: none; }"));
    expect(capsule.files["styles.css"]).toBe(`${seed["styles.css"]}\n.extra { display: none; }`);
    expect(capsule.changedFiles()).toEqual(["styles.css"]);
  });

  test("changedFiles diffs against the seed in first-write order", async () => {
    const capsule = workspace();
    expect(capsule.changedFiles()).toEqual([]);

    getOrThrow(await capsule.env.writeFile("/workspace/app.js", "const a = 3;"));
    getOrThrow(await capsule.env.writeFile("/workspace/index.html", "<main>bye</main>"));
    expect(capsule.changedFiles()).toEqual(["app.js", "index.html"]);

    getOrThrow(await capsule.env.writeFile("/workspace/app.js", seed["app.js"]));
    expect(capsule.changedFiles()).toEqual(["index.html"]);

    const snapshot = capsule.serialize();
    expect(snapshot.changedFiles).toEqual(["index.html"]);
    expect(snapshot.files["app.js"]).toBe(seed["app.js"]);
    expect(Object.keys(snapshot.files).sort()).toEqual([...capsuleFileNames].sort());
  });

  test("mutation events carry the file, a full snapshot and the current changed set", async () => {
    const capsule = workspace();
    const seen: CapsuleMutation[] = [];
    const unsubscribe = capsule.onMutation((mutation) => seen.push(mutation));

    getOrThrow(await capsule.env.writeFile("/workspace/schema.sql", "CREATE TABLE IF NOT EXISTS notes (id INTEGER);"));
    unsubscribe();
    getOrThrow(await capsule.env.writeFile("/workspace/app.js", "const a = 4;"));

    expect(seen).toHaveLength(1);
    expect(seen[0].file).toBe<ProjectFile>("schema.sql");
    expect(seen[0].changedFiles).toEqual(["schema.sql"]);
    expect(seen[0].files["schema.sql"]).toContain("notes");
    expect(capsule.changedFiles()).toEqual(["schema.sql", "app.js"]);
  });

  test("the pi built-in file tools run against this env unchanged", async () => {
    const capsule = workspace();
    const context = { env: capsule.env };

    const edited = await createEditTool()
      .execute("call_1", { path: "/workspace/styles.css", edits: [{ oldText: "#24251f", newText: "#3157d5" }] }, undefined, undefined, context);
    expect(edited.content[0]).toMatchObject({ type: "text" });
    expect(capsule.files["styles.css"]).toBe("body { color: #3157d5; }");

    const read = await createReadTool().execute("call_2", { path: "/workspace/app.js" }, undefined, undefined, context);
    expect(read.content[0]).toMatchObject({ type: "text", text: seed["app.js"] });

    await createWriteTool().execute("call_3", { path: "/workspace/index.html", content: "<main>bye</main>" }, undefined, undefined, context);
    expect(capsule.files["index.html"]).toBe("<main>bye</main>");

    await createWriteTool().execute("call_4", { path: "/workspace/src/features/extra.ts", content: "export const x = 1;" }, undefined, undefined, context);
    expect(capsule.files["src/features/extra.ts"]).toBe("export const x = 1;");
    expect(getOrThrow(await capsule.env.listDir("/workspace/src")).map((entry) => entry.name)).toEqual(["features"]);
    expect(capsule.changedFiles()).toEqual(["styles.css", "index.html", "src/features/extra.ts"]);
  });

  test("fileInfo size and mtime follow writes", async () => {
    const capsule = workspace();
    const before = getOrThrow(await capsule.env.fileInfo("/workspace/app.js"));
    getOrThrow(await capsule.env.writeFile("/workspace/app.js", "x".repeat(500)));
    const after = getOrThrow(await capsule.env.fileInfo("/workspace/app.js"));
    expect(after.size).toBe(500);
    expect(after.mtimeMs).toBeGreaterThanOrEqual(before.mtimeMs);
  });
});
