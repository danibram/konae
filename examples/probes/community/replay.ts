import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

const here = import.meta.dir;

await probe("community", 23030, async (kit) => {
  const { owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_community", visibility: "public", signup: { mode: "open" } });
  const people = { edith: "edith@probe.example", ana: "ana@probe.example", rui: "rui@probe.example" };
  const cookies = Object.fromEntries(await Promise.all(Object.entries(people).map(async ([name, email]) => [name, await kit.signup(app.host, email)])));
  const as = (name: keyof typeof people) => kit.runtime(app.host, cookies[name]);
  const act = <T = any>(name: keyof typeof people, action: string, input: object = {}) => as(name).call<T>("actions_invoke", { name: action, input });
  const ids: Record<string, string> = {};
  for (const name of Object.keys(people) as (keyof typeof people)[]) {
    ids[name] = (await as(name).call<{ userId: string }>("auth_session")).userId;
    await act(name, "members.join", { name: name[0].toUpperCase() + name.slice(1) });
  }

  const early = await kit.errorText(act("ana", "posts.create", { title: "Hello", status: "published" }));
  assert.equal(early, "Only a publisher can post.");
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: `UPDATE members SET role = 'editor' WHERE user_id = '${ids.edith}'` } });
  const listed = await act<any[]>("edith", "members.list");
  assert.deepEqual(listed.map((row) => row.display_name), ["Ana", "Edith", "Rui"]);
  const byReader = await kit.errorText(act("rui", "members.promote", { member: ids.ana }));
  assert.equal(byReader, "Only an editor can make someone a publisher.");
  assert.equal((await act("rui", "members.list")).length, 0);
  assert.equal((await act("edith", "members.promote", { member: ids.ana })).changes, 1);
  kit.pass("Roles inside the app", `A reader's post is refused with "${early}". The owner makes the first editor with database_execute; after that the editor lists members by the display names they gave and promotes Ana, while Rui's promotion is refused with "${byReader}" and his members.list returns no rows.`);

  const actionsJson = JSON.parse(await readFile(path.join(here, "actions.json"), "utf8"));
  actionsJson.actions["members.join"].bindings[1] = { source: "actor", field: "email" };
  const variant = { "schema.sql": await readFile(path.join(here, "schema.sql"), "utf8"), "actions.json": JSON.stringify(actionsJson) };
  const emailGate = await kit.gateRefusal(variant);
  const emailKernel = await kit.planRefusal(here, "probe_community_email", variant);
  assert.equal(emailGate.detail ?? emailGate.message, "Invalid binding 1 in members.join.");
  assert.equal(emailKernel, emailGate.detail ?? emailGate.message);
  console.log(`  actor.email: both doors refuse with "${emailKernel}"`);
  kit.pass("No email from the actor", `A binding { source: "actor", field: "email" } is refused at both doors with "${emailKernel}", so it never publishes. The app knows a person by ID and by what they typed.`);

  assert.equal((await act("ana", "posts.create", { title: "Draft about rivers", status: "draft" })).changes, 1);
  assert.equal((await act("ana", "posts.create", { title: "Letter one", status: "published" })).changes, 1);
  const visitor = kit.runtime(app.host);
  const feed = await visitor.call<any[]>("actions_invoke", { name: "posts.feed", input: {} });
  assert.deepEqual(feed.map((row) => [row.title, row.author]), [["Letter one", "Ana"]]);
  assert.deepEqual((await act<any[]>("ana", "posts.mine")).map((row) => row.status), ["draft", "published"]);
  assert.equal((await act("rui", "follows.add", { author: ids.ana })).changes, 1);
  assert.equal((await act("rui", "follows.add", { author: ids.rui })).changes, 0);
  assert.deepEqual((await act<any[]>("rui", "posts.following")).map((row) => row.title), ["Letter one"]);
  kit.pass("Publishers and readers", "A visitor's posts.feed shows Letter one and not Ana's draft; Ana sees both; Rui follows Ana (following a reader changes nothing) and his feed holds her published letter.");

  const last = feed.at(-1).id;
  assert.deepEqual(await visitor.call("actions_invoke", { name: "posts.feed", input: { after: last } }), []);
  await act("ana", "posts.create", { title: "Letter two", status: "published" });
  const next = await visitor.call<any[]>("actions_invoke", { name: "posts.feed", input: { after: last } });
  assert.deepEqual(next.map((row) => row.title), ["Letter two"]);
  kit.pass("Realtime by polling", "posts.feed with after=<last id> returns [] until Ana publishes, then only Letter two. Nothing is pushed: the page asks again, within 30 calls per 10 seconds per visitor address and 120 per signed-in user.");

  const signals = await owner.call<{ signals: any[] }>("signals_read", { deploymentId: app.deploymentId });
  const created = signals.signals.filter((signal) => signal.name === "post.created");
  assert.equal(created.length, 3);
  assert(created.every((signal) => signal.properties.actor === ids.ana));
  const promoted = signals.signals.find((signal) => signal.name === "member.promoted");
  assert.deepEqual(promoted.properties, { member: ids.ana, actor: ids.edith });
  kit.pass("Audit", "signals_read gives the owner 3 post.created naming Ana and member.promoted naming Edith as actor and Ana as member; the two refused calls left none. Members see only what the rows keep (author_id), because one statement writes one table and Signals are for the owner.");

  const ana = as("ana");
  const transfer = await ana.upload(new TextEncoder().encode("manuscript"));
  const file = await ana.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "draft.txt", type: "text/plain" });
  await ana.cancel(transfer.transferId).catch(() => {});
  const editorRead = await kit.errorText(as("edith").call("files_read", { fileId: file.id }));
  assert.equal(editorRead, "File not found.");
  kit.pass("Files by role", `Ana's private file answers "${editorRead}" to the editor: a file is the uploader's, or public, and no rule shares it with a role.`);
});
