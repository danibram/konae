import type { AgentSessionConfig, ProviderConfig, ProviderId } from "./contracts";

export const PROVIDERS: readonly ProviderConfig[] = [
  {
    id: "anthropic",
    label: "Anthropic",
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

export function resolveProvider(providers: readonly ProviderConfig[], id: ProviderId): ProviderConfig {
  return providers.find((provider) => provider.id === id) ?? providers[0];
}

export function isCustomModelId(provider: ProviderConfig, modelId: string): boolean {
  return !provider.defaultModels.some((model) => model.id === modelId);
}

export function defaultModelIdFor(provider: ProviderConfig): string {
  return provider.defaultModels[0]?.id ?? "";
}

export type KeyState = "missing" | "stored";

export function keyState(hasKey: boolean): KeyState {
  return hasKey ? "stored" : "missing";
}
