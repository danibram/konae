// A hostile app user and a hostile program against sign-in, recovery,
// invitations, creator admission, account email and the Kernel's own pages
// on the showcase apps. Each test is one attempt from the security review.
// The minute limit per address is raised so one test can reach the day limit;
// the minute limit itself has a Kernel test in mail.rs.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { appUser, freshClient, hostFetch, password, repo, startLab, testPort, type App, type Lab } from "./security-review-kernel";

let lab: Lab;
beforeAll(async () => {
  lab = await startLab(testPort("LITTLEBOX_REVIEW_ACCOUNTS_PORT", 24200), { LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_MINUTE: "100" });
}, 180_000);
afterAll(async () => { await lab?.stop(); });

const post = (app: App, route: string, body: object, client = freshClient()) =>
  hostFetch(lab, app.hostname, route, { method: "POST", client, body: JSON.stringify(body) });
const answer = async (response: Response) => ({ status: response.status, body: await response.json() });
const mails = async (to: string) => (await lab.kernel.mailbox()).filter((mail) => mail.to === to);
const recoveryToken = (link: string) => new URL(link).hash.slice(1).split("=")[1];

describe("enumeration", () => {
  test("login answers a wrong password and an unknown email the same way", async () => {
    const { club } = lab.apps;
    const member = await appUser(lab, club);
    const known = await answer(await post(club, "/_littlebox/auth/login", { email: member.email, password: "wrong-password-123" }));
    const unknown = await answer(await post(club, "/_littlebox/auth/login", { email: `nobody-${crypto.randomUUID()}@review.test`, password: "wrong-password-123" }));
    expect(unknown).toEqual(known);
    expect(known.status).toBe(401);
    // auth.rs runs the full password hash for an unknown email (dummy_hash), so the timing matches too.
  });

  test("recovery answers a known and an unknown address the same way and mails only the known one", async () => {
    const { club } = lab.apps;
    const member = await appUser(lab, club);
    const stranger = `stranger-${crypto.randomUUID().slice(0, 8)}@review.test`;
    const known = await answer(await post(club, "/_littlebox/auth/recover", { email: member.email }));
    const unknown = await answer(await post(club, "/_littlebox/auth/recover", { email: stranger }));
    expect(unknown).toEqual(known);
    await Bun.sleep(1500);
    expect(await mails(member.email)).toHaveLength(1);
    // Recovery is no relay: an address with no account gets nothing.
    expect(await mails(stranger)).toHaveLength(0);
  });

  test("creator signup with a made-up code answers a taken and a new email the same way", async () => {
    const editor = new URL(lab.kernel.url).host;
    const signup = (email: string) => fetch(`${lab.kernel.url}/_littlebox/owner/signup`, {
      method: "POST", headers: { host: editor, "content-type": "application/json", "x-forwarded-for": freshClient() },
      body: JSON.stringify({ email, password, code: "00000-00000-00000-00000-00000" }),
    }).then(answer);
    const taken = await signup("owner@review.test");
    expect(taken).toEqual(await signup(`new-${crypto.randomUUID().slice(0, 8)}@review.test`));
    expect(taken.status).toBe(403);
  });
});

describe("links and tokens", () => {
  test("a recovery link works once, only on its own app, and ends the account's other sessions", async () => {
    const { club, habits } = lab.apps;
    const member = await appUser(lab, club);
    await post(club, "/_littlebox/auth/recover", { email: member.email });
    await Bun.sleep(1500);
    const [mail] = await mails(member.email);
    const token = recoveryToken(mail.link);
    const elsewhere = await answer(await post(habits, "/_littlebox/auth/reset", { token, password: "a-new-password-2026" }));
    expect(elsewhere.status).toBeGreaterThanOrEqual(400);
    const first = await post(club, "/_littlebox/auth/reset", { token, password: "a-new-password-2026" });
    expect(first.status).toBe(200);
    const again = await answer(await post(club, "/_littlebox/auth/reset", { token, password: "another-password-2026" }));
    expect(again).toEqual(elsewhere);
    const old = await hostFetch(lab, club.hostname, "/_littlebox/mcp", { method: "POST", headers: { cookie: member.cookie }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name: "me.get", input: {} } } }) });
    expect(JSON.stringify(await old.json())).not.toContain('"user_id"');
  });

  test("a guessed invitation token gets the answer a malformed one gets, and guessing is rate limited", async () => {
    const { inbox } = lab.apps;
    const client = freshClient();
    const guessed = await answer(await post(inbox, "/_littlebox/auth/invitation", { token: "a".repeat(64) }, client));
    const malformed = await answer(await post(inbox, "/_littlebox/auth/invitation", { token: "x" }, client));
    expect(guessed.status).toBe(malformed.status);
    expect(guessed.body).toEqual(malformed.body);
    const statuses: number[] = [];
    for (let attempt = 0; attempt < 12; attempt++) {
      statuses.push((await post(inbox, "/_littlebox/auth/invitation", { token: crypto.randomUUID().replaceAll("-", "").repeat(2) }, client)).status);
    }
    expect(statuses).toContain(429);
  });

  test("creator invitation codes cannot be guessed faster than the owner limit", async () => {
    const editor = new URL(lab.kernel.url).host;
    const client = freshClient();
    const statuses: number[] = [];
    for (let attempt = 0; attempt < 12; attempt++) {
      const response = await fetch(`${lab.kernel.url}/_littlebox/owner/signup`, {
        method: "POST", headers: { host: editor, "content-type": "application/json", "x-forwarded-for": client },
        body: JSON.stringify({ email: "guess@review.test", password, code: Array.from({ length: 25 }, () => "0123456789ABCDEFGHJKMNPQRSTVWXYZ"[Math.floor(Math.random() * 32)]).join("") }),
      });
      statuses.push(response.status);
      await response.arrayBuffer();
    }
    // The owner limiter allows 5 attempts a minute per email and 10 per client address, refilling over the minute.
    const firstLimited = statuses.indexOf(429);
    expect(firstLimited).toBeGreaterThan(0);
    expect(firstLimited).toBeLessThanOrEqual(10);
    expect(statuses.slice(0, firstLimited).every((status) => status === 403)).toBe(true);
  });
});

