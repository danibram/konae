import { describe, expect, test } from "bun:test";
import type { Models, SimpleStreamOptions } from "@earendil-works/pi-ai";
import { agentKeyName, createAgentKeyStore, emptyAgentKeyStore, type KeyStorage } from "./key-store";
import { PROVIDERS, buildAuth, buildModel, createAgentModels, createStreamFn, defaultSessionConfig, providerIsConfigured } from "./providers";

function memoryStorage(): KeyStorage & { entries: Map<string, string> } {
  const entries = new Map<string, string>();
  return {
    entries,
    getItem: (name) => entries.get(name) ?? null,
    setItem: (name, value) => void entries.set(name, value),
    removeItem: (name) => void entries.delete(name),
  };
}

const models = createAgentModels();

describe("agent providers", () => {
  test("builds a model for every curated option on every provider", () => {
    for (const provider of PROVIDERS) {
      expect(provider.defaultModels.length).toBeGreaterThan(0);
      for (const option of provider.defaultModels) {
        const model = buildModel({ provider: provider.id, modelId: option.id }, models);
        expect(model.id).toBe(option.id);
        expect(model.provider).toBe(provider.id);
        expect(model.baseUrl).toStartWith("https://");
      }
    }
  });

  test("keeps each provider on its own endpoint", () => {
    expect(buildModel(defaultSessionConfig("anthropic"), models).baseUrl).toBe("https://api.anthropic.com");
    expect(buildModel(defaultSessionConfig("openai"), models).baseUrl).toBe("https://api.openai.com/v1");
    expect(buildModel(defaultSessionConfig("openrouter"), models).baseUrl).toBe("https://openrouter.ai/api/v1");
  });

  test("sends a custom OpenRouter id verbatim as the wire slug", () => {
    const model = buildModel({ provider: "openrouter", modelId: " anthropic/some-future-model " }, models);
    expect(model.id).toBe("anthropic/some-future-model");
    expect(model.provider).toBe("openrouter");
    expect(model.api).toBe("openai-completions");
  });

  test("derives a custom id from a same-vendor catalog entry", () => {
    const model = buildModel({ provider: "openrouter", modelId: "openai/some-future-model" }, models);
    expect(model.id).toBe("openai/some-future-model");
    expect(model.baseUrl).toBe("https://openrouter.ai/api/v1");
  });

  test("carries Anthropic-style cache markers on OpenRouter Anthropic models", () => {
    const anthropicOnOpenrouter = buildModel({ provider: "openrouter", modelId: "anthropic/claude-sonnet-4.5" }, models);
    expect(anthropicOnOpenrouter.compat).toMatchObject({ cacheControlFormat: "anthropic" });
  });

  test("rejects an empty model id and an unknown provider", () => {
    expect(() => buildModel({ provider: "anthropic", modelId: "  " }, models)).toThrow();
    // @ts-expect-error unknown provider ids must not type-check either
    expect(() => buildModel({ provider: "ollama", modelId: "llama3" }, models)).toThrow();
  });

  test("auth resolves the key of its own provider only", () => {
    const storage = memoryStorage();
    const keyStore = createAgentKeyStore(storage);
    keyStore.setKey("anthropic", "sk-ant-test");
    keyStore.setKey("openrouter", "sk-or-test");

    const auth = buildAuth("anthropic", keyStore);
    expect(auth.provider).toBe("anthropic");
    expect(auth.getApiKey("anthropic")).toBe("sk-ant-test");
    expect(auth.getApiKey("openrouter")).toBeUndefined();
  });

  test("auth carries the browser-access header and a cache retention", () => {
    const keyStore = createAgentKeyStore(memoryStorage());
    keyStore.setKey("anthropic", "sk-ant-test");

    const anthropic = buildAuth("anthropic", keyStore).requestOptions();
    expect(anthropic.apiKey).toBe("sk-ant-test");
    expect(anthropic.headers["anthropic-dangerous-direct-browser-access"]).toBe("true");
    expect(anthropic.cacheRetention).toBe("short");

    const openai = buildAuth("openai", keyStore, { cacheRetention: "long" });
    expect(openai.requestOptions().cacheRetention).toBe("long");
    expect(openai.requestOptions().apiKey).toBeUndefined();
  });

  test("auth re-reads the key on every turn", () => {
    const keyStore = createAgentKeyStore(memoryStorage());
    const auth = buildAuth("openrouter", keyStore);
    expect(auth.getApiKey("openrouter")).toBeUndefined();
    keyStore.setKey("openrouter", "sk-or-late");
    expect(auth.getApiKey("openrouter")).toBe("sk-or-late");
  });

  test("the stream fn injects what the Agent does not forward", () => {
    const keyStore = createAgentKeyStore(memoryStorage());
    keyStore.setKey("anthropic", "sk-ant-test");

    let seen: SimpleStreamOptions | undefined;
    const spy = { streamSimple: (_model: unknown, _context: unknown, options?: SimpleStreamOptions) => { seen = options; return null; } };
    const streamFn = createStreamFn(buildAuth("anthropic", keyStore), spy as unknown as Models);

    streamFn(buildModel(defaultSessionConfig("anthropic"), models), { messages: [] }, { signal: new AbortController().signal });
    expect(seen?.apiKey).toBe("sk-ant-test");
    expect(seen?.headers?.["anthropic-dangerous-direct-browser-access"]).toBe("true");
    expect(seen?.cacheRetention).toBe("short");
    expect(seen?.signal).toBeDefined();
  });

  test("reports whether a provider is configured", () => {
    const keyStore = createAgentKeyStore(memoryStorage());
    expect(providerIsConfigured("anthropic", keyStore)).toBe(false);
    keyStore.setKey("anthropic", "sk-ant-test");
    expect(providerIsConfigured("anthropic", keyStore)).toBe(true);
  });
});

