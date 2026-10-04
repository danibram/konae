/**
 * A route's signature scheme and its `fields` map, as the Validation Gate
 * reads them (ADR-0015). The same checks, in the same order and with the same
 * messages, as kernel-rs/src/provider.rs `signature` and `fields`;
 * kernel-rs/src/http-binding-cases.json holds the cases both test suites run.
 */

export type SignatureScheme = "littlebox" | "hmac-body" | "timestamped" | "svix";

/**
 * A route's signature. `scheme` is absent for Littlebox's own. `header` is the
 * header the MAC arrives in, `svix-signature` for the svix scheme. `secret`
 * names a value the owner sets with `secret_set`; it is never in actions.json.
 */
export type RouteSignature = {
  header: string;
  secret: string;
  scheme?: Exclude<SignatureScheme, "littlebox">;
  /** hmac-body: the HMAC's hash. */
  algorithm?: "sha256" | "sha1";
  /** hmac-body: how the MAC is written. */
  encoding?: "hex" | "base64";
  /** hmac-body: text before the MAC, such as "sha256=". timestamped: the timestamp field, such as "t=". */
  prefix?: string;
  /** timestamped: the field that carries the MAC, such as "v1=". */
  tag?: string;
  /** timestamped: how far `t` may sit from the server's clock, in seconds. */
  window?: number;
};

const SCHEMES: readonly SignatureScheme[] = ["littlebox", "hmac-body", "timestamped", "svix"];
const SCHEME_KEYS: Record<SignatureScheme, readonly string[]> = {
  littlebox: ["scheme", "header", "secret"],
  "hmac-body": ["scheme", "header", "secret", "algorithm", "encoding", "prefix"],
  timestamped: ["scheme", "header", "secret", "prefix", "tag", "window"],
  svix: ["scheme", "secret"],
};
const SIGNATURE_HEADER = /^[A-Za-z0-9-]{1,64}$/;
const UNSIGNABLE_HEADERS = new Set(["authorization", "cookie", "host", "content-type", "content-length"]);
const SECRET_NAME = /^[a-z][a-z0-9_]{0,63}$/;
const BODY_PREFIX = /^[A-Za-z0-9=\-_.:]{1,32}$/;
const FIELD_PREFIX = /^[A-Za-z0-9_-]{1,15}=$/;
const MAX_WINDOW = 3600;
const MAX_FIELDS = 32;
const MAX_PATH_STEPS = 16;
const MAX_PATH_LENGTH = 200;
const PATH_STEP = /\.([A-Za-z0-9_-]{1,64})|\[(0|[1-9][0-9]{0,2})\]/y;

function quotedList(keys: readonly string[]): string {
  const quoted = keys.map((key) => `"${key}"`);
  return quoted.length === 1 ? quoted[0] : `${quoted.slice(0, -1).join(", ")} and ${quoted[quoted.length - 1]}`;
}

