// Calls between apps, docs/adr/0019-an-app-calls-another-from-the-kernel.md.
//
// The same checks, in the same order and with the same messages, as
// kernel-rs/src/calls.rs `declaration`, `check_program` and `check_connects`.
// kernel-rs/src/call-cases.json holds the cases both test suites run. The
// checks that need the installation, http only to its own apps and never the
// editor's host, and the secrets a call names, are the Kernel's alone.

/** How a call proves itself to the app it calls. */
export type CallAuth =
  /** Signs with Littlebox's route scheme in `header`, with the secret's value. */
  | { secret: string; header: string }
  /** Sends the secret's value, an API key, as `Authorization: Bearer`. */
  | { key: string };

/** The mutation that runs when a call ends, with inputs read from the answer. */
export type CallThen = { action: string; inputs: Record<string, string> };

/** What one body key or header sends: an input's value, or a constant fixed in actions.json. */
export type CallBodyValue = string | { value: string | number | boolean | null };

export type CallDeclaration = {
  url: string;
  method: "GET" | "POST";
  /** Body key to what it sends. A GET sends them as query parameters. A POST expands dotted keys into nested JSON. */
  body: Record<string, CallBodyValue>;
  /** Extra request headers. Only Idempotency-Key is allowed. */
  headers?: Record<string, CallBodyValue>;
  auth?: CallAuth;
  then?: CallThen;
};

/** What `miniapp.actions.call` answers for a call: the Kernel queued it. */
export type CallQueued = { callId: string; queued: true };

const CALL_KEYS = ["url", "method", "body", "headers", "auth", "then"] as const;
const MAX_URL_CHARS = 2048;
const MAX_BODY_KEYS = 32;
const MAX_HEADERS = 8;
const MAX_POINTER_CHARS = 200;
const MAX_CONSTANT_CHARS = 1000;
const MAX_THEN_INPUTS = 100;
const RESERVED_SEGMENTS = new Set(["_littlebox", "oauth", ".well-known", "mcp"]);
const ANSWER_ROOTS = new Set(["ok", "status", "error", "body", "input"]);
const LOCAL_NAMES = new Set(["localhost", "metadata", "instance-data"]);
const SECRET_NAME = /^[a-z][a-z0-9_]{0,63}$/;
const SIGNATURE_HEADER = /^[A-Za-z0-9-]{1,64}$/;
const UNSIGNABLE_HEADERS = new Set(["authorization", "cookie", "host", "content-type", "content-length"]);
const ALLOWED_CALL_HEADER = "Idempotency-Key";
const IPV4 = /^\d+\.\d+\.\d+\.\d+$/;
export const DEFAULT_SIGNATURE_HEADER = "X-Littlebox-Signature";

type Kind = "query" | "mutation" | "signal" | "call";
type Binding = { source: string; key?: string; nullable?: boolean };

/** Refuses url, method, body, headers, auth and then on an action that is not a call. */
export function assertNoCallKeys(name: string, kind: Exclude<Kind, "call">, candidate: Record<string, unknown>) {
  if (CALL_KEYS.some((key) => candidate[key] != null)) {
    const word = kind === "signal" ? "signal action" : kind;
    throw new Error(`Action ${name} is a ${word}, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call".`);
  }
}

/** Checks one call's own keys. `assertCallTargets` checks its then against the program. */
export function parseCall(name: string, bindings: Binding[], candidate: Record<string, unknown>): CallDeclaration {
  const statement = candidate.statement;
  const hasStatement = statement != null && !(typeof statement === "string" && !statement.trim());
  if (hasStatement || candidate.signal != null || candidate.guard != null || candidate.http != null) {
    throw new Error(`Action ${name} is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route.`);
  }
  if (bindings.some((binding) => binding.source !== "input")) throw new Error(`Action ${name} is a call, so its bindings may only be inputs.`);
  const method = candidate.method === "GET" || candidate.method === "POST" ? candidate.method : null;
  if (!method) throw new Error(`Action ${name} must call with "method": "GET" or "POST".`);
  const url = staticUrl(name, candidate.url);
  const inputs = bindings.flatMap((binding) => binding.key === undefined ? [] : [binding.key]);
  const body = bodyMap(name, method, candidate.body, inputs);
  const headers = headersMap(name, candidate.headers, inputs);
  const auth = parseAuth(name, candidate.auth);
  const then = parseThen(name, candidate.then, inputs);
  return {
    url,
    method,
    body,
    ...(Object.keys(headers).length ? { headers } : {}),
    ...(auth ? { auth } : {}),
    ...(then ? { then } : {}),
  };
}