describe("account email", () => {
  test("an address written with display-name, comment or list syntax is refused", async () => {
    const { gallery } = lab.apps;
    for (const email of ["x<victim@review.test>", "victim@review.test>", "(c)victim@review.test", "a,b@review.test", "a\r\nbcc:b@review.test", "\"a\"@review.test"]) {
      const response = await answer(await post(gallery, "/_littlebox/auth/signup", { email, password }));
      expect(response, email).toEqual({ status: 400, body: { error: "Enter a valid email address." } });
      // Recovery answers every input the same way and mails nothing for this one.
      expect((await post(gallery, "/_littlebox/auth/recover", { email })).status).toBe(202);
    }
    await Bun.sleep(1500);
    expect((await lab.kernel.mailbox()).filter((mail) => mail.to.includes("victim@review.test"))).toEqual([]);
  });

  test("requests on one app cannot use up an address's day on another app", async () => {
    const { gallery, booking } = lab.apps;
    const victim = `victim-${crypto.randomUUID().slice(0, 8)}@review.test`;
    // Twelve requests at once for an address with no account: the day limit holds under the race.
    const answers = await Promise.all(Array.from({ length: 12 }, () => post(gallery, "/_littlebox/auth/recover", { email: victim }).then((response) => response.status)));
    expect(answers.filter((status) => status === 202)).toHaveLength(10);
    expect(answers.filter((status) => status === 429)).toHaveLength(2);
    const blocked = await answer(await post(gallery, "/_littlebox/auth/recover", { email: victim }));
    expect(blocked).toEqual({ status: 429, body: { error: "This address has received the most account emails allowed today. Try again tomorrow." } });
    // The same person signs up on another app that day.
    expect((await post(booking, "/_littlebox/auth/signup", { email: victim, password })).status).toBe(200);
  });
});

describe("the Kernel's own pages on an app host", () => {
  test("sign-in, account and invitation pages run only their own script, reflect nothing and cannot be framed", async () => {
    const { club } = lab.apps;
    const probe = encodeURIComponent(`"><script>alert(1)</script><img src=x onerror=alert(2)>`);
    for (const route of [`/_littlebox/login?next=${probe}`, `/_littlebox/account?token=${probe}`, `/_littlebox/invitation?token=${probe}`]) {
      const response = await hostFetch(lab, club.hostname, route);
      const html = await response.text();
      expect(html, route).not.toContain("alert(");
      const policy = response.headers.get("content-security-policy")!;
      const scripts = policy.split(";").map((part) => part.trim()).find((part) => part.startsWith("script-src"))!;
      expect(scripts).not.toContain("unsafe-inline");
      expect(scripts).toMatch(/^script-src 'self' 'sha256-[^']+'$/);
      expect(policy).toContain("frame-ancestors 'none'");
      expect(response.headers.get("x-frame-options")).toBe("DENY");
    }
  });

  test("an app session cookie is host-only, HttpOnly and Lax, and carries nothing from the request", async () => {
    const { club } = lab.apps;
    const response = await post(club, "/_littlebox/auth/signup", { email: `cookie-${crypto.randomUUID().slice(0, 8)}@review.test`, password });
    const cookies = response.headers.getSetCookie();
    expect(cookies).toHaveLength(1);
    expect(cookies[0]).toMatch(/^littlebox_app_session=[0-9a-f]{64}; /);
    expect(cookies[0]).toContain("HttpOnly");
    expect(cookies[0]).toContain("SameSite=Lax");
    expect(cookies[0]).not.toContain("Domain=");
  });
});

describe("the editor", () => {
  test("no editor view writes HTML from data", () => {
    // The Users, Invitations, Integrations, secrets and API key views render
    // emails, names and keys as React text. A view that set HTML from data
    // would turn an app user's email into script on the editor's origin.
    const offenders: string[] = [];
    const walk = (directory: string) => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const file = path.join(directory, entry.name);
        if (entry.isDirectory()) walk(file);
        else if (/\.tsx?$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
          const source = readFileSync(file, "utf8");
          if (/dangerouslySetInnerHTML|\.innerHTML\s*=|\.outerHTML\s*=|insertAdjacentHTML|document\.write/.test(source)) offenders.push(path.relative(repo, file));
        }
      }
    };
    walk(path.join(repo, "src/components"));
    expect(offenders).toEqual([]);
  });
});
