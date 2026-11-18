const secretAliases = new Set(["apikey", "xapikey", "accesstoken", "refreshtoken", "authorization", "cookie", "setcookie", "credential", "credentials", "secret", "password", "invitationcode", "verificationtoken", "ownertoken"]);
const credentialMarker = /\b(?:sk-ant-[A-Za-z0-9_-]{8,}|sk-or-v1-[A-Fa-f0-9]{16,}|sk-proj-[A-Za-z0-9_-]{8,}|lb_[A-Za-z0-9_-]{12,}|Bearer\s+[A-Za-z0-9._~+/-]{12,})\b/g;
const sensitiveParameter = /^(?:token|key|api[_-]?key|code|invite|invitation|invitation[_-]?code|verification|verification[_-]?token|email[_-]?verification|auth|authorization|signature)$/i;

export type BoundedText = { text: string; originalLength: number; retainedLength: number; truncated: boolean; redacted: boolean; method: "head" };

/** Redact before persistence. This is trace hygiene, not an authorization boundary. */
export function redactDurableValue(value: unknown, knownCredentials: readonly string[] = []): unknown {
  if (typeof value === "string") return redactText(value, knownCredentials);
  if (Array.isArray(value)) return value.map((item) => redactDurableValue(item, knownCredentials));
  if (!value || typeof value !== "object") return value;
  const entries: Array<[string, unknown]> = [];
  for (const [key, item] of Object.entries(value)) {
    const normalized = key.replace(/[^a-z0-9]/gi, "").toLowerCase();
    if (secretAliases.has(normalized) || normalized === "clientsecret") entries.push([key, "[redacted]"]);
    else entries.push([key, redactDurableValue(item, knownCredentials)]);
  }
  return Object.fromEntries(entries);
}

export function redactText(value: string, knownCredentials: readonly string[] = []): string {
  let credentialSafe = value.replace(credentialMarker, "[redacted credential]");
  for (const credential of knownCredentials) if (credential) credentialSafe = credentialSafe.split(credential).join("[redacted credential]");
  credentialSafe = credentialSafe.replace(/https?:\/\/[^\s"'<>]+/gi, (candidate) => redactUrl(candidate));
  try {
    const parsed = JSON.parse(credentialSafe);
    const safe = JSON.stringify(redactDurableValue(parsed, knownCredentials));
    return safe ?? credentialSafe;
  } catch { return credentialSafe; }
}

export function boundModelText(value: unknown, limit = 20_000, knownCredentials: readonly string[] = []): BoundedText {
  const rawSource = typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
  const structured = structuredText(value, knownCredentials);
  const safe = redactText(structured, knownCredentials);
  const text = safe.length > limit ? `${safe.slice(0, limit)}\n… cut at ${limit} characters. Ask for a smaller result.` : safe;
  return { text, originalLength: rawSource.length, retainedLength: text.length, truncated: safe.length > limit, redacted: safe !== rawSource, method: "head" };
}

function structuredText(value: unknown, knownCredentials: readonly string[]) {
  if (typeof value !== "string") return JSON.stringify(redactDurableValue(value, knownCredentials), null, 2) ?? "";
  try { return JSON.stringify(redactDurableValue(JSON.parse(value), knownCredentials), null, 2) ?? ""; }
  catch { return value; }
}

export function declaredIntent(origin: "owner-message" | "assistant-declaration", sourceEntryId: string | undefined, text: string, declaredAt: number) {
  if (!sourceEntryId || !text.trim()) return undefined;
  return { origin, sourceEntryId, text: redactText(text.trim()).slice(0, 500), declaredAt };
}

function redactUrl(value: string) {
  try {
    const url = new URL(value);
    const original = value;
    if (url.username || url.password) { url.username = "[redacted]"; url.password = "[redacted]"; }
    for (const key of [...url.searchParams.keys()]) if (isSensitiveParameter(key) || /invite|verif/i.test(key)) url.searchParams.set(key, "[redacted]");
    if (url.hash && /invite|verif|token|auth|code/i.test(url.hash)) url.hash = "#[redacted]";
    const result = url.toString();
    const originalUrl = new URL(original);
    const sensitive = Boolean(originalUrl.username || originalUrl.password) || [...originalUrl.searchParams.keys()].some((key) => isSensitiveParameter(key) || /invite|verif/i.test(key)) || /#.*(?:invite|verif|token|auth|code)/i.test(original);
    return sensitive ? result : original;
  } catch {
    return value.replace(credentialMarker, "[redacted credential]");
  }
}

function isSensitiveParameter(key: string) {
  const normalized = key.replace(/[^a-z0-9]/gi, "").toLowerCase();
  return secretAliases.has(normalized) || sensitiveParameter.test(key);
}
