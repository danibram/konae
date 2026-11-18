import { createModels, type Api, type Model, type Models } from "@konae/pi-ai-durable";
import { anthropicProvider } from "@konae/pi-ai-durable/providers/anthropic";
import { openaiProvider } from "@konae/pi-ai-durable/providers/openai";
import { openrouterProvider } from "@konae/pi-ai-durable/providers/openrouter";
import { createAssistantMessageEventStream } from "@konae/pi-ai-durable";
import { PROVIDERS } from "./catalog";
import type { AgentKeyStore, AgentSessionConfig, ProviderId } from "./contracts";
import { redactDurableValue, redactText } from "./durable-trace";

/** Pi Durable's Pi AI 1.5.2 types/runtime stay explicitly scoped away from legacy 0.84.2. */
export function createDurableModels(keyStore: AgentKeyStore) {
  const providers = [anthropicProvider(), openaiProvider(), openrouterProvider()];
  const credentials = {
    async read(providerId: string) {
      const provider = providerId as ProviderId;
      const key = PROVIDERS.some((item) => item.id === provider) ? keyStore.getKey(provider) : null;
      return key ? { type: "api_key" as const, key } : undefined;
    },
    async list() {
      return PROVIDERS.flatMap(({ id }) => keyStore.getKey(id) ? [{ providerId: id, type: "api_key" as const }] : []);
    },
    async modify(providerId: string, modify: (current: { type: "api_key"; key: string } | undefined) => Promise<{ type: "api_key"; key: string } | undefined>) {
      const provider = providerId as ProviderId;
      if (!PROVIDERS.some((item) => item.id === provider)) return undefined;
      const value = await modify(await credentials.read(provider));
      if (value?.type === "api_key" && value.key) keyStore.setKey(provider, value.key);
      else keyStore.clearKey(provider);
      return value;
    },
    async delete(providerId: string) {
      const provider = providerId as ProviderId;
      if (PROVIDERS.some((item) => item.id === provider)) keyStore.clearKey(provider);
    },
  };
  const models = createModels({ credentials });
  for (const provider of providers) models.setProvider(provider);
  return models;
}

export function durableModelFor(config: AgentSessionConfig, models: Models): Model<Api> {
  const id = config.modelId.trim();
  if (!id) throw new Error(`No model id for provider "${config.provider}"`);
  const providerConfig = PROVIDERS.find((item) => item.id === config.provider);
  if (!providerConfig) throw new Error(`Unknown provider "${config.provider}".`);
  const known = models.getModel(config.provider, id);
  if (known) return withBrowserHeaders(known, config.provider);
  if (!providerConfig.supportsCustomModelId) throw new Error(`Provider "${config.provider}" has no model "${id}".`);
  const catalog = models.getModels(config.provider);
  if (!catalog.length) throw new Error(`Provider "${config.provider}" has no available models.`);
  const provider = config.provider;
  const vendor = id.includes("/") ? `${id.split("/")[0]}/` : "";
  const base = (vendor ? catalog.find((item) => item.id.startsWith(vendor)) : undefined) ?? catalog[0];
  return withBrowserHeaders({ ...base, id, name: id, provider } as Model<Api>, provider);
}

