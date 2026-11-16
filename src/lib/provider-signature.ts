import type { RouteSignature } from "../kernel/route-declaration";

const encoder = new TextEncoder();

async function hmac(hash: "SHA-256" | "SHA-1", key: Uint8Array<ArrayBuffer>, message: string): Promise<Uint8Array> {
  const imported = await crypto.subtle.importKey("raw", key, { name: "HMAC", hash }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", imported, encoder.encode(message)));
}

const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
const base64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

export function svixKey(secret: string): Uint8Array<ArrayBuffer> {
  const decoded = atob(secret.replace(/^whsec_/, ""));
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

export async function signDelivery(
  signature: RouteSignature,
  secret: string,
  body: string,
  options: { t?: number; id?: string } = {},
): Promise<Record<string, string>> {
  const t = options.t ?? Math.floor(Date.now() / 1000);
  if (signature.scheme === "hmac-body") {
    const mac = await hmac(signature.algorithm === "sha1" ? "SHA-1" : "SHA-256", encoder.encode(secret), body);
    return { [signature.header]: `${signature.prefix ?? ""}${signature.encoding === "base64" ? base64(mac) : hex(mac)}` };
  }
  if (signature.scheme === "timestamped") {
    const mac = await hmac("SHA-256", encoder.encode(secret), `${t}.${body}`);
    return { [signature.header]: `${signature.prefix}${t},${signature.tag}${hex(mac)}` };
  }
  if (signature.scheme === "svix") {
    const id = options.id ?? `msg_${crypto.randomUUID().replaceAll("-", "")}`;
    const mac = await hmac("SHA-256", svixKey(secret), `${id}.${t}.${body}`);
    return { "svix-id": id, "svix-timestamp": String(t), "svix-signature": `v1,${base64(mac)}` };
  }
  throw new Error("A route in Konae's own scheme is signed with signRoute.");
}
