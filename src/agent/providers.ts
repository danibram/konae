import type { StreamFn } from "@earendil-works/pi-agent-core";
import { createModels, type Api, type CacheRetention, type Model, type Models, type MutableModels, type Provider, type ProviderHeaders } from "@earendil-works/pi-ai";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { openaiProvider } from "@earendil-works/pi-ai/providers/openai";
import { openrouterProvider } from "@earendil-works/pi-ai/providers/openrouter";
import { PROVIDERS, providerConfig } from "./catalog";
import type { AgentKeyStore, AgentSessionConfig, ProviderConfig, ProviderId } from "./contracts";

/** The curated catalogue lives in ./catalog so the editor can read it without pi-ai. */
export { PROVIDERS, providerConfig, defaultSessionConfig } from "./catalog";

/**
 * Extra request options the Agent does not forward to `streamFn` (it passes
 * only `apiKey` and `signal`), so they have to be re-applied per request.
 */
export type AgentRequestAuth = {
  apiKey?: string;
  headers: ProviderHeaders;
  cacheRetention: CacheRetention;
};

export type AgentAuth = {
  provider: ProviderId;
  /** `AgentOptions.getApiKey`: re-read every turn, so a key entered mid-session takes effect. */
  getApiKey: (provider: string) => string | undefined;
  requestOptions: () => AgentRequestAuth;
};

const providerFactories: Record<ProviderId, () => Provider> = {
  anthropic: anthropicProvider,
  openai: openaiProvider,
  openrouter: openrouterProvider,
};

/**
 * Anthropic direct-from-browser needs `anthropic-dangerous-direct-browser-access`.
 * The Anthropic adapter already sets it; sending it explicitly keeps the browser
 * path from depending on that internal detail.
 */
const providerHeaders: Record<ProviderId, ProviderHeaders> = {
  anthropic: { "anthropic-dangerous-direct-browser-access": "true" },
  openai: {},
  openrouter: { "x-title": "Konae" },
};

let sharedModels: MutableModels | null = null;

/** The three providers we support, registered once. Never `providers/all`. */
export function agentModels(): MutableModels {
  if (!sharedModels) sharedModels = createAgentModels();
  return sharedModels;
}

export function createAgentModels(): MutableModels {
  const models = createModels();
  for (const config of PROVIDERS) models.setProvider(providerFactories[config.id]());
  return models;
}

export function providerIsConfigured(provider: ProviderId, keyStore: AgentKeyStore) {
  return !providerConfig(provider).needsKey || keyStore.getKey(provider) !== null;
}

/**
 * Resolves a session config to a pi-ai model. Catalog ids resolve directly;
 * a custom id is sent verbatim as the wire slug (OpenRouter's `vendor/model`
 * form included) on a model derived from the closest catalog entry, so its
 * cost and context-window metadata are an approximation.
 */
export function buildModel(config: AgentSessionConfig, models: Models = agentModels()): Model<Api> {
  const provider = providerConfig(config.provider);
  const modelId = config.modelId.trim();
  if (modelId.length === 0) throw new Error(`No model id for provider "${provider.id}"`);

  const known = models.getModel(provider.id, modelId);
  if (known) return known;
  if (!provider.supportsCustomModelId) throw new Error(`Provider "${provider.id}" has no model "${modelId}"`);

  const base = customModelBase(provider, modelId, models);
  if (!base) throw new Error(`Provider "${provider.id}" has no catalog entry to derive "${modelId}" from`);
  return { ...base, id: modelId, name: modelId };
}

/**
 * Browser-local BYOK. Passing `apiKey` short-circuits every other credential
 * source in pi-ai, so no key ever reaches a credential store or an env lookup.
 */
export function buildAuth(
  provider: ProviderId,
  keyStore: AgentKeyStore,
  options: { cacheRetention?: CacheRetention } = {},
): AgentAuth {
  const config = providerConfig(provider);
  const cacheRetention = options.cacheRetention ?? "short";
  const readKey = () => (config.needsKey ? keyStore.getKey(provider) ?? undefined : undefined);
  return {
    provider,
    getApiKey: (requested) => (requested === provider ? readKey() : undefined),
    requestOptions: () => ({ apiKey: readKey(), headers: providerHeaders[provider], cacheRetention }),
  };
}

/**
 * Prompt caching: Anthropic marks the system prompt, the last tool definition
 * and the last conversation block automatically, and OpenRouter gets the same
 * treatment for `anthropic/*` models through `compat.cacheControlFormat`.
 * Direct OpenAI caches implicitly and ignores `cacheRetention` here.
 */
export function createStreamFn(auth: AgentAuth, models: Models = agentModels()): StreamFn {
  return (model, context, options) => {
    const resolved = auth.requestOptions();
    return models.streamSimple(model, context, {
      ...options,
      apiKey: options?.apiKey ?? resolved.apiKey,
      headers: { ...resolved.headers, ...options?.headers },
      cacheRetention: options?.cacheRetention ?? resolved.cacheRetention,
    });
  };
}

function customModelBase(provider: ProviderConfig, modelId: string, models: Models) {
  const catalog = models.getModels(provider.id);
  if (catalog.length === 0) return undefined;
  const vendor = modelId.includes("/") ? `${modelId.split("/")[0]}/` : null;
  const sameVendor = vendor ? catalog.find((model) => model.id.startsWith(vendor)) : undefined;
  const declaredDefault = catalog.find((model) => model.id === provider.defaultModels[0]?.id);
  return sameVendor ?? declaredDefault ?? catalog[0];
}
