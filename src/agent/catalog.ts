import type { AgentSessionConfig, ProviderConfig, ProviderId } from "./contracts";

/**
 * The curated provider catalogue. It lives in its own module, free of `pi-ai`
 * imports, so the editor can render the model picker without pulling the agent
 * chunk (and its provider catalogs) into the cold start.
 *
 * Every id here was checked against the shipped 0.84.2 catalogs.
 * `supportsCustomModelId` is the escape hatch for everything else.
 *
 * Each `keyPage` is the page the provider's own docs link to for creating a key
 * (docs.claude.com API overview, OpenAI's quickstart, OpenRouter's
 * authentication guide); the pi-ai catalogs record only API endpoints.
 */
export const PROVIDERS: readonly ProviderConfig[] = [
  {
    id: "anthropic",
    label: "Anthropic",
    needsKey: true,
    keyPage: "https://platform.claude.com/settings/keys",
    supportsCustomModelId: true,
    defaultModels: [
      { id: "claude-sonnet-5", label: "Claude Sonnet 5" },
      { id: "claude-opus-5", label: "Claude Opus 5" },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
    ],
  },
  {
    id: "openai",
    label: "OpenAI",
    needsKey: true,
    keyPage: "https://platform.openai.com/api-keys",
    supportsCustomModelId: true,
    defaultModels: [
      { id: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
      { id: "gpt-5.4", label: "GPT-5.4" },
      { id: "gpt-5-mini", label: "GPT-5 mini" },
    ],
  },
  {
    id: "openrouter",
    label: "OpenRouter",
    needsKey: true,
    keyPage: "https://openrouter.ai/keys",
    supportsCustomModelId: true,
    defaultModels: [
      { id: "anthropic/claude-sonnet-5", label: "Claude Sonnet 5" },
      { id: "anthropic/claude-opus-5", label: "Claude Opus 5" },
      { id: "anthropic/claude-haiku-4.5", label: "Claude Haiku 4.5" },
      { id: "openai/gpt-5.1", label: "GPT-5.1" },
    ],
  },
];

export function providerConfig(provider: ProviderId): ProviderConfig {
  const config = PROVIDERS.find((candidate) => candidate.id === provider);
  if (!config) throw new Error(`Unknown agent provider "${provider}"`);
  return config;
}

export function defaultSessionConfig(provider: ProviderId = "anthropic"): AgentSessionConfig {
  return { provider, modelId: providerConfig(provider).defaultModels[0].id };
}

/** Tolerant lookup for the pickers: an unknown id falls back to the first provider. */
export function resolveProvider(providers: readonly ProviderConfig[], id: ProviderId): ProviderConfig {
  return providers.find((provider) => provider.id === id) ?? providers[0];
}

export function isCustomModelId(provider: ProviderConfig, modelId: string): boolean {
  return !provider.defaultModels.some((model) => model.id === modelId);
}

/** Switching provider must not leave a model id the new provider cannot serve. */
export function defaultModelIdFor(provider: ProviderConfig): string {
  return provider.defaultModels[0]?.id ?? "";
}

export type KeyState = "not-needed" | "missing" | "stored";

export function keyState(provider: ProviderConfig, hasKey: boolean): KeyState {
  if (!provider.needsKey) return "not-needed";
  return hasKey ? "stored" : "missing";
}
