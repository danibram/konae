/**
 * The signature a sender puts on a request to a signed HTTP route, as the
 * Kernel checks it in kernel-rs/src/http.rs. The header is
 * `t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256>`, keyed with the secret,
 * over `t`, the nonce, the method, the path, the canonical query string and
 * the raw body, joined by "\n". kernel-rs/src/signature-cases.json holds
 * vectors both sides compute.
 */
export type SignedRequest = { method: "GET" | "POST"; path: string; query?: string; body?: string };

/** The query string's pairs sorted by key, then value, written as application/x-www-form-urlencoded. */
export function canonicalQuery(query: string): string {
  const pairs = [...new URLSearchParams(query)];
  pairs.sort(([a, x], [b, y]) => compareBytes(a, b) || compareBytes(x, y));
  return new URLSearchParams(pairs).toString();
}

/** A fresh nonce: 22 URL-safe characters. */
export function routeNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

/** The header value for `request`, signed with `secret` at `t` with `nonce`. */
export async function signRoute(secret: string, request: SignedRequest, t = Math.floor(Date.now() / 1000), nonce = routeNonce()): Promise<string> {
  const message = `${t}\n${nonce}\n${request.method}\n${request.path}\n${canonicalQuery(request.query ?? "")}\n${request.body ?? ""}`;
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message)));
  return `t=${t},n=${nonce},v1=${Array.from(mac, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

/** UTF-8 byte order, as the Kernel sorts. */
function compareBytes(a: string, b: string): number {
  const left = new TextEncoder().encode(a);
  const right = new TextEncoder().encode(b);
  for (let index = 0; index < Math.min(left.length, right.length); index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return left.length - right.length;
}
