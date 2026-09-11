import type { AgentKeyStore, ProviderId } from "./contracts";

/**
 * Provider keys live under their own localStorage namespace, separate from
 * `littlebox:projects` / `littlebox:deployments`, so no Project, Capsule file,
 * Deployment or export can carry one. Keys are never logged.
 */
const keyNamespace = "littlebox:agent-key:";

export type KeyStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function agentKeyName(provider: ProviderId) {
  return `${keyNamespace}${provider}`;
}

export function isAgentKeyName(name: string) {
  return name.startsWith(keyNamespace);
}

export function createAgentKeyStore(storage: KeyStorage = localStorage): AgentKeyStore {
  return {
    getKey(provider) {
      const stored = read(storage, agentKeyName(provider));
      return stored && stored.length > 0 ? stored : null;
    },
    setKey(provider, key) {
      const trimmed = key.trim();
      write(storage, agentKeyName(provider), trimmed.length > 0 ? trimmed : null);
    },
    clearKey(provider) {
      write(storage, agentKeyName(provider), null);
    },
  };
}

/** A store that holds nothing: the Kernel-proxy path, and the safe default in tests. */
export function emptyAgentKeyStore(): AgentKeyStore {
  return { getKey: () => null, setKey: () => {}, clearKey: () => {} };
}

function read(storage: KeyStorage, name: string) {
  try {
    return storage.getItem(name)?.trim() ?? null;
  } catch {
    return null;
  }
}

function write(storage: KeyStorage, name: string, value: string | null) {
  try {
    if (value === null) storage.removeItem(name);
    else storage.setItem(name, value);
  } catch {
    // Private-mode storage denials must not break an in-memory session.
  }
}
