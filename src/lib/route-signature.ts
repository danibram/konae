export type SignedRequest = { method: "GET" | "POST"; path: string; query?: string; body?: string };

export function canonicalQuery(query: string): string {
  const pairs = [...new URLSearchParams(query)];
  pairs.sort(([a, x], [b, y]) => compareBytes(a, b) || compareBytes(x, y));
  return new URLSearchParams(pairs).toString();
}

export function routeNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

export async function signRoute(secret: string, request: SignedRequest, t = Math.floor(Date.now() / 1000), nonce = routeNonce()): Promise<string> {
  const message = `${t}\n${nonce}\n${request.method}\n${request.path}\n${canonicalQuery(request.query ?? "")}\n${request.body ?? ""}`;
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message)));
  return `t=${t},n=${nonce},v1=${Array.from(mac, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function compareBytes(a: string, b: string): number {
  const left = new TextEncoder().encode(a);
  const right = new TextEncoder().encode(b);
  for (let index = 0; index < Math.min(left.length, right.length); index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return left.length - right.length;
}