/** Harness resolves a persisted ModelRef on every request; retain custom selections there. */
export function modelsWithConfiguredModel(models: Models, config: AgentSessionConfig, keyStore?: AgentKeyStore, onEvent?: (event: any) => void): Models {
  return new Proxy(models, {
    get(target, property) {
      if (property === "getModel") return (provider: string, id: string) => {
        if (provider === config.provider && id === config.modelId.trim()) {
          try { return durableModelFor(config, target); } catch { /* keep the provider's normal resolution error */ }
        }
        return target.getModel(provider, id);
      };
      if (property === "streamSimple") return (model: Model<Api>, context: Parameters<Models["streamSimple"]>[1], streamOptions?: Parameters<Models["streamSimple"]>[2]) => {
        const stream = target.streamSimple(model, context, streamOptions);
        const output = createAssistantMessageEventStream();
        const credentials = ["anthropic", "openai", "openrouter"].flatMap((provider) => {
          try { const key = keyStore?.getKey(provider as ProviderId); return key ? [key] : []; }
          catch { return []; }
        });
        void forwardSanitizedStream(stream, output, credentials, onEvent);
        return output;
      };
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
}

export async function forwardSanitizedStream(source: ReturnType<Models["streamSimple"]>, output: ReturnType<typeof createAssistantMessageEventStream>, credentials: string[], onEvent?: (event: any) => void) {
  const pendingText = new Map<number, string>();
  const emittedText = new Map<number, string>();
  try {
    for await (const raw of source) {
      const event = raw as any;
      if (event.type.startsWith("thinking") || event.type === "redacted_thinking") continue;
      if (event.type === "text_delta") {
        const index = event.contentIndex as number;
        const pending = (pendingText.get(index) ?? "") + event.delta;
        const cut = safeStreamCut(pending, credentials);
        const safe = pending.slice(0, cut);
        pendingText.set(index, pending.slice(cut));
        if (!safe) continue;
        const delta = redactText(safe, credentials);
        emittedText.set(index, (emittedText.get(index) ?? "") + delta);
        const safeEvent = { ...event, delta, partial: sanitizePartial(event.partial, emittedText, credentials) };
        output.push(safeEvent);
        onEvent?.(safeEvent);
        continue;
      }
      if (event.type === "text_end") {
        const index = event.contentIndex as number;
        const safe = redactText(pendingText.get(index) ?? "", credentials);
        pendingText.delete(index);
        const delta = safe;
        emittedText.set(index, (emittedText.get(index) ?? "") + delta);
        if (delta) {
          const deltaEvent = { type: "text_delta", contentIndex: index, delta, partial: sanitizePartial(event.partial, emittedText, credentials) } as any;
          output.push(deltaEvent);
          onEvent?.(deltaEvent);
        }
        const endEvent = { ...event, content: redactText(event.content, credentials), partial: sanitizePartial(event.partial, emittedText, credentials) };
        output.push(endEvent);
        continue;
      }
      if (event.type === "toolcall_delta") {
        // Partial JSON can contain a credential split across provider chunks.
        // Keep the call identity, but do not persist arguments until the complete
        // toolcall_end has been scrubbed as structured data.
        output.push({ ...event, delta: "", partial: sanitizePartial(event.partial, emittedText, credentials) });
        continue;
      }
      if (event.type === "toolcall_end") {
        const safeEvent = { ...event, toolCall: redactDurableValue(event.toolCall, credentials), partial: sanitizePartial(event.partial, emittedText, credentials) };
        output.push(safeEvent);
        onEvent?.(safeEvent);
        continue;
      }
      if (event.type === "done") {
        output.push({ ...event, message: sanitizeAssistant(event.message, credentials), partial: sanitizePartial(event.partial, emittedText, credentials) });
        continue;
      }
      if (event.type === "error") {
        const safeEvent = { ...redactDurableValue(event, credentials) as any, error: sanitizeAssistant(event.error, credentials), partial: sanitizePartial(event.partial, emittedText, credentials) };
        output.push(safeEvent);
        onEvent?.(safeEvent);
        continue;
      }
      output.push(redactDurableValue({ ...event, partial: sanitizePartial(event.partial, emittedText, credentials) }, credentials) as any);
    }
    const final = await source.result();
    output.end(sanitizeAssistant(final, credentials));
  } catch (error) {
    output.end(sanitizeAssistant({ role: "assistant", content: [], stopReason: "error", errorMessage: String(error), timestamp: Date.now() } as any, credentials));
  }
}

function safeStreamCut(value: string, credentials: readonly string[]) {
  let cut = value.length;
  const potentialMarkers = ["sk-ant-", "sk-or-v1-", "sk-proj-", "Bearer "];
  for (const marker of potentialMarkers) {
    const at = value.indexOf(marker);
    if (at >= 0 && !/[\s"'<>;,]/.test(value.slice(at + marker.length))) cut = Math.min(cut, at);
    for (let length = 1; length < marker.length && length <= value.length; length += 1) {
      if (value.endsWith(marker.slice(0, length))) cut = Math.min(cut, value.length - length);
    }
  }
  for (const key of credentials) {
    for (let length = 1; length < key.length && length <= value.length; length += 1) {
      if (value.endsWith(key.slice(0, length))) cut = Math.min(cut, value.length - length);
    }
  }
  for (const marker of ["https://", "http://"]) {
    const at = value.lastIndexOf(marker, cut);
    if (at >= 0 && !/[\s<>"']/.test(value.slice(at))) cut = Math.min(cut, at);
  }
  return cut;
}

function sanitizePartial(message: any, emittedText: Map<number, string>, credentials: readonly string[]) {
  const safe = redactDurableValue(message, credentials) as any;
  if (!Array.isArray(safe?.content)) return safe;
  const originalContent = message?.content;
  if (Array.isArray(originalContent)) safe.content = safe.content.map((part: any, index: number) => {
    const original = originalContent[index];
    if (original?.type === "thinking" || original?.type === "redacted_thinking" || original?.type === "reasoning") return null;
    if (original?.type === "toolCall") return { type: "toolCall", id: original.id, name: original.name, arguments: {} };
    if (original?.type === "text") return { ...part, text: emittedText.get(index) ?? "" };
    return part;
  }).filter(Boolean);
  return safe;
}

function sanitizeAssistant(message: any, credentials: readonly string[]) {
  const safe = redactDurableValue(message, credentials) as any;
  if (Array.isArray(safe?.content)) safe.content = safe.content.filter((part: any) => part?.type !== "thinking" && part?.type !== "redacted_thinking" && part?.type !== "reasoning");
  return safe;
}

function withBrowserHeaders(model: Model<Api>, provider: ProviderId): Model<Api> {
  const headers: Record<string, string> = provider === "anthropic"
    ? { "anthropic-dangerous-direct-browser-access": "true" }
    : provider === "openrouter" ? { "x-title": "Konae" } : {};
  return { ...model, headers: { ...model.headers, ...headers } };
}
