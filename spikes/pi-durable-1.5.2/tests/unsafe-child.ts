import { appendFile, mkdir, writeFile } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import { createRegistry, defineExtension, defineTool, Harness } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { obj } from "@earendil-works/pi-schema";
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";

const [directory, markerPath, effectsPath] = process.argv.slice(2);
if (!directory || !markerPath || !effectsPath) throw new Error("usage: unsafe-child <store-dir> <marker> <effects>");
await mkdir(directory, { recursive: true });
const faux = fauxProvider();
faux.setResponses([fauxAssistantMessage(fauxToolCall("external_write", {}, { id: "unsafe-call-0001" }), { stopReason: "toolUse" })]);
const models = createModels();
models.setProvider(faux.provider);
const externalWrite = defineTool({
  name: "external_write",
  description: "Fixture external side effect that blocks after dispatch.",
  parameters: obj({}),
  replay: "unsafe",
  execute: async () => {
    await appendFile(effectsPath, "dispatched\n");
    await writeFile(markerPath, "external side effect dispatched; tool still pending\n");
    return new Promise(() => {});
  },
});
const registry = createRegistry();
const fixtureExtension = defineExtension({ name: "fixture", tools: [externalWrite] });
registry.install(fixtureExtension);
const storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT);
const harness = await Harness.open(storage, { models, registry }, BACKGROUND_CONTEXT);
const root = await harness.root(BACKGROUND_CONTEXT, { agent: { model: { provider: "faux", modelId: "faux-1" }, extensions: [fixtureExtension] } });
await root.submit({ type: "input", content: "perform the external write", requestId: "unsafe-process-kill-001" }, BACKGROUND_CONTEXT);
await new Promise(() => {});