function staticUrl(name: string, value: unknown): string {
  const invalid = () => new Error(`Action ${name} has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters.`);
  if (typeof value !== "string") throw invalid();
  // Code points, as the Kernel counts characters.
  if ([...value].length > MAX_URL_CHARS || value.includes("#") || [...value].some((c) => c.codePointAt(0)! <= 0x20 || c.codePointAt(0) === 0x7f)) throw invalid();
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw invalid();
  }
  if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password || !url.hostname) throw invalid();
  const host = url.hostname.toLowerCase();
  if (host.startsWith("[") || IPV4.test(host)) throw new Error(`Action ${name} calls the IP address ${host}. Name a host instead.`);
  const bare = host.replace(/\.+$/, "");
  if (LOCAL_NAMES.has(bare) || bare.endsWith(".internal")) {
    throw new Error(`Action ${name} calls ${host}, which names this machine or a cloud metadata service.`);
  }
  if (RESERVED_SEGMENTS.has((url.pathname.split("/")[1] ?? "").toLowerCase())) {
    throw new Error(`Action ${name} calls a path under /_littlebox, /oauth, /.well-known or /mcp, which belong to the Kernel.`);
  }
  return value;
}

/** Sorted the way the Kernel sorts strings: by bytes, which for ASCII is by code unit. */
function sortedEntries(record: Record<string, unknown>): [string, unknown][] {
  return Object.entries(record).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

/** A constant body value: { "value": <string of at most 1000 characters, number, boolean or null> }. */
function isConstant(value: unknown): value is { value: string | number | boolean | null } {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== "value") return false;
  const constant = (value as { value: unknown }).value;
  if (typeof constant === "string") return [...constant].length <= MAX_CONSTANT_CHARS;
  return constant === null || typeof constant === "boolean" || (typeof constant === "number" && Number.isFinite(constant));
}

const MAX_BODY_INDEX = 31;

function validBodySegment(segment: string): boolean {
  if (!segment) return false;
  if (/^\d+$/.test(segment)) {
    if (!(segment === "0" || !segment.startsWith("0"))) return false;
    const index = Number(segment);
    return Number.isInteger(index) && index >= 0 && index <= MAX_BODY_INDEX;
  }
  return /^[A-Za-z_][A-Za-z0-9_-]*$/.test(segment);
}

function validBodyKey(key: string): boolean {
  return key.length >= 1 && key.length <= 64 && key.split(".").every(validBodySegment);
}

function bodyMap(name: string, method: "GET" | "POST", value: unknown, inputs: string[]): Record<string, CallBodyValue> {
  if (value == null) return {};
  const invalid = () => new Error(`Action ${name} has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32.`);
  if (typeof value !== "object" || Array.isArray(value)) throw invalid();
  const entries = sortedEntries(value as Record<string, unknown>);
  if (entries.length > MAX_BODY_KEYS || entries.some(([key, sent]) => !validBodyKey(key) || !(typeof sent === "string" || isConstant(sent)))) throw invalid();
  for (let index = 0; index < entries.length; index++) {
    const key = entries[index][0];
    if (method === "GET" && key.includes(".")) {
      throw new Error(`Action ${name} sends body key ${key} with a path. Nested body keys belong on POST.`);
    }
    for (const [other] of entries.slice(index + 1)) {
      if (other.startsWith(`${key}.`)) {
        throw new Error(`Action ${name} has body keys ${key} and ${other}, which overlap. Use one path or the other.`);
      }
    }
  }
  for (const [key, sent] of entries) {
    if (typeof sent === "string" && !inputs.includes(sent)) throw new Error(`Action ${name} sends body key ${key} from ${sent}, which is not one of its input bindings.`);
  }
  return Object.fromEntries(entries) as Record<string, CallBodyValue>;
}

function headersMap(name: string, value: unknown, inputs: string[]): Record<string, CallBodyValue> {
  if (value == null) return {};
  const invalid = () => new Error(`Action ${name} has an invalid headers. Map Idempotency-Key to an input or a constant, at most 8.`);
  if (typeof value !== "object" || Array.isArray(value)) throw invalid();
  const entries = sortedEntries(value as Record<string, unknown>);
  if (entries.length === 0 || entries.length > MAX_HEADERS) throw invalid();
  for (const [key, sent] of entries) {
    if (key !== ALLOWED_CALL_HEADER) {
      throw new Error(`Action ${name} sets header ${key}, which is not allowed. Only Idempotency-Key is allowed.`);
    }
    if (!(typeof sent === "string" || isConstant(sent))) throw invalid();
    if (isConstant(sent) && typeof sent.value !== "string" && typeof sent.value !== "number" && typeof sent.value !== "boolean") {
      throw invalid();
    }
    if (typeof sent === "string" && !inputs.includes(sent)) {
      throw new Error(`Action ${name} sends header ${key} from ${sent}, which is not one of its input bindings.`);
    }
  }
  return Object.fromEntries(entries) as Record<string, CallBodyValue>;
}

function parseAuth(name: string, value: unknown): CallAuth | undefined {
  if (value == null) return undefined;
  const invalid = () => new Error(`Action ${name} has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds.`);
  if (typeof value !== "object" || Array.isArray(value)) throw invalid();
  const record = value as Record<string, unknown>;
  const signs = "secret" in record;
  const keyed = "key" in record;
  const allowed = signs ? ["secret", "header"] : ["key"];
  if (signs === keyed || Object.keys(record).some((key) => !allowed.includes(key))) throw invalid();
  const secret = signs ? record.secret : record.key;
  if (typeof secret !== "string" || !SECRET_NAME.test(secret)) {
    throw new Error(`Action ${name} names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter.`);
  }
  if (!signs) return { key: secret };
  if (record.header === undefined) return { secret, header: DEFAULT_SIGNATURE_HEADER };
  const header = record.header;
  if (typeof header !== "string" || !SIGNATURE_HEADER.test(header) || UNSIGNABLE_HEADERS.has(header.toLowerCase())) {
    throw new Error(`Action ${name} has an invalid signature header.`);
  }
  return { secret, header };
}