/** Parses a route's `signature`: its scheme, the keys that scheme takes, the header, the secret, then the scheme's own settings. */
export function parseRouteSignature(name: string, method: "GET" | "POST", value: unknown): RouteSignature {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`Action ${name} has an invalid http binding.`);
  const record = value as Record<string, unknown>;
  const scheme = record.scheme === undefined ? "littlebox" : SCHEMES.find((candidate) => candidate === record.scheme);
  if (!scheme) throw new Error(`Action ${name} names an unknown signature scheme. Use "littlebox", "hmac-body", "timestamped" or "svix".`);
  const keys = SCHEME_KEYS[scheme];
  if (Object.keys(record).some((key) => !keys.includes(key))) {
    throw new Error(`Action ${name} has an invalid ${scheme} signature. It takes ${quotedList(keys)}.`);
  }
  if (method === "GET" && scheme !== "littlebox") {
    throw new Error(`Action ${name} binds GET, so its signature must use the littlebox scheme. A provider's scheme signs a POST body.`);
  }
  let header = "svix-signature";
  if (scheme !== "svix") {
    if (typeof record.header !== "string" || !SIGNATURE_HEADER.test(record.header) || UNSIGNABLE_HEADERS.has(record.header.toLowerCase())) {
      throw new Error(`Action ${name} has an invalid signature header.`);
    }
    header = record.header;
  }
  if (typeof record.secret !== "string" || !SECRET_NAME.test(record.secret)) {
    throw new Error(`Action ${name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter.`);
  }
  const secret = record.secret;
  if (scheme === "hmac-body") {
    if (record.algorithm !== "sha256" && record.algorithm !== "sha1") throw new Error(`Action ${name} has an invalid signature algorithm. Use "sha256" or "sha1".`);
    if (record.encoding !== "hex" && record.encoding !== "base64") throw new Error(`Action ${name} has an invalid signature encoding. Use "hex" or "base64".`);
    if (record.prefix !== undefined && (typeof record.prefix !== "string" || !BODY_PREFIX.test(record.prefix))) {
      throw new Error(`Action ${name} has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256=".`);
    }
    return { scheme, header, secret, algorithm: record.algorithm, encoding: record.encoding, ...(record.prefix === undefined ? {} : { prefix: record.prefix }) };
  }
  if (scheme === "timestamped") {
    if (typeof record.prefix !== "string" || !FIELD_PREFIX.test(record.prefix)) {
      throw new Error(`Action ${name} has an invalid timestamp prefix. Use the field's name and "=", such as "t=".`);
    }
    if (typeof record.tag !== "string" || !FIELD_PREFIX.test(record.tag) || record.tag === record.prefix) {
      throw new Error(`Action ${name} has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's.`);
    }
    if (typeof record.window !== "number" || !Number.isInteger(record.window) || record.window < 1 || record.window > MAX_WINDOW) {
      throw new Error(`Action ${name} has an invalid signature window. Use a whole number of seconds from 1 to 3600.`);
    }
    return { scheme, header, secret, prefix: record.prefix, tag: record.tag, window: record.window };
  }
  return scheme === "svix" ? { scheme, header, secret } : { header, secret };
}

/** The steps of a JSON path: `$`, then 1 to 16 of `.name` or `[index]`. Null when it is not one. */
export function jsonPathSteps(path: string): Array<string | number> | null {
  if (path.length > MAX_PATH_LENGTH || !path.startsWith("$")) return null;
  const steps: Array<string | number> = [];
  PATH_STEP.lastIndex = 1;
  while (PATH_STEP.lastIndex < path.length) {
    const match = PATH_STEP.exec(path);
    if (!match) return null;
    steps.push(match[1] ?? Number(match[2]));
  }
  return steps.length >= 1 && steps.length <= MAX_PATH_STEPS ? steps : null;
}

/** Parses a route's `fields`, input keys in sorted order, each one of the action's input bindings and mapped to a JSON path. */
export function parseRouteFields(name: string, method: "GET" | "POST", inputs: ReadonlySet<string>, value: unknown): Record<string, string> {
  if (method === "GET") throw new Error(`Action ${name} binds GET, so it cannot declare fields. Fields read a POST body.`);
  const count = value && typeof value === "object" && !Array.isArray(value) ? Object.keys(value).length : 0;
  if (count < 1 || count > MAX_FIELDS) {
    throw new Error(`Action ${name} has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id".`);
  }
  const record = value as Record<string, unknown>;
  const fields: Record<string, string> = {};
  for (const key of Object.keys(record).sort()) {
    if (!inputs.has(key)) throw new Error(`Action ${name} maps ${key}, which is not one of its input bindings.`);
    const path = record[key];
    if (typeof path !== "string" || !jsonPathSteps(path)) {
      throw new Error(`Action ${name} maps ${key} to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id".`);
    }
    fields[key] = path;
  }
  return fields;
}

/**
 * The POST body a sender writes so a route with `fields` reads `input`: each
 * value at its path, every other key at the top level. The inverse of the
 * Kernel's projection, for the conformance runner and the editor's curl.
 */
export function bodyFromFields(fields: Record<string, string> | undefined, input: Record<string, unknown>): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    const steps = fields?.[key] ? jsonPathSteps(fields[key]) : null;
    if (!steps) {
      body[key] = value;
      continue;
    }
    let node: Record<string | number, unknown> = body;
    steps.forEach((step, index) => {
      if (index === steps.length - 1) {
        node[step] = value;
        return;
      }
      const next = steps[index + 1];
      if (node[step] === undefined || typeof node[step] !== "object") node[step] = typeof next === "number" ? [] : {};
      node = node[step] as Record<string | number, unknown>;
    });
  }
  return body;
}
