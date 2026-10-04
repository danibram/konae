// A hostile app user and a hostile app against the showcase apps: other
// users' rows, a forged actor, sessions carried to another Deployment or to
// the editor, and Host header games. Each test is one attempt from the
// security review; a refusal that already held says why in a comment.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { answerText, appUser, hostFetch, rawCall, runtimeClient, startLab, testPort, type Lab } from "./security-review-kernel";

let lab: Lab;
beforeAll(async () => { lab = await startLab(testPort("LITTLEBOX_REVIEW_SESSIONS_PORT", 23600)); }, 180_000);
afterAll(async () => { await lab?.stop(); });

const invoke = (client: ReturnType<typeof runtimeClient>, name: string, input: object = {}) => client.call<any>("actions_invoke", { name, input });

describe("cross-user", () => {
  test("a user cannot read, archive or check in on another user's habits through actions_invoke", async () => {
    const { habits } = lab.apps;
    const ana = runtimeClient(lab, habits.hostname, (await appUser(lab, habits)).cookie);
    const ben = runtimeClient(lab, habits.hostname, (await appUser(lab, habits)).cookie);
    const created = await invoke(ana, "habits.create", { name: "Read" });
    expect(await invoke(ben, "habits.list")).toEqual([]);
    // The statement binds the actor, so Ben's update of Ana's row changes nothing.
    await expect(invoke(ben, "habits.archive", { id: created.lastInsertId })).rejects.toThrow("This habit is already archived, or it is not yours.");
    const day = new Date().toISOString().slice(0, 10);
    expect((await invoke(ben, "checkins.add", { habitId: created.lastInsertId, day, device: "phone" })).changes).toBe(0);
    expect((await invoke(ana, "habits.list")).map((row: { name: string }) => row.name)).toEqual(["Read"]);
  });

  test("an input that names the actor is refused before any SQL runs", async () => {
    const { habits } = lab.apps;
    const ben = runtimeClient(lab, habits.hostname, (await appUser(lab, habits)).cookie);
    // The actor binding comes from the session; an input key with its name is an undeclared input.
    for (const key of ["actor", "userId", "user_id", "id"]) {
      await expect(invoke(ben, "habits.list", { [key]: "someone-else" })).rejects.toThrow(`Unexpected action input: ${key}`);
    }
  });

  test("an API key on a session route acts as itself and never as a user", async () => {
    const { habits } = lab.apps;
    const ana = await appUser(lab, habits);
    await invoke(runtimeClient(lab, habits.hostname, ana.cookie), "habits.create", { name: "Private habit" });
    const key = await lab.kernel.owner.call<{ key: string }>("api_keys_create", { deploymentId: habits.id, name: "Reader", actions: ["habits.mine"] });
    const read = await hostFetch(lab, habits.hostname, "/api/habits", { headers: { authorization: `Bearer ${key.key}` } });
    expect(read.status).toBe(200);
    // The key is its own Runtime Actor, so user_id = ?1 matches no user's row.
    expect(JSON.stringify(await read.json())).not.toContain("Private habit");
    // A key plus a user's cookie is still the key.
    const both = await hostFetch(lab, habits.hostname, "/api/habits", { headers: { authorization: `Bearer ${key.key}`, cookie: ana.cookie } });
    expect(JSON.stringify(await both.json())).not.toContain("Private habit");
  });

  test("a member cannot act as an organizer, and a guard says so", async () => {
    const { club } = lab.apps;
    const member = runtimeClient(lab, club.hostname, (await appUser(lab, club)).cookie);
    await invoke(member, "me.join", { display: "Mallory" });
    const proposed = await invoke(member, "books.propose", { title: "A", author: "B", coverFileId: null });
    await expect(invoke(member, "books.schedule", { id: proposed.lastInsertId, meetsOn: "2027-01-01" }))
      .rejects.toThrow("Only an organizer can schedule a meeting");
    await expect(invoke(member, "books.remove", { id: proposed.lastInsertId })).rejects.toThrow("Only an organizer can remove a book.");
  });

  test("naming another user's private file in a row does not make the file readable", async () => {
    const { club } = lab.apps;
    const owner = await appUser(lab, club);
    const ownerClient = runtimeClient(lab, club.hostname, owner.cookie);
    const transfer = await ownerClient.upload(new TextEncoder().encode("private notes"));
    const file = await ownerClient.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "notes.txt", type: "text/plain" });
    const thief = await appUser(lab, club);
    const thiefClient = runtimeClient(lab, club.hostname, thief.cookie);
    await invoke(thiefClient, "me.join", { display: "Thief" });
    const book = await invoke(thiefClient, "books.propose", { title: "Bait", author: "X", coverFileId: file.id });
    expect(book.changes).toBe(1);
    expect((await hostFetch(lab, club.hostname, `/_littlebox/files/${file.id}`, { headers: { cookie: thief.cookie } })).status).toBe(404);
    await expect(thiefClient.call("files_read", { fileId: file.id })).rejects.toThrow("File not found.");
    await expect(thiefClient.call("files_update", { fileId: file.id, visibility: "public" })).rejects.toThrow("File not found.");
    await expect(thiefClient.call("files_delete", { fileId: file.id })).rejects.toThrow("File not found.");
    expect((await hostFetch(lab, club.hostname, `/_littlebox/files/${file.id}`, { headers: { cookie: owner.cookie } })).status).toBe(200);
  });

  test("a suspended user's open session is refused on the runtime, on a route and on the file path", async () => {
    const { habits } = lab.apps;
    const user = await appUser(lab, habits);
    const client = runtimeClient(lab, habits.hostname, user.cookie);
    await invoke(client, "habits.create", { name: "Before" });
    const transfer = await client.upload(new TextEncoder().encode("mine"));
    const file = await client.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "a.txt", type: "text/plain" });
    const { users } = await lab.kernel.owner.call<{ users: { userId: string; email: string }[] }>("app_users_list", { deploymentId: habits.id });
    await lab.kernel.owner.call("app_user_suspend", { deploymentId: habits.id, userId: users.find((row) => row.email === user.email)!.userId });
    await expect(invoke(client, "habits.list")).rejects.toThrow();
    expect((await hostFetch(lab, habits.hostname, "/api/habits", { headers: { cookie: user.cookie } })).status).toBe(401);
    expect((await hostFetch(lab, habits.hostname, `/_littlebox/files/${file.id}`, { headers: { cookie: user.cookie } })).status).toBe(401);
    // A bearer copy of the same session token is the same session.
    expect((await hostFetch(lab, habits.hostname, "/api/habits", { headers: { authorization: `Bearer ${user.token}` } })).status).toBe(401);
  });
});

