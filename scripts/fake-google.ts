/**
 * A local stand-in for Google's OAuth endpoints, for trying "Continue with
 * Google" on a local Kernel without a real Google client.
 *
 *   bun run scripts/fake-google.ts            # listens on 127.0.0.1:8799
 *   KONAE_GOOGLE_TEST_ISSUER=http://127.0.0.1:8799 bun run dev:local
 *
 * Then set any client ID and secret for the app in Access > Users and turn
 * Google on. /authorize shows a form where you pick the Google subject, the
 * email, its Workspace domain (hd) and whether Google verified it. /token
 * checks that the client sends the secret it used, the redirect URI and
 * PKCE, and answers an ID token signed with the Kernel's RS256 test key.
 * /jwks serves the matching public key. A development build of the Kernel
 * reads KONAE_GOOGLE_TEST_ISSUER only with a local HTTP public origin; a
 * release build refuses to start with it set.
 */
import { createHash, createPrivateKey, randomBytes, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";

const port = Number(process.env.FAKE_GOOGLE_PORT ?? 8799);
const issuer = `http://127.0.0.1:${port}`;
const fixtures = path.join(import.meta.dir, "../kernel-rs/test-fixtures/clerk");
const key = createPrivateKey({ key: readFileSync(path.join(fixtures, "test-key.pk8")), format: "der", type: "pkcs8" });
const jwks = readFileSync(path.join(fixtures, "jwks.json"), "utf8");

type Grant = { clientId: string; redirectUri: string; challenge: string; nonce: string; subject: string; email: string; hd: string; verified: boolean };
const grants = new Map<string, Grant>();

const b64url = (bytes: Buffer | string) => Buffer.from(bytes).toString("base64url");
const escape = (value: string) => value.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

function idToken(grant: Grant) {
  const now = Math.floor(Date.now() / 1000);
  const header = b64url(JSON.stringify({ alg: "RS256", kid: "local-test", typ: "JWT" }));
  const claims = b64url(JSON.stringify({
    iss: issuer, aud: grant.clientId, azp: grant.clientId, sub: grant.subject, email: grant.email,
    email_verified: grant.verified, nonce: grant.nonce, iat: now, exp: now + 3600, ...(grant.hd ? { hd: grant.hd } : {}),
  }));
  const signature = sign("sha256", Buffer.from(`${header}.${claims}`), key);
  return `${header}.${claims}.${b64url(signature)}`;
}

function authorizePage(params: URLSearchParams) {
  const hidden = [...params].map(([name, value]) => `<input type="hidden" name="${escape(name)}" value="${escape(value)}">`).join("");
  return `<!doctype html><meta charset="utf-8"><title>Fake Google</title><style>body{font:16px system-ui;max-width:420px;margin:48px auto}label{display:grid;gap:4px;margin:12px 0}input{font:inherit;padding:8px}button{font:inherit;padding:10px 14px}</style>
<h1>Fake Google</h1><p>Choose who signs in. This page stands in for Google on a local Kernel.</p>
<form method="post" action="/authorize">${hidden}
<label>Email <input name="email" type="email" required value="ana@gmail.com"></label>
<label>Workspace domain (hd), empty for Gmail <input name="hd" value=""></label>
<label>Subject <input name="subject" required value="100000000000000000001"></label>
<label><span><input name="verified" type="checkbox" checked> Google verified this email</span></label>
<button name="decision" value="allow">Continue</button> <button name="decision" value="deny" formnovalidate>Cancel</button></form>`;
}

Bun.serve({
  port,
  hostname: "127.0.0.1",
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/jwks") return new Response(jwks, { headers: { "content-type": "application/json", "cache-control": "public, max-age=300" } });
    if (url.pathname === "/authorize" && request.method === "GET") {
      return new Response(authorizePage(url.searchParams), { headers: { "content-type": "text/html; charset=utf-8" } });
    }
    if (url.pathname === "/authorize" && request.method === "POST") {
      const form = new URLSearchParams(await request.text());
      const redirect = new URL(form.get("redirect_uri") ?? "");
      redirect.searchParams.set("state", form.get("state") ?? "");
      if (form.get("decision") !== "allow") {
        redirect.searchParams.set("error", "access_denied");
        return Response.redirect(redirect.toString(), 302);
      }
      const code = randomBytes(16).toString("hex");
      grants.set(code, {
        clientId: form.get("client_id") ?? "", redirectUri: form.get("redirect_uri") ?? "", challenge: form.get("code_challenge") ?? "",
        nonce: form.get("nonce") ?? "", subject: form.get("subject") ?? "", email: form.get("email") ?? "", hd: form.get("hd") ?? "", verified: form.get("verified") === "on",
      });
      redirect.searchParams.set("code", code);
      return Response.redirect(redirect.toString(), 302);
    }
    if (url.pathname === "/token" && request.method === "POST") {
      const form = new URLSearchParams(await request.text());
      const grant = grants.get(form.get("code") ?? "");
      grants.delete(form.get("code") ?? "");
      const verifier = createHash("sha256").update(form.get("code_verifier") ?? "").digest("base64url");
      // Any client is registered here; the client must still send a secret and the same redirect URI.
      if (!grant || !form.get("client_secret") || form.get("client_id") !== grant.clientId || form.get("redirect_uri") !== grant.redirectUri || verifier !== grant.challenge) {
        return Response.json({ error: "invalid_grant" }, { status: 400 });
      }
      return Response.json({ id_token: idToken(grant), token_type: "Bearer", expires_in: 3600 });
    }
    return new Response("Not found", { status: 404 });
  },
});
console.log(`Fake Google listening on ${issuer}`);
