// Probe: a custom domain (ADR-0014). It serves a public guestbook from guestbook.example.org next to its generated hostname.
// A fake resolver on a local UDP port answers the Kernel's own TXT lookup;
// requests reach the alias with a Host header, as a proxy that forwards Host
// unchanged sends them.
//
//   KONAE_PROBE_PORT=23060 bun run examples/probes/domain/replay.ts
import assert from "node:assert/strict";
import { probe } from "../_kit";
import { fakeResolver } from "../_dns";

const here = import.meta.dir;
const alias = "guestbook.example.org";
const record = `_konae.${alias}`;
const dns = await fakeResolver();

type Domain = { status: string; hostname: string | null; url?: string; record?: { name: string; value: string }; dns?: string };

await probe("domain", 23060, async (kit) => {
  const { url, owner } = kit;
  const port = new URL(url).port;
  const aliasHost = `${alias}:${port}`;
  const aliasOrigin = `http://${aliasHost}`;
  const app = await kit.publish(here, { projectId: "probe_domain", visibility: "public", signup: { mode: "open" } });
  const generatedOrigin = `http://${app.host}:${port}`;
  const set = (deploymentId: string, hostname: string) => owner.call<Domain>("domain_set", { deploymentId, hostname });
  const page = (host: string) => kit.appFetch(host)(`${url}/`, { headers: { accept: "text/html" } });

  // Before anything is claimed, the name answers like any unknown host.
  assert.equal((await page(aliasHost)).status, 404);

  // Names the Kernel refuses before it stores anything.
  const underSuffix = await kit.errorText(set(app.deploymentId, "guestbook.localhost"));
  assert.equal(underSuffix, "guestbook.localhost is under this installation's app suffix localhost, where Konae names apps itself. Name a hostname outside it.");
  const notAName = await kit.errorText(set(app.deploymentId, "https://guestbook.example.org/"));
  assert.match(notAName, /is not a valid DNS name/);
  assert.equal((await owner.call<Domain>("domain_read", { deploymentId: app.deploymentId })).status, "none");
  kit.pass("Names refused", `A name under the suffix answers "${underSuffix}"; a URL answers "${notAName}". Nothing is stored.`);

  // The first call issues the token; the alias does not serve yet.
  const pending = await set(app.deploymentId, "Guestbook.Example.org");
  assert.equal(pending.status, "pending");
  assert.equal(pending.hostname, alias);
  assert.equal(pending.record!.name, record);
  assert.equal(pending.dns, `No TXT record at ${record} yet.`);
  const token = pending.record!.value;
  assert.equal((await page(aliasHost)).status, 404);
  const pendingRuntime = await fetch(`${url}/_konae/mcp`, {
    method: "POST", headers: { host: aliasHost, origin: aliasOrigin, "content-type": "application/json", accept: "application/json, text/event-stream" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "probe", version: "1" } } }),
  });
  assert.equal(pendingRuntime.status, 403, "a pending alias is not a host the runtime knows");

  // A record that holds something else is refused.
  dns.txt(record, ["konae-verify-not-this-one"]);
  const wrong = await kit.errorText(set(app.deploymentId, alias));
  assert.equal(wrong, `The TXT record at ${record} holds "konae-verify-not-this-one", not this Deployment's token. Set it to ${token} and call domain_set again.`);
  assert.equal((await page(aliasHost)).status, 404);

  // The right token makes it serve.
  dns.txt(record, ["v=spf1 -all", token]);
  const active = await set(app.deploymentId, alias);
  assert.equal(active.status, "active");
  assert(dns.asked.filter((name) => name === record).length >= 3, "the Kernel asked the resolver itself");
  kit.pass("TXT proof", `The first domain_set answers pending with ${record} to hold ${token.slice(0, 22)}…; the Kernel asked the fake resolver ${dns.asked.length} times; a record holding another value answers "${wrong.split(",")[0]}, …" and the alias keeps answering 404; the right value makes it active.`);

  // On the alias the app has its own origin.
  const home = await page(aliasHost);
  assert.equal(home.status, 200);
  assert.match(home.headers.get("content-security-policy") ?? "", /connect-src 'self'/);
  assert.equal(home.headers.get("strict-transport-security"), null, "plain HTTP sends no HSTS");
  const initialize = (host: string, origin: string) => fetch(`${url}/_konae/mcp`, {
    method: "POST",
    headers: { host, origin, "content-type": "application/json", accept: "application/json, text/event-stream", "x-forwarded-for": kit.address() },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "probe", version: "1" } } }),
  });
  assert.equal((await initialize(aliasHost, aliasOrigin)).status, 200);
  const crossed = await initialize(aliasHost, generatedOrigin);
  assert.equal(crossed.status, 403);
  const crossedBody = (await crossed.json()).error;
  await kit.runtime(aliasHost).call("actions_invoke", { name: "entries.add", input: { name: "Ines", body: "Signed from the custom domain" } });
  const fromAlias = await (await kit.appFetch(aliasHost)(`${url}/api/entries`)).json();
  const fromGenerated = await (await kit.appFetch(`${app.host}:${port}`)(`${url}/api/entries`)).json();
  assert.equal(fromAlias.result[0].body, "Signed from the custom domain");
  assert.deepEqual(fromAlias, fromGenerated);
  const signup = await kit.appFetch(aliasHost)(`${url}/_konae/auth/signup`, {
    method: "POST", headers: { "content-type": "application/json", origin: aliasOrigin }, body: JSON.stringify({ email: "ines@probe.example", password: crypto.randomUUID() }),
  });
  assert.equal(signup.status, 200, await signup.clone().text());
  const cookie = signup.headers.get("set-cookie") ?? "";
  assert.doesNotMatch(cookie, /domain=/i);
  const session = await kit.runtime(aliasHost, cookie.split(";")[0]).call<{ name?: string; role: string }>("auth_session", {});
  assert.deepEqual([session.name, session.role], ["ines@probe.example", "member"]);
  const admin = await fetch(`${url}/mcp`, { method: "POST", headers: { origin: aliasOrigin, "content-type": "application/json" }, body: "{}" });
  assert.equal(admin.status, 403);
  const adminBody = (await admin.json()).error;
  kit.pass("Serve from the alias", `GET / on Host ${alias} answers 200 with the app policy and the generated hostname still answers. The runtime takes Origin ${aliasOrigin} and refuses Origin ${generatedOrigin} with 403 "${crossedBody}". An entry added on the alias reads the same from /api/entries on both hosts. Signup on the alias sets a host-only cookie (${cookie.split(";").slice(1).map((part) => part.trim()).join("; ")}) and auth_session sees the account there. Administration refuses Origin ${aliasOrigin} with 403 "${adminBody}".`);

  // One name serves one Deployment.
  const other = await kit.publish(here, { projectId: "probe_domain_other", visibility: "public" });
  const taken = await kit.errorText(set(other.deploymentId, alias));
  assert.equal(taken, `${alias} already serves another Deployment on this installation. Clear it there with domain_clear first.`);
  kit.pass("A name another Deployment holds", `domain_set on a second app answers "${taken}"`);

  // A package carries the name without the token; the destination issues its own.
  const exported = await owner.call<{ manifest: any; downloads: Record<string, any> }>("project_export", { deploymentId: app.deploymentId });
  assert.equal(exported.manifest.domain, alias);
  assert(!JSON.stringify(exported.manifest).includes(token), "the token stays behind");
  const bytes: Record<string, Uint8Array> = {};
  for (const content of exported.manifest.contents) bytes[content.name] = await owner.download(exported.downloads[content.name]);
  const importInto = async (projectId: string) => {
    await owner.call("project_create", { projectId, name: projectId, slug: projectId.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" } });
    const components: Record<string, string> = {};
    for (const content of exported.manifest.contents) components[content.name] = (await owner.upload(bytes[content.name], projectId, content.name)).transferId;
    try {
      return await kit.publish(here, { projectId, visibility: "public", initialPackage: { manifest: exported.manifest, components } });
    } finally { for (const id of Object.values(components)) await owner.cancel(id).catch(() => {}); }
  };
  const held = await kit.errorText(importInto("probe_domain_copy"));
  assert.match(held, new RegExp(`Package import failed: ${alias.replaceAll(".", "\\.")} already serves another Deployment`));
  await owner.call("domain_clear", { deploymentId: app.deploymentId });
  assert.equal((await page(aliasHost)).status, 404);
  assert.equal((await page(`${app.host}:${port}`)).status, 200);
  const copy = await importInto("probe_domain_copy2");
  const arrived = await owner.call<Domain>("domain_read", { deploymentId: copy.deploymentId });
  assert.equal(arrived.status, "pending");
  assert.equal(arrived.hostname, alias);
  assert.notEqual(arrived.record!.value, token);
  assert.equal((await page(aliasHost)).status, 404, "an imported name waits for its own token");
  kit.pass("Package", `project_export's manifest names domain ${alias} and holds no token. Importing it while the origin serves the name answers "${held.slice(0, 90)}…". After domain_clear the alias answers 404, the generated hostname 200, and the import arrives with ${alias} pending under a new token.`);

  // A new claim meets the old record: the first answer says so instead of refusing a token nobody could know.
  const reclaimed = await set(app.deploymentId, alias);
  assert.equal(reclaimed.status, "pending");
  assert.match(reclaimed.dns!, /not this token yet\.$/);
  dns.txt(record, [reclaimed.record!.value, arrived.record!.value]);
  assert.equal((await set(app.deploymentId, alias)).status, "active");
  kit.pass("Reclaim", `A new claim on ${alias} meets the old record, says so, and turns active once the record holds its token.`);
}, { env: { KONAE_DNS_RESOLVER: dns.address } });
dns.stop();