function parseThen(name: string, value: unknown, inputs: string[]): CallThen | undefined {
  if (value == null) return undefined;
  const invalid = () => new Error(`Action ${name} has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer.`);
  if (typeof value !== "object" || Array.isArray(value)) throw invalid();
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => key !== "action" && key !== "inputs")) throw invalid();
  if (typeof record.action !== "string" || !record.action) throw invalid();
  const mapped = record.inputs === undefined ? {} : record.inputs;
  if (!mapped || typeof mapped !== "object" || Array.isArray(mapped)) throw invalid();
  const entries = sortedEntries(mapped as Record<string, unknown>);
  if (entries.length > MAX_THEN_INPUTS || entries.some(([key, pointer]) => !key || typeof pointer !== "string")) throw invalid();
  for (const [, value] of entries) {
    const pointer = value as string;
    const segments = pointer.split("/").slice(1);
    const escapesValid = pointer.split("~").slice(1).every((rest) => rest.startsWith("0") || rest.startsWith("1"));
    if ([...pointer].length > MAX_POINTER_CHARS || !pointer.startsWith("/") || !escapesValid || !ANSWER_ROOTS.has(segments[0])) {
      throw new Error(`Action ${name} reads ${pointer} for then, which is not a JSON pointer into /ok, /status, /error, /body or /input.`);
    }
    if (segments[0] === "input" && segments.length > 1) {
      const key = segments[1].replaceAll("~1", "/").replaceAll("~0", "~");
      if (!inputs.includes(key)) throw new Error(`Action ${name} reads ${pointer} for then, but ${key} is not one of its input bindings.`);
    }
  }
  return { action: record.action, inputs: Object.fromEntries(entries) as Record<string, string> };
}

type ProgramAction = { kind: Kind; http?: unknown; bindings: Binding[]; call?: CallDeclaration };

/** Checks each call's then against the program, in sorted name order. */
export function assertCallTargets(actions: Record<string, ProgramAction>) {
  for (const name of Object.keys(actions).sort()) {
    const then = actions[name].call?.then;
    if (!then) continue;
    const target = then.action;
    const mutation = Object.hasOwn(actions, target) ? actions[target] : undefined;
    if (!mutation) throw new Error(`Action ${name} names ${target} as its then, which is not declared.`);
    if (mutation.kind !== "mutation") throw new Error(`Action ${name} names ${target} as its then, which is not a mutation.`);
    if (mutation.http) throw new Error(`Action ${name} names ${target} as its then, which answers on an HTTP route. A then mutation runs only after its call.`);
    if (mutation.bindings.some((binding) => binding.source === "actor")) {
      throw new Error(`Action ${name} names ${target} as its then, which binds the actor. A then mutation runs with no caller.`);
    }
    const inputs = mutation.bindings.filter((binding) => binding.source === "input");
    for (const key of Object.keys(then.inputs).sort()) {
      if (!inputs.some((binding) => binding.key === key)) throw new Error(`Action ${name} fills ${key}, which is not an input of ${target}.`);
    }
    const required = inputs.filter((binding) => !binding.nullable).map((binding) => binding.key!).sort();
    for (const key of required) {
      if (!(key in then.inputs)) throw new Error(`Action ${name} leaves ${key} of ${target} unfilled. Fill it from the answer or make the input nullable.`);
    }
  }
}

/** The call that names `action` as its then. Such a mutation runs only after its call. */
export function thenOf(actions: Record<string, ProgramAction>, action: string): string | null {
  return Object.keys(actions).sort().find((name) => actions[name].call?.then?.action === action) ?? null;
}

// The manifest's connects: origins the page may reach with fetch.
const MAX_CONNECTS = 10;

export function checkConnects(value: unknown): string[] | undefined {
  if (value == null) return undefined;
  const invalid = () => new Error('connects must be a list of at most 10 origins, such as ["https://api.example.com"].');
  if (!Array.isArray(value) || value.length > MAX_CONNECTS) throw invalid();
  const seen: string[] = [];
  for (const entry of value) {
    if (typeof entry !== "string") throw invalid();
    let origin: string | null = null;
    try {
      const url = new URL(entry);
      // A browser treats localhost and its subdomains as secure, and a local
      // installation serves its apps there over http.
      const local = url.hostname === "localhost" || url.hostname.endsWith(".localhost");
      if ((url.protocol === "https:" || (url.protocol === "http:" && local)) && !entry.includes("*")) origin = url.origin;
    } catch {}
    if (origin !== entry) {
      throw new Error(`connects lists ${entry}, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost.`);
    }
    if (seen.includes(entry)) throw new Error(`connects lists ${entry} twice.`);
    seen.push(entry);
  }
  return seen;
}
