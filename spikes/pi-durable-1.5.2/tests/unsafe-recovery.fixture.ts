import { appendFile, mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import { createRegistry, defineExtension, defineTool, Harness, ToolResultEntry } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { obj } from "@earendil-works/pi-schema";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { expect, it } from "bun:test";

const packageDir = new URL("..", import.meta.url).pathname;
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

it("kills the writer during an external unsafe tool and recovers without replay", async () => {
  const directory = await mkdtemp(join(tmpdir(), "pi-durable-unsafe-"));
  const storeDir = join(directory, "store");
  const marker = join(directory, "entered");
  const effects = join(directory, "effects.log");
  const child = Bun.spawn([process.execPath, "tests/unsafe-child.ts", storeDir, marker, effects], {
    cwd: packageDir,
    stdout: "ignore",
    stderr: "pipe",
  });
  let dispatched = false;
  try {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      try { await stat(marker); dispatched = true; break; }
      catch { if (child.exitCode !== null) break; await sleep(25); }
    }
    expect(dispatched).toBe(true);
    child.kill(9);
    await child.exited;
    expect((await readFile(effects, "utf8")).trim().split("\n")).toHaveLength(1);
  } catch (error) {
    if (child.exitCode === null) child.kill(9);
    await child.exited;
    await rm(directory, { recursive: true, force: true });
    throw error;
  }

  const faux = fauxProvider();
  faux.setResponses([fauxAssistantMessage(fauxText("The earlier write may have happened; reconcile before a new write."))]);
  const models = createModels();
  models.setProvider(faux.provider);
  let replayCount = 0;
  const externalWrite = defineTool({
    name: "external_write",
    description: "Must not run a second time after a process interruption.",
    parameters: obj({}),
    replay: "unsafe",
    execute: async () => { replayCount++; await appendFile(effects, "replayed\n"); return { output: [{ type: "text", text: "incorrect replay" }] }; },
  });
  const registry = createRegistry();
  const fixtureExtension = defineExtension({ name: "fixture", tools: [externalWrite] });
  registry.install(fixtureExtension);
  const storage = await openNodeJsonlStorage(storeDir, BACKGROUND_CONTEXT);
  const harness = await Harness.open(storage, { models, registry }, BACKGROUND_CONTEXT);
  try {
    harness.resume();
    const root = await harness.root(BACKGROUND_CONTEXT);
    await sleep(350);
    expect(replayCount).toBe(0);
    expect((await readFile(effects, "utf8")).trim().split("\n")).toHaveLength(1);
    const entries = await root.entries({ order: "ascending" }, 50, undefined, BACKGROUND_CONTEXT);
    const tools = entries.items.filter((entry) => ToolResultEntry.is(entry));
    expect(tools.length).toBeGreaterThan(0);
    expect(JSON.stringify(tools)).toMatch(/interrupted/i);
  } finally {
    await harness.close(BACKGROUND_CONTEXT);
    await rm(directory, { recursive: true, force: true });
  }
});
