/**
 * A route's signature scheme and its `fields` map, as the Validation Gate
 * reads them (ADR-0018). The same checks, in the same order and with the same
 * messages, as kernel-rs/kernel/src/provider.rs `signature` and `fields`;
 * kernel-rs/kernel/src/http-binding-cases.json and signature-preset-cases.json hold
 * the cases both test suites run.
 */
import signaturePresets from "../../kernel-rs/kernel/src/signature-presets.json";
import { joinRefusals, refusal, type RefusalError } from "./refusal";

export type SignatureScheme = "konae" | "hmac-body" | "timestamped" | "svix";

/**
 * A route's signature. `scheme` is absent for Konae's own. `header` is the
 * header the MAC arrives in, `svix-signature` for the svix scheme. `secret`
 * names a value the owner sets with `secret_set`; it is never in actions.json.
 */
export type RouteSignature = {
  header: string;
  secret: string;
  scheme?: Exclude<SignatureScheme, "konae">;
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

const SCHEMES: readonly SignatureScheme[] = ["konae", "hmac-body", "timestamped", "svix"];
const SCHEME_KEYS: Record<SignatureScheme, readonly string[]> = {
  konae: ["scheme", "header", "secret"],
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

/**
 * The providers a route may name instead of spelling out a scheme, each the
 * long form it stands for without its secret. kernel-rs/kernel/src/provider.rs reads
 * the same file, so both doors expand a preset alike.
 */
const PRESETS: Readonly<Record<string, Record<string, unknown>>> = signaturePresets;
const PROVIDERS = Object.keys(PRESETS).sort();

function quotedChoices(keys: readonly string[]): string {
  const quoted = keys.map((key) => `"${key}"`);
  return quoted.length === 1 ? quoted[0] : `${quoted.slice(0, -1).join(", ")} or ${quoted[quoted.length - 1]}`;
}

/**
 * Parses a route's `signature`. A `provider` names a row of the preset table
 * and takes only a `secret` beside it; it then parses as the long form that
 * row spells.
 */
export function parseRouteSignature(name: string, method: "GET" | "POST", value: unknown): RouteSignature {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw refusal("route.invalid", `Action ${name} has an invalid http binding.`);
  const record = value as Record<string, unknown>;
  if (record.provider === undefined) return parseLongForm(name, method, record);
  const provider = typeof record.provider === "string" && Object.hasOwn(PRESETS, record.provider) ? record.provider : undefined;
  if (provider === undefined) throw refusal("signature.provider-unknown", `Action ${name} names an unknown signature provider. Use ${quotedChoices(PROVIDERS)}.`);
  const extra = Object.keys(record).filter((key) => key !== "provider" && key !== "secret").sort();
  if (extra.length > 0) {
    throw refusal("signature.provider-extra-keys", `Action ${name} names the signature provider "${provider}", so its signature takes only "provider" and "secret". Remove ${quotedList(extra)}.`);
  }
  return parseLongForm(name, method, { ...PRESETS[provider], ...(record.secret === undefined ? {} : { secret: record.secret }) });
}

/**
 * Parses a long-form `signature`: its scheme, then every key that scheme
 * takes. An unknown scheme, and a provider's scheme on GET, stop the parse;
 * every other wrong key adds its sentence, so one refusal names them all.
 */
function parseLongForm(name: string, method: "GET" | "POST", record: Record<string, unknown>): RouteSignature {
  const scheme = record.scheme === undefined ? "konae" : SCHEMES.find((candidate) => candidate === record.scheme);
  if (!scheme) throw refusal("signature.scheme-unknown", `Action ${name} names an unknown signature scheme. Use "konae", "hmac-body", "timestamped" or "svix".`);
  const problems: RefusalError[] = [];
  const keys = SCHEME_KEYS[scheme];
  if (Object.keys(record).some((key) => !keys.includes(key))) {
    problems.push(refusal("signature.keys-invalid", `Action ${name} has an invalid ${scheme} signature. It takes ${quotedList(keys)}.`));
  }
  if (method === "GET" && scheme !== "konae") {
    problems.push(refusal("signature.provider-on-get", `Action ${name} binds GET, so its signature must use the konae scheme. A provider's scheme signs a POST body.`));
    throw joinRefusals(problems);
  }
  const header =
    scheme === "svix"
      ? "svix-signature"
      : typeof record.header === "string" && SIGNATURE_HEADER.test(record.header) && !UNSIGNABLE_HEADERS.has(record.header.toLowerCase())
        ? record.header
        : undefined;
  if (header === undefined) problems.push(refusal("signature.header-invalid", `Action ${name} has an invalid signature header.`));
  const secret = typeof record.secret === "string" && SECRET_NAME.test(record.secret) ? record.secret : undefined;
  if (secret === undefined) problems.push(refusal("signature.secret-invalid", `Action ${name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter.`));
  let settings: Omit<RouteSignature, "header" | "secret"> = {};
  if (scheme === "hmac-body") {
    const { algorithm, encoding, prefix } = record;
    if (algorithm !== "sha256" && algorithm !== "sha1") problems.push(refusal("signature.algorithm-invalid", `Action ${name} has an invalid signature algorithm. Use "sha256" or "sha1".`));
    if (encoding !== "hex" && encoding !== "base64") problems.push(refusal("signature.encoding-invalid", `Action ${name} has an invalid signature encoding. Use "hex" or "base64".`));
    if (prefix !== undefined && (typeof prefix !== "string" || !BODY_PREFIX.test(prefix))) {
      problems.push(refusal("signature.prefix-invalid", `Action ${name} has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256=".`));
    }
    settings = { scheme, algorithm: algorithm as "sha256" | "sha1", encoding: encoding as "hex" | "base64", ...(prefix === undefined ? {} : { prefix: prefix as string }) };
  } else if (scheme === "timestamped") {
    const { prefix, tag, window } = record;
    if (typeof prefix !== "string" || !FIELD_PREFIX.test(prefix)) {
      problems.push(refusal("signature.timestamp-prefix-invalid", `Action ${name} has an invalid timestamp prefix. Use the field's name and "=", such as "t=".`));
    }
    if (typeof tag !== "string" || !FIELD_PREFIX.test(tag) || tag === prefix) {
      problems.push(refusal("signature.tag-invalid", `Action ${name} has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's.`));
    }
    if (typeof window !== "number" || !Number.isInteger(window) || window < 1 || window > MAX_WINDOW) {
      problems.push(refusal("signature.window-invalid", `Action ${name} has an invalid signature window. Use a whole number of seconds from 1 to 3600.`));
    }
    settings = { scheme, prefix: prefix as string, tag: tag as string, window: window as number };
  } else if (scheme === "svix") {
    settings = { scheme };
  }
  if (problems.length > 0 || header === undefined || secret === undefined) throw joinRefusals(problems);
  const { scheme: named, ...rest } = settings;
  return named === undefined ? { header, secret } : { scheme: named, header, secret, ...rest };
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
  if (method === "GET") throw refusal("route.fields-on-get", `Action ${name} binds GET, so it cannot declare fields. Fields read a POST body.`);
  const count = value && typeof value === "object" && !Array.isArray(value) ? Object.keys(value).length : 0;
  if (count < 1 || count > MAX_FIELDS) {
    throw refusal("route.fields-invalid", `Action ${name} has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id".`);
  }
  const record = value as Record<string, unknown>;
  const fields: Record<string, string> = {};
  for (const key of Object.keys(record).sort()) {
    if (!inputs.has(key)) throw refusal("route.field-unbound", `Action ${name} maps ${key}, which is not one of its input bindings.`);
    const path = record[key];
    if (typeof path !== "string" || !jsonPathSteps(path)) {
      throw refusal("route.field-path-invalid", `Action ${name} maps ${key} to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id".`);
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