describe("sessions across Deployments and hosts", () => {
  test("a session from one app, as a cookie or a bearer token, is unknown to every other app", async () => {
    const { club, habits, gallery } = lab.apps;
    const member = await appUser(lab, club);
    for (const app of [habits, gallery]) {
      for (const headers of [{ cookie: member.cookie }, { authorization: `Bearer ${member.token}` }]) {
        const answer = await rawCall(lab, "/_littlebox/mcp", app.hostname, "auth_session", {}, headers);
        expect(answerText(answer.body)).not.toContain(member.email);
      }
    }
    // A private app's route does not take it either.
    expect((await hostFetch(lab, habits.hostname, "/api/habits", { headers: { authorization: `Bearer ${member.token}` } })).status).toBe(401);
  });

  test("an app session is not an owner session on the editor host, and an owner session is not an app session", async () => {
    const { club } = lab.apps;
    const member = await appUser(lab, club);
    const editor = new URL(lab.kernel.url).host;
    for (const cookie of [member.cookie, `littlebox_owner_session=${member.token}`]) {
      expect((await rawCall(lab, "/mcp", editor, "projects_list", {}, { cookie })).status).toBe(401);
    }
    const ownerToken = lab.kernel.ownerCookie.split("=")[1];
    for (const headers of [{ cookie: lab.kernel.ownerCookie }, { cookie: `littlebox_app_session=${ownerToken}` }, { authorization: `Bearer ${ownerToken}` }]) {
      expect(answerText((await rawCall(lab, "/_littlebox/mcp", club.hostname, "actions_invoke", { name: "me.get", input: {} }, headers)).body)).not.toContain("result");
    }
  });

  test("Host header variants never select another Deployment or a session", async () => {
    const { club, gallery } = lab.apps;
    const member = await appUser(lab, club);
    // Upper case, a trailing dot and a sub-host are not the Deployment's hostname.
    for (const host of [club.hostname.toUpperCase(), `${club.hostname}.`, `x.${club.hostname}`, `${club.hostname}@${gallery.hostname}`]) {
      const answer = await rawCall(lab, "/_littlebox/mcp", host, "auth_session", {}, { cookie: member.cookie });
      expect([403, 404]).toContain(answer.status);
    }
    // X-Forwarded-Host is not read: the request stays on the gallery, where the club session means nothing.
    const forwarded = await rawCall(lab, "/_littlebox/mcp", gallery.hostname, "auth_session", {}, { cookie: member.cookie, "x-forwarded-host": club.hostname });
    expect(answerText(forwarded.body)).toContain("anonymous");
  });

  test("a page on another app or a sandboxed frame cannot use a member's session", async () => {
    const { club, gallery, habits } = lab.apps;
    const member = await appUser(lab, habits);
    for (const origin of [`http://${gallery.hostname}`, `http://${club.hostname}:1`, "null"]) {
      const runtime = await rawCall(lab, "/_littlebox/mcp", habits.hostname, "actions_invoke", { name: "habits.list", input: {} }, { cookie: member.cookie, origin });
      expect(runtime.status).toBe(403);
      // On a route the session is dropped and the caller is a visitor, which a private app refuses.
      const route = await hostFetch(lab, habits.hostname, "/api/habits", { method: "POST", headers: { cookie: member.cookie, origin }, body: JSON.stringify({ name: "csrf" }) });
      expect(route.status).toBe(401);
    }
    // A same-site request without Origin, such as an <img> on a sibling app, gets no session either.
    const sibling = await hostFetch(lab, habits.hostname, "/api/habits", { headers: { cookie: member.cookie, "sec-fetch-site": "same-site" } });
    expect(sibling.status).toBe(401);
  });
});
