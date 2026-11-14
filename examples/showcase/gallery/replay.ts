// Replays the gallery on its own Kernel: publishes it as a Public Deployment
// with Closed signup, admits one curator by invitation, uploads public photos
// and private drafts, reads them as a visitor, sends contact messages until the
// rate limit answers, reads /feed.json as a program, walks the multi-page
// artifact, writes the Action Contract and runs conform. Exits 0 when every
// assertion holds; removes its Kernel and data either way.
//
//   KONAE_SHOWCASE_PORT=19620 bun run examples/showcase/gallery/replay.ts
//
// KONAE_SHOWCASE_BROWSER=0 skips Chrome. KONAE_SHOWCASE_SCREENSHOTS=<dir>
// writes a light and a dark screenshot of the published home page there.
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { deflateSync } from "node:zlib";
import { chromium } from "playwright";
import { McpClient } from "../../../src/mcp/client";
import { appDir, buildLocal } from "../../../scripts/konae";
import { kernelTarget } from "../../../scripts/conform";
import { generateActionContract } from "../../../src/lib/action-contract";
import { formatConformanceReport, runConformance } from "../../../src/lib/action-conformance";
import { fakeResolver } from "../../probes/_dns";

const here = import.meta.dir;
const repo = path.resolve(here, "../../..");
const scratch = await mkdtemp(path.join(tmpdir(), "konae-showcase-gallery-"));
const port = Number(process.env.KONAE_SHOWCASE_PORT ?? 19620);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set KONAE_SHOWCASE_PORT to a free local port.");
const url = `http://127.0.0.1:${port}`;
const dataDir = path.join(scratch, "data");
const setupKey = crypto.randomUUID();
const ownerPassword = crypto.randomUUID();
const curatorPassword = crypto.randomUUID();
const curatorEmail = "lena@studio.example";

const observations: { flow: string; result: string }[] = [];
const refusals: { attempt: string; status?: number; message: string }[] = [];
const pass = (flow: string, result: string) => { observations.push({ flow, result }); console.log(`PASS ${flow}. ${result}`); };
const refused = (attempt: string, message: string, status?: number) => { refusals.push({ attempt, status, message }); };
let addressCounter = 0;
const freshAddress = () => `198.18.${Math.floor(++addressCounter / 250)}.${(addressCounter % 250) + 1}`;
const errorText = async (promise: Promise<unknown>) => {
  try { await promise; } catch (error) { return error instanceof Error ? error.message : String(error); }
  throw Error("Expected a refusal, but the call succeeded.");
};

// Photographs: 640x480 PNG landscapes drawn here, so the replay carries no binaries.
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = (bytes: Uint8Array) => { let c = 0xffffffff; for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const pngChunk = (type: string, data: Uint8Array) => {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0); out.write(type, 4, "ascii"); Buffer.from(data).copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
};
const palettes: number[][][] = [
  [[38, 58, 112], [242, 172, 128], [28, 38, 48], [255, 224, 176]],
  [[64, 128, 204], [196, 224, 242], [204, 182, 142], [255, 250, 232]],
  [[30, 20, 62], [204, 94, 92], [20, 24, 34], [255, 196, 140]],
  [[176, 182, 188], [222, 224, 226], [108, 120, 114], [240, 240, 236]],
  [[10, 14, 30], [42, 52, 94], [6, 8, 12], [228, 230, 240]],
];
function landscape(seed: number, width = 640, height = 480): Uint8Array {
  const [top, bottom, ground, sun] = palettes[seed % palettes.length];
  const raw = Buffer.alloc((width * 3 + 1) * height);
  const sunX = width * (0.25 + 0.12 * seed), sunY = height * 0.38, sunR = height * 0.07;
  for (let y = 0; y < height; y++) {
    const row = y * (width * 3 + 1);
    for (let x = 0; x < width; x++) {
      const horizon = height * 0.62 + Math.sin((x / width) * Math.PI * (2 + seed) + seed) * height * 0.05;
      let color: number[];
      if (y >= horizon) {
        const depth = (y - horizon) / (height - horizon);
        color = ground.map((c) => c * (1 - 0.35 * depth));
      } else if ((x - sunX) ** 2 + (y - sunY) ** 2 < sunR ** 2) {
        color = sun;
      } else {
        const t = y / horizon;
        color = top.map((c, i) => c + (bottom[i] - c) * t);
      }
      raw[row + 1 + x * 3] = color[0]; raw[row + 2 + x * 3] = color[1]; raw[row + 3 + x * 3] = color[2];
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), pngChunk("IHDR", header), pngChunk("IDAT", deflateSync(raw)), pngChunk("IEND", new Uint8Array())]);
}

// The studio's own domain. A fake resolver answers the Kernel's TXT lookup.
const dns = await fakeResolver();
const studioDomain = "gallery.studio.example";

