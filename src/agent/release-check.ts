// release_plan's sentences from names alone, for the Validation Gate and the
// Deploy dialog. Kept apart from gate.ts so the editor shell can import it
// without the Gate's sql.js path (ADR-0003, lever 7).
import type { ServerActionProgram } from "../kernel/actions";
import type { DomainRules } from "../lib/domain-name";

/**
 * What the editor knows about the project and its installation that
 * release_plan also reads. Each part is absent when the editor could not read
 * it, and the Gate then says nothing about what it would decide.
 */
export type ReleaseFacts = {
  /**
   * Names, use and whether the value decodes as a svix key, from
   * `secrets_list`. Never a value.
   */
  secrets?: { name: string; use?: string; svixKey?: boolean }[];
  /** The installation's app suffix and its own hosts, from `domain_read`. */
  rules?: DomainRules;
};

/**
 * The first sentence release_plan would refuse a parsed program with, from
 * names alone: kernel-rs/src/http.rs `require_secrets`, then calls.rs
 * `check_release`, each over actions in name order. The cases are in
 * kernel-rs/src/release-cases.json. A svix route is refused for its key only
 * when `secrets_list` says the value does not decode, since the editor never
 * reads the value.
 */
export function releaseRefusal(program: ServerActionProgram, facts: ReleaseFacts): string | null {
  const names = Object.keys(program.actions).sort();
  const known = facts.secrets ? new Map(facts.secrets.map((secret) => [secret.name, secret])) : null;
  if (known) {
    const actions: string[] = [];
    const unset: string[] = [];
    for (const name of names) {
      const signature = program.actions[name].http?.signature;
      if (!signature) continue;
      const secret = signature.secret;
      const listed = known.get(secret);
      if (listed?.use === "call") {
        return `Action ${name} signs its route with the secret ${secret}, which is set for calls only. Set a route secret with a value of its own.`;
      }
      if (!listed) {
        actions.push(name);
        if (!unset.includes(secret)) unset.push(secret);
      } else if (signature.scheme === "svix" && listed.svixKey === false) {
        return `Action ${name} verifies svix signatures with the secret ${secret}, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_.`;
      }
    }
    const unsetSentence = unsetSecretsSentence(actions, unset);
    if (unsetSentence) return unsetSentence;
  }
  for (const name of names) {
    const call = program.actions[name].call;
    if (!call) continue;
    if (facts.rules) {
      const url = new URL(call.url);
      const host = url.hostname.replace(/\.+$/, "").toLowerCase();
      if (facts.rules.installationHosts.some((editor) => editor.toLowerCase() === host)) {
        return `Action ${name} calls ${host}, the editor's host. A call reaches an app or an outside service, never the editor.`;
      }
      const suffix = facts.rules.suffix.replace(/^\.+/, "").toLowerCase();
      const installation = suffix !== "" && (host === suffix || host.endsWith(`.${suffix}`));
      if (url.protocol !== "https:" && !installation) {
        return `Action ${name} calls http://${host}, which is not an app of this installation. Use https.`;
      }
    }
    const secret = call.auth ? ("secret" in call.auth ? call.auth.secret : call.auth.key) : null;
    if (known && secret && !known.has(secret)) {
      return `Action ${name} calls with the secret ${secret}, which is not set for this project. Set it with secret_set before publishing.`;
    }
  }
  return null;
}

/** kernel-rs/src/http.rs `unset_secrets_message`: one sentence for every unset route secret. */
function unsetSecretsSentence(actions: string[], secrets: string[]): string | null {
  const list = (items: string[]) => items.length === 1 ? items[0] : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
  if (actions.length === 0) return null;
  if (actions.length === 1 && secrets.length === 1) {
    return `Action ${actions[0]} signs with the secret ${secrets[0]}, which is not set for this project. Set it with secret_set before publishing.`;
  }
  if (secrets.length === 1) {
    return `Actions ${list(actions)} sign with the secret ${secrets[0]}, which is not set for this project. Set it with secret_set before publishing.`;
  }
  return `Actions ${list(actions)} sign with the secrets ${list(secrets)}, which are not set for this project. Set each with secret_set before publishing.`;
}

/**
 * True when a publish refusal asks the owner to set or replace a secret: a
 * route or a call names one that is not set, a route signs with a secret set
 * for calls only, or a svix secret is not a whsec_ key. The Kernel's sentence
 * names the MCP tool; in the editor the owner acts in Auth > Integrations.
 */
export function refusalNeedsSecret(sentence: string): boolean {
  return /which (is|are) not set for this project|which is set for calls only|which is not a whsec_ signing secret/.test(sentence);
}
