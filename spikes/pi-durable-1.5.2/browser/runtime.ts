import { createModels } from "@earendil-works/pi-ai";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { openaiProvider } from "@earendil-works/pi-ai/providers/openai";
import { openrouterProvider } from "@earendil-works/pi-ai/providers/openrouter";
import { IndexedDbStorage } from "@earendil-works/pi-durable/storage/indexeddb";
import { createRegistry, Harness } from "@earendil-works/pi-durable";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { CredentialStore, Model } from "@earendil-works/pi-ai";

const providers = [anthropicProvider(), openaiProvider(), openrouterProvider()];
const keyPrefix = "konae:agent-key:";

function browserCredentials(): CredentialStore {
  return {
    async read(providerId) {
      const key = localStorage.getItem(`${keyPrefix}${providerId}`);
      return key ? { type: "api_key", key } : undefined;
    },
    async list() {
      return providers.flatMap(({ id }) => localStorage.getItem(`${keyPrefix}${id}`) ? [{ providerId: id, type: "api_key" as const }] : []);
    },
    async modify(providerId, fn) {
      const key = localStorage.getItem(`${keyPrefix}${providerId}`);
      const updated = await fn(key ? { type: "api_key", key } : undefined);
      if (updated?.type === "api_key" && updated.key) localStorage.setItem(`${keyPrefix}${providerId}`, updated.key);
      else localStorage.removeItem(`${keyPrefix}${providerId}`);
      return updated;
    },
    async delete(providerId) { localStorage.removeItem(`${keyPrefix}${providerId}`); },
  };
}

export async function runCompatibilityFixture() {
  const credentials = browserCredentials();
  const models = createModels({ credentials });
  providers.forEach((provider) => models.setProvider(provider));
  const catalogs = providers.map(({ id }) => ({ provider: id, modelCount: models.getModels(id).length }));
  const outputs: Record<string, unknown> = {};
  for (const provider of providers) {
    const catalog = models.getModels(provider.id);
    const base = catalog[0] as Model;
    if (!base) throw new Error(`empty model catalog for ${provider.id}`);
    const custom = { ...base, id: `fixture/${provider.id}-custom`, name: `fixture/${provider.id}-custom` } as Model;
    const stream = models.streamSimple(custom, { messages: [{ role: "user", content: "Return only fixture-ok." }] });
    let text = "";
    let started = false;
    for await (const event of stream) {
      if (event.type === "start") started = true;
      if (event.type === "text_delta") text += event.delta;
      if (event.type === "error") throw new Error(`${provider.id} stream error: ${event.error.errorMessage}`);
    }
    outputs[provider.id] = { model: custom.id, started, text };
  }
  const abortController = new AbortController();
  const abortModel = { ...(models.getModels("openai")[0] as Model), id: "fixture/openai-interrupt", name: "fixture/openai-interrupt" } as Model;
  const abortStream = models.streamSimple(abortModel, { messages: [{ role: "user", content: "interrupt fixture" }] }, { signal: abortController.signal });
  let abortStarted = false;
  let abortTerminated = false;
  try {
    for await (const event of abortStream) {
      if (event.type === "start") { abortStarted = true; abortController.abort(new Error("fixture interruption")); }
      if (event.type === "error") abortTerminated = true;
      if (event.type === "done") abortTerminated = true;
    }
  } catch { abortTerminated = true; }
  const storage = await IndexedDbStorage.open("konae-durable-spike");
  await storage.close();
  const harness = await Harness.open(await IndexedDbStorage.open("konae-durable-spike"), { models, registry: createRegistry() }, BACKGROUND_CONTEXT);
  const root = await harness.root(BACKGROUND_CONTEXT, { agent: { model: { provider: "openai", modelId: models.getModels("openai")[0].id } } });
  const submission = await root.submit({ type: "input", content: "Open a real Durable browser run." }, BACKGROUND_CONTEXT);
  const settled = await submission.wait(BACKGROUND_CONTEXT);
  if (settled.status !== "done") throw new Error(`Harness fixture did not finish: ${settled.status}`);
  await harness.close(BACKGROUND_CONTEXT);
  const viewer = await Harness.open(await IndexedDbStorage.open("konae-durable-spike"), { models, registry: createRegistry(), readOnly: true }, BACKGROUND_CONTEXT);
  const recoveredRoot = await viewer.root(BACKGROUND_CONTEXT);
  const recoveredContext = await recoveredRoot.context(BACKGROUND_CONTEXT);
  const latestPage = await recoveredRoot.entries({ order: "descending" }, 50, undefined, BACKGROUND_CONTEXT);
  await viewer.close(BACKGROUND_CONTEXT);
  return {
    providerIds: providers.map(({ id }) => id),
    catalogs,
    localKeys: providers.map(({ id }) => ({ provider: id, stored: Boolean(localStorage.getItem(`${keyPrefix}${id}`)) })),
    outputs,
    interruption: { started: abortStarted, terminatedBySignal: abortTerminated },
    durableHarness: { settled: settled.status, contextMessages: recoveredContext.messages.length, latestPage: latestPage.items.length, readonly: true },
    indexedDb: typeof indexedDB !== "undefined",
  };
}