describe("agent key store", () => {
  test("round-trips a key under its own namespace", () => {
    const storage = memoryStorage();
    const keyStore = createAgentKeyStore(storage);

    expect(keyStore.getKey("openai")).toBeNull();
    keyStore.setKey("openai", "  sk-openai-test  ");
    expect(keyStore.getKey("openai")).toBe("sk-openai-test");
    expect([...storage.entries.keys()]).toEqual([agentKeyName("openai")]);
    expect(agentKeyName("openai")).toStartWith("littlebox:agent-key:");

    keyStore.clearKey("openai");
    expect(keyStore.getKey("openai")).toBeNull();
    expect(storage.entries.size).toBe(0);
  });

  test("keeps providers separate and treats a blank key as cleared", () => {
    const keyStore = createAgentKeyStore(memoryStorage());
    keyStore.setKey("anthropic", "sk-ant-test");
    keyStore.setKey("openrouter", "sk-or-test");
    expect(keyStore.getKey("anthropic")).toBe("sk-ant-test");
    expect(keyStore.getKey("openrouter")).toBe("sk-or-test");

    keyStore.setKey("anthropic", "   ");
    expect(keyStore.getKey("anthropic")).toBeNull();
    expect(keyStore.getKey("openrouter")).toBe("sk-or-test");
  });

  test("survives a storage that refuses to persist", () => {
    const denied: KeyStorage = {
      getItem: () => { throw new Error("denied"); },
      setItem: () => { throw new Error("denied"); },
      removeItem: () => { throw new Error("denied"); },
    };
    const keyStore = createAgentKeyStore(denied);
    expect(() => keyStore.setKey("openai", "sk-openai-test")).not.toThrow();
    expect(keyStore.getKey("openai")).toBeNull();
  });

  test("the empty store holds nothing", () => {
    const keyStore = emptyAgentKeyStore();
    keyStore.setKey("anthropic", "sk-ant-test");
    expect(keyStore.getKey("anthropic")).toBeNull();
  });
});