let kernel: ReturnType<typeof Bun.spawn> | undefined;
try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/konae")], {
    cwd: repo,
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
      KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: dataDir, KONAE_SETUP_KEY: setupKey,
      KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development",
      KONAE_DNS_RESOLVER: dns.address,
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    assert.equal(kernel.exitCode, null, "The Kernel stopped before startup. Is the port taken?");
    try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
    if (!ready) await Bun.sleep(100);
  }
  assert(ready, "The Kernel did not start.");

  // The owner of the installation and of the project.
  const signup = await fetch(`${url}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@studio.example", password: ownerPassword, setupKey }),
  });
  assert.equal(signup.status, 200, await signup.clone().text());
  const ownerCookie = signup.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
    return fetch(input, { ...init, headers });
  } });

  // konae build publishes every page at the Capsule root with the files they load.
  const cliCopy = path.join(scratch, "cli-build");
  await cp(here, appDir(cliCopy), { recursive: true, filter: (source) => !source.endsWith("replay.ts") && !source.endsWith("README.md") });
  const { artifact } = await buildLocal(cliCopy);
  assert.deepEqual(Object.keys(artifact.files).sort(), ["404.html", "about.html", "app.js", "index.html", "runtime.js", "styles.css"]);
  assert(!("schema.sql" in artifact.files) && !("actions.json" in artifact.files));
  pass("CLI build", `konae build writes an artifact with ${Object.keys(artifact.files).sort().join(", ")}: the three pages, the stylesheet and module they load, and the SDK. schema.sql and actions.json stay out.`);
  // A page that names a file the Capsule does not hold stops the build with the path.
  const brokenCopy = path.join(scratch, "cli-broken");
  await cp(cliCopy, brokenCopy, { recursive: true });
  await writeFile(path.join(appDir(brokenCopy), "about.html"), (await readFile(path.join(appDir(brokenCopy), "about.html"), "utf8")).replace("</main>", '<img src="/portrait.jpg" alt=""></main>'));
  const brokenBuild = await errorText(buildLocal(brokenCopy));
  assert.equal(brokenBuild, "about.html references /portrait.jpg, but the Capsule holds no portrait.jpg.");
  refused("`konae build` with about.html naming a missing /portrait.jpg", brokenBuild);

  const source = async (name: string) => readFile(path.join(here, name), "utf8");
  const capsule = {
    "index.html": await source("index.html"), "about.html": await source("about.html"), "404.html": await source("404.html"), "styles.css": await source("styles.css"),
    "app.js": await source("app.js"), "schema.sql": await source("schema.sql"), "actions.json": await source("actions.json"),
  };
  const project = { id: "showcase_gallery", name: "Gallery", slug: "gallery", manifest: { version: 1, visibility: "public" } };
  await owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const release = async (files: Record<string, string>, extra: object = {}) => {
    const bytes = new TextEncoder().encode(JSON.stringify({ project: { ...project, files }, visibility: "public", artifact, ...extra }));
    const upload = await owner.upload(bytes, project.id);
    try {
      const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
      return await owner.call<{ id: string; hostname: string; url: string }>("release_apply", { planId: plan.planId });
    } finally { await owner.cancel(upload.transferId).catch(() => {}); }
  };

  // A mutation with RETURNING is refused when the release is planned.
  const returning = JSON.parse(capsule["actions.json"]);
  returning.actions["contact.send"].statement += " RETURNING id";
  const returningMessage = await errorText(release({ ...capsule, "actions.json": JSON.stringify(returning) }));
  assert.equal(returningMessage, "Action contact.send cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.");
  refused("Publish contact.send with `... RETURNING id`", returningMessage);
  pass("RETURNING refused", returningMessage);

  // First release: public, signup Closed from the first request.
  const receipt = await release(capsule, { initialSignupPolicy: { mode: "closed", allowlist: [] } });
  const host = receipt.hostname;
  const policy = await owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receipt.id });
  assert.equal(policy.mode, "closed");
  pass("Publish", `${receipt.url} is public with signup ${policy.mode}.`);

  const appFetch = (address = freshAddress(), cookie?: string): typeof fetch => (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", host); headers.set("x-forwarded-for", address);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  };
  const runtime = (cookie?: string, address = freshAddress()) => new McpClient(`${url}/_konae/mcp`, { fetch: appFetch(address, cookie) });
  const post = (route: string, body: object, address = freshAddress()) => appFetch(address)(url + route, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  const get = (route: string, init: RequestInit = {}) => appFetch()(url + route, { redirect: "manual", ...init });

  // Closed: a stranger cannot create an account, and neither can the curator yet.
  const stranger = await post("/_konae/auth/signup", { email: "stranger@example.test", password: crypto.randomUUID() });
  const strangerBody = await stranger.json();
  assert.equal(stranger.status, 403);
  refused("A visitor signs up while signup is Closed (POST /_konae/auth/signup)", strangerBody.error, 403);

  // The owner invites the curator. Closed refuses self-signup, not the
  // owner's invitation (ADR-0003).
  const invited = await owner.call<{ policy: string; results: any[] }>("app_invitations_create", {
    deploymentId: receipt.id, emails: [curatorEmail], idempotencyKey: crypto.randomUUID(),
  });
  assert.equal(invited.policy, "closed");
  assert.equal(invited.results[0].status, "invited");
  assert.equal(invited.results[0].acceptable, true);
  let invitationToken = "";
  for (let attempt = 0; attempt < 50 && !invitationToken; attempt++) {
    const mailbox = await readFile(path.join(dataDir, "mailbox.jsonl"), "utf8").catch(() => "");
    invitationToken = mailbox.match(/#invitation=([A-Za-z0-9_-]+)/)?.[1] ?? "";
    if (!invitationToken) await Bun.sleep(100);
  }
  assert(invitationToken, "The invitation never reached mailbox.jsonl.");
  const curatorSignup = await post("/_konae/auth/signup", { email: curatorEmail, password: curatorPassword });
  assert.equal(curatorSignup.status, 403, "an invitation is not a signup");
  const accepted = await post("/_konae/auth/accept", { token: invitationToken, password: curatorPassword });
  assert.equal(accepted.status, 200, await accepted.clone().text());
  const curatorCookie = accepted.headers.get("set-cookie")!.split(";")[0];
  const policyAfter = await owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receipt.id });
  assert.equal(policyAfter.mode, "closed", "the policy never left Closed");
  const strangerAgain = await post("/_konae/auth/signup", { email: "second@example.test", password: crypto.randomUUID() });
  assert.equal(strangerAgain.status, 403);
  const curatorLogin = await post("/_konae/auth/login", { email: curatorEmail, password: curatorPassword });
  assert.equal(curatorLogin.status, 200);
  const users = await owner.call<{ users: any[] }>("app_users_list", { deploymentId: receipt.id });
  assert.deepEqual(users.users.map((user) => user.email), [curatorEmail]);
  pass("Closed signup, one account", `A stranger's signup answers 403 "${strangerBody.error}". The owner's invitation answers acceptable: true under Closed and the curator accepts it with 200, while the curator's own signup answers 403; the policy stays Closed throughout, and the app has exactly one account and still refuses new ones.`);

  // The curator's account becomes the studio. Only the owner can say so, with SQL.
  const curator = runtime(curatorCookie);
  const curatorSession = await curator.call<{ userId: string; role: string }>("auth_session");
  assert.equal(curatorSession.role, "member");
  // Before the owner names the curator in the studio table, photos.add's guard refuses her.
  const beforeStudio = await errorText(curator.call<any>("actions_invoke", { name: "photos.add", input: { file_id: "none", title: "Before the studio row", caption: null, status: "draft" } }));
  assert.equal(beforeStudio, "Only the studio's curator can add photographs.");
  await owner.call("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: `INSERT INTO studio (user_id) VALUES ('${curatorSession.userId.replaceAll("'", "''")}')` } });

  const upload = async (bytes: Uint8Array, name: string, visibility?: "public") => {
    const transfer = await curator.upload(bytes);
    try { return await curator.call<{ id: string; visibility: string }>("files_upload", { transferId: transfer.transferId, name, type: "image/png", ...(visibility ? { visibility } : {}) }); }
    finally { await curator.cancel(transfer.transferId).catch(() => {}); }
  };
  const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
  const photos = [
    { title: "Low tide, Vlissingen", caption: "The harbour wall at six in the morning." },
    { title: "Depot 4", caption: "Night trams waiting for the first shift." },
    { title: "Before harvest", caption: null },
    { title: "Fog over the dyke", caption: "Unpublished: still choosing the crop." },
    { title: "Moon over the yard", caption: null },
  ];
  const images = photos.map((_, index) => landscape(index));
  const stored: { id: number; fileId: string; status: string }[] = [];
  for (const [index, photo] of photos.entries()) {
    const draft = index >= 3;
    const file = await upload(images[index], `photo-${index + 1}.png`, draft ? undefined : "public");
    assert.equal(file.visibility, draft ? "private" : "public");
    const added = await action(curator, "photos.add", { file_id: file.id, title: photo.title, caption: photo.caption, status: draft ? "draft" : "published" });
    assert.equal(added.changes, 1);
    stored.push({ id: added.lastInsertId, fileId: file.id, status: draft ? "draft" : "published" });
  }
  const drafts = stored.filter((photo) => photo.status === "draft");
  assert.equal((await action(curator, "studio.photos")).length, 5);
  pass("Curator uploads", "The curator stores three public photographs and two private drafts; photos.add was refused with its guard before the owner added the account to the studio table.");

  // A visitor sees published photographs only, by every door.
  const visitor = runtime();
  const published = await action(visitor, "photos.published");
  assert.deepEqual(published.map((row: any) => row.title).sort(), photos.slice(0, 3).map((p) => p.title).sort());
  const visibleFiles = await visitor.call<{ id: string; visibility: string }[]>("files_list");
  assert.deepEqual(visibleFiles.map((file) => file.id).sort(), stored.filter((p) => p.status === "published").map((p) => p.fileId).sort());
  const draftRead = await errorText(visitor.call("files_read", { fileId: drafts[0].fileId }));
  assert.equal(draftRead, "File not found.");
  refused("A visitor reads a draft over MCP (files_read)", draftRead);
  const draftHttp = await get(`/_konae/files/${drafts[0].fileId}`);
  assert.equal(draftHttp.status, 404);
  refused("A visitor reads a draft at GET /_konae/files/<id>", (await draftHttp.text()).trim() || "(empty body)", 404);
  const publicHttp = await get(`/_konae/files/${stored[0].fileId}`);
  assert.equal(publicHttp.status, 200);
  assert.equal(publicHttp.headers.get("content-type"), "image/png");
  assert.match(publicHttp.headers.get("content-disposition") ?? "", /^inline/);
  assert.equal(publicHttp.headers.get("cross-origin-resource-policy"), "same-origin");
  assert.deepEqual(new Uint8Array(await publicHttp.arrayBuffer()), images[0]);
  const studioAsVisitor = await errorText(action(visitor, "studio.photos"));
  assert.equal(studioAsVisitor, "authentication-required");
  const addAsVisitor = await errorText(action(visitor, "photos.add", { file_id: "x", title: "x", caption: null, status: "published" }));
  assert.equal(addAsVisitor, "authentication-required");
  const visitorUpload = await errorText(visitor.upload(images[0]));
  refused("A visitor uploads a file (transfer_begin)", visitorUpload);
  pass("Visitor view", `photos.published returns 3 rows, files_list lists the 3 public files, a draft answers "${draftRead}" over MCP and 404 at the file path, a public file answers 200 image/png inline with CORP same-origin, and studio actions answer "${studioAsVisitor}".`);

  // Publishing a draft makes its own file public; nothing is uploaded twice.
  const madePublic = await curator.call<{ id: string; visibility: string; url?: string }>("files_update", { fileId: drafts[0].fileId, visibility: "public" });
  assert.equal(madePublic.id, drafts[0].fileId);
  assert.equal(madePublic.visibility, "public");
  assert.equal((await action(curator, "photos.publish", { id: drafts[0].id })).changes, 1);
  // The row keeps its own file, so publish takes only the photograph's ID, and a second publish is refused.
  assert.equal(await errorText(action(curator, "photos.publish", { id: drafts[0].id })), "Only the studio's curator can publish, and only a photograph that is still a draft.");
  assert.equal(await errorText(action(curator, "photos.publish", { id: drafts[0].id, file_id: stored[0].fileId })), "Unexpected action input: file_id");
  assert.equal((await action(visitor, "photos.published")).length, 4);
  const publishedDraft = await get(`/_konae/files/${drafts[0].fileId}`);
  assert.equal(publishedDraft.status, 200);
  assert.deepEqual(new Uint8Array(await publishedDraft.arrayBuffer()), images[3]);
  assert.equal((await curator.call<any[]>("files_list")).length, 5);
  const visitorUpdate = await errorText(visitor.call("files_update", { fileId: drafts[1].fileId, visibility: "public" }));
  assert.equal(visitorUpdate, "authentication-required");
  refused("A visitor makes a draft public (files_update)", visitorUpdate);
  const unknownVisibility = await errorText(curator.call("files_update", { fileId: drafts[1].fileId, visibility: "friends" }));
  assert.equal(unknownVisibility, "Value is outside the allowed choices.");
  refused("files_update with visibility \"friends\"", unknownVisibility);
  // A wrong upload is deleted, and its bytes stop counting against the quota.
  const stray = await upload(images[4], "wrong-crop.png");
  assert.equal((await curator.call<any[]>("files_list")).length, 6);
  assert.deepEqual(await curator.call("files_delete", { fileId: stray.id }), { id: stray.id, deleted: true });
  assert.equal((await curator.call<any[]>("files_list")).length, 5);
  const deletedRead = await errorText(curator.call("files_read", { fileId: stray.id }));
  assert.equal(deletedRead, "File not found.");
  const deletedHttp = await appFetch(freshAddress(), curatorCookie)(`${url}/_konae/files/${stray.id}`);
  assert.equal(deletedHttp.status, 404);
  const deletedAgain = await errorText(curator.call("files_delete", { fileId: stray.id }));
  assert.equal(deletedAgain, "File not found.");
  refused("Delete a file that was already deleted (files_delete)", deletedAgain);
  pass("Publish a draft", `files_update makes the draft's own file public under the same ID and photos.publish marks the row published, a second publish is refused with its guard, so the curator holds 5 files for 5 photographs. A stray upload is deleted with files_delete: files_list drops it, files_read answers "${deletedRead}", the file path answers 404, and its size no longer counts against the 20 MiB quota.`);

  // /feed.json: a public GET route any program reads without a signature or a key.
  const feed = await fetch(`${url}/feed.json`, { headers: { host, "x-forwarded-for": freshAddress(), origin: "https://reader.example" } });
  assert.equal(feed.status, 200);
  assert.equal(feed.headers.get("content-type"), "application/json");
  assert.equal(feed.headers.get("cache-control"), "no-store");
  // An open GET route allows every origin since ADR-0013.
  assert.equal(feed.headers.get("access-control-allow-origin"), "*");
  const feedBody = await feed.json();
  assert.deepEqual(Object.keys(feedBody), ["result"]);
  assert.equal(feedBody.result.length, 4);
  assert.equal(feedBody.result[0].title, "Fog over the dyke");
  assert.match(feedBody.result[0].image, /^\/_konae\/files\/[^/]+$/);
  assert.equal((await get(feedBody.result[0].image)).status, 200);
  const head = await get("/feed.json", { method: "HEAD" });
  assert.equal(head.status, 405);
  assert.equal(head.headers.get("allow"), "GET");
  // HTTP strips the body of a HEAD answer, so the documented message never reaches the caller.
  const headBody = await head.text();
  refused("HEAD /feed.json", `405 with Allow: GET and ${headBody ? `body ${headBody}` : "an empty body"}`, 405);
  const postFeed = await get("/feed.json", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
  assert.equal(postFeed.status, 405);
  assert.equal(postFeed.headers.get("allow"), "GET");
  const postFeedBody = await postFeed.json();
  assert.equal(postFeedBody.error, "No route accepts POST on this path.");
  refused("POST /feed.json", postFeedBody.error, 405);
  const feedQuery = await get("/feed.json?page=2");
  const feedQueryBody = await feedQuery.json();
  assert.equal(feedQuery.status, 400);
  refused("GET /feed.json?page=2 (an undeclared query parameter)", feedQueryBody.error, 400);
  refused("Shape /feed.json as a JSON Feed document (version, title, items)", "No primitive: a route always answers { \"result\": [rows] }.");
  refused("Absolute image URLs in /feed.json", "No primitive: the only kernel binding is now; the statement cannot read the Deployment's origin.");
  refused("Let a browser cache /feed.json", "No primitive: Cache-Control: no-store on every answer. A page on another origin reads it, since the route answers Access-Control-Allow-Origin: *.");
  pass("/feed.json", `GET with no signature or key answers 200 application/json { result: [4 items] } newest first, with image paths that load; HEAD answers 405 Allow: GET with ${headBody ? "a body" : "no body"}; POST answers 405 Allow: GET "${postFeedBody.error}"; ?page=2 answers 400 "${feedQueryBody.error}".`);

  // contact.send: anonymous, records a Signal, and trips the per-address limit.
  const message = (n: number) => ({ name: `Reader ${n}`, email: `reader${n}@example.test`, topic: n % 2 ? "print" : "commission", message: `Is print ${n} still available?` });
  let lastResponse: Response | undefined;
  const limitedAddress = freshAddress();
  const sameAddress = new McpClient(`${url}/_konae/mcp`, { fetch: async (input, init) => (lastResponse = await appFetch(limitedAddress)(input, init)) });
  let sent = 0, limitedAt = 0, retryAfter = "";
  for (let n = 1; n <= 40 && !limitedAt; n++) {
    try { assert.equal((await action(sameAddress, "contact.send", message(n))).changes, 1); sent++; }
    catch (error) {
      assert.equal((error as Error).message, "rate-limited");
      limitedAt = n; retryAfter = lastResponse!.headers.get("retry-after") ?? "";
      assert.equal(lastResponse!.status, 200);
    }
  }
  assert(limitedAt > 0, "40 messages from one address were never rate-limited.");
  assert.match(retryAfter, /^\d+$/);
  assert(Number(retryAfter) >= 1);
  refused(`Message ${limitedAt} from one address within 10 seconds (actions_invoke contact.send)`, `rate-limited, HTTP 200 with isError and Retry-After: ${retryAfter}`);
  const otherAddress = runtime();
  assert.equal((await action(otherAddress, "contact.send", message(99))).changes, 1);
  const badTopic = await errorText(action(runtime(), "contact.send", { ...message(100), topic: "spam" }));
  assert.match(badTopic, /CHECK constraint failed/);
  refused("contact.send with topic \"spam\"", badTopic);
  refused("A tighter limit for contact.send alone, such as 3 an hour", "No primitive: actions.json declares no rate limit; every anonymous action shares 30 calls per 10 seconds per address.");
  const signals = await owner.call<{ signals: { name: string; properties: Record<string, unknown> }[] }>("signals_read", { deploymentId: receipt.id, name: "contact.received" });
  assert.equal(signals.signals.length, sent + 1);
  assert.deepEqual(Object.keys(signals.signals[0].properties), ["topic"]);
  const inbox = await action(curator, "studio.messages");
  assert.equal(inbox.length, sent + 1);
  pass("contact.send", `One address sends ${sent} messages, call ${limitedAt} answers "rate-limited" (HTTP 200, isError, Retry-After: ${retryAfter}), another address sends at once; ${signals.signals.length} contact.received Signals carry only the topic, and the curator's studio.messages lists ${inbox.length}. A topic outside the CHECK answers SQLite's text: "${badTopic}".`);

  // The multi-page artifact and unknown paths.
  const title = (html: string) => html.match(/<title>([^<]*)<\/title>/)?.[1] ?? "(no title)";
  const paths = ["/", "/index.html", "/about.html", "/about", "/about/", "/missing-page", "/photos/2026/fog.jpg", "/styles.css", "/feed"];
  const walk: { path: string; status: number; type: string; title: string }[] = [];
  for (const route of paths) {
    const response = await get(route);
    const text = await response.text();
    walk.push({ path: route, status: response.status, type: response.headers.get("content-type") ?? "", title: /html/.test(response.headers.get("content-type") ?? "") ? title(text) : "" });
  }
  console.table(walk);
  const at = (route: string) => walk.find((entry) => entry.path === route)!;
  assert.equal(at("/about.html").title, "About · Lena Varga");
  assert.equal(at("/").title, "Lena Varga · Photographs");
  assert.equal(at("/about").status, 200);
  assert.equal(at("/about").title, "About · Lena Varga");
  for (const missing of ["/about/", "/missing-page", "/photos/2026/fog.jpg", "/feed"]) {
    assert.equal(at(missing).status, 404, missing);
    assert.equal(at(missing).title, "Not found · Lena Varga", missing);
  }
  assert.equal(at("/styles.css").type, "text/css; charset=utf-8");
  const headStyles = await get("/styles.css", { method: "HEAD" });
  assert.equal(headStyles.status, 200);
  assert.equal(headStyles.headers.get("content-type"), "text/css; charset=utf-8");
  pass("Multi-page artifact", `about.html serves its own page at /about.html and /about; /missing-page, /about/, /photos/2026/fog.jpg and /feed answer 404 with 404.html ("${at("/missing-page").title}"), never the home page. HEAD /styles.css answers ${headStyles.status} text/css.`);

  // Usage counts the route under its method and path.
  const today = new Date().toISOString().slice(0, 10);
  const usage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: receipt.id, from: today, to: today });
  const routeCount = usage.totals.find((row) => row.kind === "route" && row.name === "GET /feed.json")?.count ?? 0;
  assert.equal(routeCount, 1);
  pass("Usage", `usage_read counts GET /feed.json ${routeCount} time (the 400 and 405 answers count nothing) and contact.send ${usage.totals.find((row) => row.kind === "action" && row.name === "contact.send")?.count} times.`);

  // The studio's own domain: proved with a TXT record, then the same app on its own origin.
  const aliasHost = `${studioDomain}:${port}`;
  const aliasOrigin = `http://${aliasHost}`;
  const onAlias = (path: string, init: RequestInit = {}) => {
    const headers = new Headers(init.headers); headers.set("host", aliasHost); headers.set("x-forwarded-for", freshAddress());
    return fetch(url + path, { ...init, headers });
  };
  const claim = await owner.call<{ status: string; record: { name: string; value: string } }>("domain_set", { deploymentId: receipt.id, hostname: studioDomain });
  assert.equal(claim.status, "pending");
  assert.equal((await onAlias("/")).status, 404);
  dns.txt(claim.record.name, ["konae-verify-copied-from-another-app"]);
  const wrongRecord = await errorText(owner.call("domain_set", { deploymentId: receipt.id, hostname: studioDomain }));
  assert.equal(wrongRecord, `The TXT record at ${claim.record.name} holds "konae-verify-copied-from-another-app", not this Deployment's token. Set it to ${claim.record.value} and call domain_set again.`);
  refused(`domain_set ${studioDomain} while ${claim.record.name} holds another value`, wrongRecord);
  const underSuffix = await errorText(owner.call("domain_set", { deploymentId: receipt.id, hostname: "gallery.localhost" }));
  assert.equal(underSuffix, "gallery.localhost is under this installation's app suffix localhost, where Konae names apps itself. Move the app's generated hostname there with hostname_set, or name a hostname outside the suffix.");
  refused("domain_set gallery.localhost", underSuffix);
  dns.txt(claim.record.name, [claim.record.value]);
  const proved = await owner.call<{ status: string; url: string }>("domain_set", { deploymentId: receipt.id, hostname: studioDomain });
  assert.equal(proved.status, "active");
  const aliasHome = await onAlias("/", { headers: { accept: "text/html" } });
  assert.equal(aliasHome.status, 200);
  assert.match(await aliasHome.text(), /<html/i);
  assert.match(aliasHome.headers.get("content-security-policy") ?? "", /img-src 'self' blob: data:/);
  assert.equal((await get("/")).status, 200, "the generated hostname keeps answering");
  const aliasAbout = await onAlias("/about");
  assert.equal(aliasAbout.status, 200);
  const aliasMissing = await onAlias("/missing-page");
  assert.equal(aliasMissing.status, 404);
  const aliasFeed = await (await onAlias("/feed.json")).json();
  assert.deepEqual(aliasFeed, feedBody);
  const aliasImage = await onAlias(aliasFeed.result[0].image);
  assert.equal(aliasImage.status, 200);
  assert.equal(aliasImage.headers.get("content-type"), "image/png");
  const aliasRuntime = new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", aliasHost); headers.set("origin", aliasOrigin); headers.set("x-forwarded-for", freshAddress());
    return fetch(input, { ...init, headers });
  } });
  const aliasPhotos = await aliasRuntime.call<any[]>("actions_invoke", { name: "photos.published", input: {} });
  assert.equal(aliasPhotos.length, 4);
  const crossOrigin = await errorText(new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", aliasHost); headers.set("origin", `http://${host}:${port}`);
    return fetch(input, { ...init, headers });
  } }).call("actions_invoke", { name: "photos.published", input: {} }));
  assert.match(crossOrigin, /Cross-origin or unrecognized-host MCP request rejected/);
  refused(`A page on ${host} calls the runtime on ${studioDomain}`, crossOrigin, 403);
  pass("Custom domain", `domain_set ${studioDomain} answers pending with ${claim.record.name}; a record holding another value is refused and the alias answers 404; once the fake resolver returns the token the alias serves: / answers 200 with the app policy, /about 200, /missing-page 404 with 404.html, /feed.json the same 4 items, the first image 200 image/png from /_konae/files/<id>, and photos.published over the runtime with Origin ${aliasOrigin} returns 4 rows. The generated hostname keeps answering, and a page on it cannot call the runtime on the alias.`);

  // The published pages in Chrome, under the Kernel's Content Security Policy.
  if (process.env.KONAE_SHOWCASE_BROWSER !== "0") {
    const browser = await chromium.launch({ channel: "chrome" });
    const shots = process.env.KONAE_SHOWCASE_SCREENSHOTS;
    if (shots) await mkdir(shots, { recursive: true });
    const open = async (route: string, options: { colorScheme?: "light" | "dark"; cookie?: string } = {}) => {
      const context = await browser.newContext({ colorScheme: options.colorScheme ?? "light", viewport: { width: 1280, height: 900 }, extraHTTPHeaders: { "x-forwarded-for": freshAddress() } });
      if (options.cookie) {
        const [name, value] = [options.cookie.slice(0, options.cookie.indexOf("=")), options.cookie.slice(options.cookie.indexOf("=") + 1)];
        await context.addCookies([{ name, value, domain: host.split(":")[0], path: "/" }]);
      }
      const page = await context.newPage();
      const violations: string[] = [];
      page.on("console", (entry) => { if (/Content Security Policy|Refused to/i.test(entry.text())) violations.push(entry.text()); });
      page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
      await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) => console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
      const response = await page.goto(new URL(route, receipt.url).href, { waitUntil: "load" });
      assert.equal(response?.status(), 200);
      assert.match(response?.headers()["content-security-policy"] ?? "", /img-src 'self' blob: data:/);
      await page.waitForFunction(() => typeof (window as any).miniapp === "object", undefined, { timeout: 10_000 });
      return { context, page, violations };
    };
    try {
      for (const scheme of ["light", "dark"] as const) {
        const { context, page, violations } = await open("/", { colorScheme: scheme });
        await page.waitForFunction(() => { const imgs = [...document.querySelectorAll<HTMLImageElement>("#grid img")]; return imgs.length === 4 && imgs.every((img) => img.complete && img.naturalWidth === 640); }, undefined, { timeout: 10_000 });
        if (shots) await page.screenshot({ path: path.join(shots, `gallery-${scheme}.png`), fullPage: true });
        assert.deepEqual(violations, [], `/ in ${scheme} broke under the Content Security Policy.`);
        await context.close();
      }
      const about = await open("/about.html");
      await about.page.getByLabel("Name").fill("Browser reader");
      await about.page.getByLabel("Email").fill("browser@example.test");
      await about.page.getByLabel("About").selectOption("print");
      await about.page.getByLabel("Message").fill("Do you ship prints abroad?");
      await about.page.getByRole("button", { name: "Send message" }).click();
      await about.page.locator("#contact-status.ok").waitFor({ timeout: 10_000 });
      if (shots) await about.page.screenshot({ path: path.join(shots, "gallery-about-light.png"), fullPage: true });
      assert.deepEqual(about.violations, [], "/about.html broke under the Content Security Policy.");
      await about.context.close();
      const studio = await open("/", { cookie: curatorCookie });
      await studio.page.locator("#studio:not([hidden]) .draft img").waitFor({ timeout: 10_000 });
      assert.equal(await studio.page.locator("#drafts .draft").count(), 1);
      if (shots) await studio.page.screenshot({ path: path.join(shots, "gallery-studio-light.png"), fullPage: true });
      assert.deepEqual(studio.violations, [], "The studio view broke under the Content Security Policy.");
      // The page re-reads a draft's bytes from files.url, publishes and hides it
      // through the SDK, and the SDK refuses a visibility the Kernel lacks.
      const sdk = await studio.page.evaluate(async (draftId) => {
        const miniapp = (window as any).miniapp;
        const url = await miniapp.files.url(draftId);
        const response = await fetch(url);
        const bytes = (await response.arrayBuffer()).byteLength;
        const published = (await miniapp.files.update(draftId, { visibility: "public" })).visibility;
        const hidden = (await miniapp.files.update(draftId, { visibility: "private" })).visibility;
        let refusal = "stored";
        try { await miniapp.files.upload(new Blob([new Uint8Array([1])], { type: "image/png" }), { visibility: "organizers" }); } catch (error) { refusal = (error as Error).message; }
        return { url, status: response.status, type: response.headers.get("content-type"), bytes, published, hidden, refusal, files: (await miniapp.files.list()).length };
      }, drafts[1].fileId);
      assert.deepEqual(sdk, { url: `/_konae/files/${drafts[1].fileId}`, status: 200, type: "image/png", bytes: images[4].byteLength, published: "public", hidden: "private", refusal: "File visibility must be private or public.", files: 5 });
      refused("miniapp.files.upload(file, { visibility: \"organizers\" }) in the page", sdk.refusal);
      await studio.page.waitForTimeout(300);
      assert.deepEqual(studio.violations, [], "Reading a file through files.url broke under the Content Security Policy.");
      // The owner takes the account out of the studio table while the studio page is open.
      // A refused upload leaves no file, and a refused publish puts the draft's file back to private.
      await owner.call("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: "DELETE FROM studio" } });
      await studio.page.locator("#upload input[name=file]").setInputFiles({ name: "late.png", mimeType: "image/png", buffer: Buffer.from(images[0]) });
      await studio.page.getByLabel("Title").fill("After the studio row");
      await studio.page.getByRole("button", { name: "Add photograph" }).click();
      await studio.page.locator("#upload-status").filter({ hasText: "Only the studio's curator can add photographs." }).waitFor({ timeout: 10_000 });
      assert.equal(await studio.page.evaluate(async () => (await (window as any).miniapp.files.list()).length), 5, "A refused photograph leaves no file.");
      await studio.page.locator("#drafts .draft").getByRole("button", { name: "Publish" }).click();
      await studio.page.locator("#drafts .draft").filter({ hasText: "Only the studio's curator can publish" }).waitFor({ timeout: 10_000 });
      assert.equal((await get(`/_konae/files/${drafts[1].fileId}`)).status, 404, "A refused publish leaves the draft's file private.");
      await owner.call("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: `INSERT INTO studio (user_id) VALUES ('${curatorSession.userId.replaceAll("'", "''")}')` } });
      assert.deepEqual(studio.violations, [], "The refused upload and publish broke under the Content Security Policy.");
      await studio.context.close();
      pass("Chrome", `The home page in light and dark, about.html with a sent contact form, and the signed-in studio load with no policy violation; with the account out of the studio table, a refused upload deletes its file and a refused publish leaves the draft private; every grid image loads from /_konae/files/<id> at 640px. In the studio, fetch(await miniapp.files.url(draft)) answers ${sdk.status} ${sdk.type} with ${sdk.bytes} bytes under connect-src 'self', files.update publishes and hides the draft, and files.upload refuses visibility "organizers" before sending anything.`);
    } finally { await browser.close(); }
  }

  // The Action Contract, written by the CLI, and conform with no credential.
  const contractDir = path.join(scratch, "contract");
  const cli = (args: string[]) => Bun.spawn(["bun", "run", path.join(repo, "scripts/konae.ts"), ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
  const written = cli(["contract", "--dir", cliCopy, "--out", contractDir]);
  const [writtenCode, writtenOut, writtenErr] = await Promise.all([written.exited, new Response(written.stdout).text(), new Response(written.stderr).text()]);
  assert.equal(writtenCode, 0, writtenErr);
  if (process.env.KONAE_SHOWCASE_CONTRACT_OUT) await writeFile(process.env.KONAE_SHOWCASE_CONTRACT_OUT, await readFile(path.join(contractDir, "action-contract.md"), "utf8"));
  const inputsFile = path.join(scratch, "conform-inputs.json");
  await writeFile(inputsFile, JSON.stringify({ "contact.send": { topic: "other" } }));
  const conformCli = cli(["conform", "--url", receipt.url, "--contract", path.join(contractDir, "action-contract.json"), "--inputs", inputsFile]);
  const [cliCode, cliOut, cliErr] = await Promise.all([conformCli.exited, new Response(conformCli.stdout).text(), new Response(conformCli.stderr).text()]);
  console.log(cliOut);
  assert.equal(cliCode, 0, cliOut + cliErr);
  const cliTotals = cliOut.trim().split("\n").at(-1)!;
  const skippedLines = cliOut.split("\n").filter((line) => line.startsWith("SKIP"));
  const { contract } = generateActionContract({ actionsSource: capsule["actions.json"], schemaSource: capsule["schema.sql"] });
  const report = await runConformance(contract, kernelTarget({ url: receipt.url, fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("x-forwarded-for", freshAddress());
    return fetch(input, { ...init, headers });
  } }), { inputs: { "contact.send": { topic: "other" } } });
  assert.equal(report.failed, 0, formatConformanceReport(report));
  pass("Action Contract", `${writtenOut.trim()} conform with no credential: "${cliTotals}". Skipped: ${skippedLines.map((line) => line.replace(/^SKIP\s+/, "")).join(" | ") || "none"}. The same run in process, one address per call: ${report.passed} passed, ${report.failed} failed, ${report.skipped} skipped.`);

  console.log(JSON.stringify({ app: "gallery", url: receipt.url, observations, refusals }, null, 2));
} finally {
  kernel?.kill();
  if (kernel) await kernel.exited;
  dns.stop();
  await rm(scratch, { recursive: true, force: true });
}
