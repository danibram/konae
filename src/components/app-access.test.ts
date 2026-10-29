import { describe, expect, test } from "bun:test";
import { signupModes } from "../deployments/client";
import googleCases from "../../kernel-rs/src/google-client-cases.json";
import { ownerFixes } from "../kernel/refusal";
import { McpError } from "../mcp/client";
import {
  apiKeyProblem, batchKey, googleRefusal, redirectUriLabel, revokedKeyNotes, defaultSignupMode, expiryText, fileCurl, firstPublishModes, invitationResultMessage, invitationView,
  mailReadiness, parseEmailBatch, republishAccessNote, routeActions, routeCurl, secretProblem, secretUsage, signupModeCopy,
  userStateView, visibilityCopy,
} from "./app-access";

const routes = JSON.stringify({
  version: 1,
  actions: {
    "orders.receive": {
      kind: "mutation", access: "anonymous",
      http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } },
      statement: "INSERT INTO orders (external_id, total) VALUES (?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 64 }, { source: "input", key: "total", type: "number" }],
    },
    "orders.list": {
      kind: "query", access: "anonymous",
      http: { method: "GET", path: "/api/orders" },
      statement: "SELECT id, total FROM orders WHERE total >= ?1 LIMIT 50",
      bindings: [{ source: "input", key: "min", type: "number" }],
    },
    "orders.mine": {
      kind: "query", access: "authenticated",
      statement: "SELECT id FROM orders WHERE user_id = ?1 LIMIT 50",
      bindings: [{ source: "actor", field: "id" }],
    },
  },
});

/** Every sentence the views show about access, to check the vocabulary once. */
function allCopy() {
  return [
    ...Object.values(signupModeCopy).map((copy) => copy.sentence),
    ...Object.values(visibilityCopy).map((copy) => copy.sentence),
    ...Object.values(userStateView).map((copy) => copy.sentence),
    republishAccessNote({ visibility: "public", mode: "open" }, "private"),
    republishAccessNote({ visibility: "private", mode: "invitation" }, "public"),
    republishAccessNote({ visibility: "private", mode: "invitation" }, "private"),
  ];
}

describe("signup policy copy", () => {
  test("explains each mode in one sentence and marks Allowlist advanced", () => {
    for (const mode of signupModes) {
      const sentence = signupModeCopy[mode].sentence;
      expect(sentence.endsWith(".")).toBe(true);
      expect(sentence.slice(0, -1)).not.toContain(". ");
    }
    expect(signupModeCopy.invitation.label).toBe("Invitation only");
    // ADR-0011, note of 2026-11-06 on Closed: the owner's invitation still works.
    expect(signupModeCopy.closed.sentence).toContain("people you invite can still accept");
    expect(signupModes.filter((mode) => signupModeCopy[mode].advanced)).toEqual(["allowlist"]);
  });

  test("a first publish proposes Invitation only for a private app and Open for a public one", () => {
    expect(defaultSignupMode("private")).toBe("invitation");
    expect(defaultSignupMode("public")).toBe("open");
    // Allowlist needs its list, so the dialog leaves it to the Users view.
    expect(firstPublishModes).not.toContain("allowlist");
  });

  test("a republish says the policy stays and where to change it", () => {
    expect(republishAccessNote({ visibility: "private", mode: "invitation" }, "private")).toContain("Auth > Users");
    expect(republishAccessNote({ visibility: "public", mode: "open" }, "private")).toContain("turns Open signup into Closed");
    expect(republishAccessNote({ visibility: "public", mode: "invitation" }, "private")).not.toContain("Closed");
  });

  test("speaks of app accounts and never of creating an app when it means joining one", () => {
    for (const sentence of allCopy()) {
      expect(sentence).not.toMatch(/create (an|the|your) app\b(?! account)/i);
      expect(sentence).not.toMatch(/\bsign up for Littlebox\b/i);
      // A person who uses the app has an app account; "account" alone could mean the owner's.
      if (/\baccount\b/.test(sentence)) expect(sentence).toMatch(/app account/);
    }
  });
});

describe("invitation batches", () => {
  test("splits, trims, lowercases and drops repeated addresses", () => {
    expect(parseEmailBatch(" Ana@Example.com,\nbo@example.com; ana@example.com  \n\n")).toEqual({ emails: ["ana@example.com", "bo@example.com"], tooMany: false });
  });

  test("refuses more than 50 addresses before calling the Kernel", () => {
    const text = Array.from({ length: 51 }, (_, index) => `p${index}@example.com`).join("\n");
    expect(parseEmailBatch(text).tooMany).toBe(true);
    expect(parseEmailBatch(text.split("\n").slice(0, 50).join("\n")).tooMany).toBe(false);
  });

  test("a retry of the same addresses reuses the idempotency key and a new list gets a new one", () => {
    let count = 0;
    const next = () => `key-${++count}`;
    const first = batchKey(null, ["ana@example.com", "bo@example.com"], next);
    expect(first.key).toBe("key-1");
    expect(batchKey(first, ["bo@example.com", "ana@example.com"], next)).toBe(first);
    expect(batchKey(first, ["ana@example.com"], next).key).toBe("key-2");
  });

  test("the default key fits the Kernel's 8 to 128 characters", () => {
    const { key } = batchKey(null, ["ana@example.com"]);
    expect(key.length).toBeGreaterThanOrEqual(8);
    expect(key.length).toBeLessThanOrEqual(128);
  });
});

describe("invitation state", () => {
  test("open invitations can be resent and revoked; finished ones cannot", () => {
    for (const status of ["pending", "expired", "needs-resend"] as const) {
      expect(invitationView({ status, delivery: "queued", sends: 1 })).toMatchObject({ canResend: true, canRevoke: true });
    }
    for (const status of ["accepted", "revoked", "already-member"] as const) {
      expect(invitationView({ status, delivery: "queued", sends: 1 })).toMatchObject({ canResend: false, canRevoke: false });
    }
  });

  test("shows only what the Kernel knows about the email", () => {
    expect(invitationView({ status: "pending", delivery: "queued", sends: 2 }).delivery.label).toBe("Handed to mail");
    expect(invitationView({ status: "pending", delivery: "failed", sends: 1 }).delivery).toEqual({ label: "Sending failed", tone: "danger" });
    expect(invitationView({ status: "pending", delivery: "not-sent", sends: 0 }).delivery.label).toBe("No email sent");
    expect(invitationView({ status: "needs-resend", delivery: "not-sent", sends: 1 }).delivery.label).toBe("Link no longer works");
    expect(invitationView({ status: "pending", delivery: "queued", sends: 2 }).sends).toBe("Sent 2 times");
    // Nothing claims delivery to an inbox: the Kernel records no per-invitation provider state.
    for (const delivery of ["queued", "failed", "not-sent"] as const) {
      expect(invitationView({ status: "pending", delivery, sends: 1 }).delivery.label).not.toMatch(/delivered/i);
    }
  });

  test("shows expiry, acceptance and revocation dates", () => {
    const now = 1_800_000_000;
    expect(expiryText({ status: "pending", expiresAt: now + 86_400, acceptedAt: null, revokedAt: null }, now, "en-US")).toMatch(/^Expires /);
    expect(expiryText({ status: "expired", expiresAt: now - 1, acceptedAt: null, revokedAt: null }, now, "en-US")).toMatch(/^Expired /);
    expect(expiryText({ status: "accepted", expiresAt: now, acceptedAt: now, revokedAt: null }, now, "en-US")).toMatch(/^Accepted /);
    expect(expiryText({ status: "revoked", expiresAt: now, acceptedAt: null, revokedAt: now }, now, "en-US")).toMatch(/^Revoked /);
  });

  test("says per address what an invite did", () => {
    expect(invitationResultMessage({ email: "a@x.test", status: "invited", invitationId: "i", expiresAt: 1, acceptable: true, delivery: "queued" }).tone).toBe("success");
    expect(invitationResultMessage({ email: "a@x.test", status: "invited", invitationId: "i", expiresAt: 1, acceptable: false, delivery: "queued" }).text).toContain("allowlist does not include this address");
    expect(invitationResultMessage({ email: "a@x.test", status: "invited", invitationId: "i", expiresAt: 1, acceptable: true, delivery: "failed" }).text).toContain("Resend");
    expect(invitationResultMessage({ email: "a@x.test", status: "already-member" }).text).toBe("Already has an active app account.");
    expect(invitationResultMessage({ email: "a@x.test", status: "suspended" }).text).toContain("Resume");
    expect(invitationResultMessage({ email: "nope", status: "invalid-email" }).tone).toBe("danger");
  });
});

describe("mail readiness", () => {
  test("a ready transport enables invite and resend", () => {
    expect(mailReadiness({ transport: "development", sending: true, preflight: { state: "ready" } })).toEqual({ ready: true, reason: "" });
  });

  test("no transport, a refused sender and a running check each give their reason", () => {
    expect(mailReadiness({ transport: null, sending: false, reason: "No mail transport is configured." }).reason).toContain("LITTLEBOX_MAIL_TRANSPORT");
    const refused = mailReadiness({ transport: "resend", sending: false, preflight: { state: "refused", reason: "Domain example.com is not verified" } });
    expect(refused.ready).toBe(false);
    expect(refused.reason).toContain("Domain example.com is not verified.");
    expect(mailReadiness({ transport: "resend", sending: false, preflight: { state: "checking" } }).reason).toContain("still checking");
    expect(mailReadiness(null).ready).toBe(false);
  });
});

describe("integrations", () => {
  test("lists the actions with a route and the secrets they sign with", () => {
    expect(routeActions(routes).map((route) => `${route.method} ${route.path}`)).toEqual(["GET /api/orders", "POST /hooks/orders"]);
    expect(secretUsage(routes)).toEqual(new Map([["orders_webhook", ["orders.receive"]]]));
    expect(routeActions("not json")).toEqual([]);
  });

  test("checks secrets the way secret_set does", () => {
    expect(secretProblem("orders_webhook", "a-long-enough-secret")).toBe("");
    expect(secretProblem("Orders", "a-long-enough-secret")).toContain("lowercase");
    expect(secretProblem("orders_webhook", "short")).toContain("16 to 512");
    expect(secretProblem("orders_webhook", "has a space in it ok")).toContain("without spaces");
  });

  test("a key needs a name and at least one route or file visibility", () => {
    expect(apiKeyProblem("", ["orders.list"], [])).toContain("Name");
    expect(apiKeyProblem("Billing", [], [])).toContain("at least one");
    expect(apiKeyProblem("Billing", [], ["private"])).toBe("");
  });

  test("lists keys revoked at import apart, with what each could call, so the owner knows what to reissue", () => {
    const key = (name: string, revokedReason: "import" | null, actions: string[] = [], files: ("public" | "private")[] = []) =>
      ({ name, actions, files, revokedAt: 5, revokedReason });
    expect(revokedKeyNotes([{ ...key("Live", null), revokedAt: null }])).toEqual([]);
    expect(revokedKeyNotes([key("Old", null)])).toEqual(["1 revoked key: Old."]);
    const notes = revokedKeyNotes([key("Old", null), key("Mirror", "import", [], ["public"]), key("Orders", "import", ["orders.receive"])]);
    expect(notes[0]).toBe("1 revoked key: Old.");
    expect(notes[1]).toBe("Revoked at import, because keys do not work on another machine: Mirror (public files); Orders (orders.receive). Create a new key for each program that still needs access.");
  });

  test("builds the curl docs/mcp.md describes for each route", () => {
    const [list, receive] = routeActions(routes);
    expect(routeCurl("https://orders-1a2b.example.net/", list, "lbk_test")).toBe('curl -H "Authorization: Bearer lbk_test" "https://orders-1a2b.example.net/api/orders?min=1"');
    expect(routeCurl("https://orders-1a2b.example.net", receive, "lbk_test")).toBe(`curl -X POST -H "Authorization: Bearer lbk_test" -H 'Content-Type: application/json' --data '{"id":"text","total":1}' "https://orders-1a2b.example.net/hooks/orders"`);
    expect(routeCurl("https://orders-1a2b.example.net", receive)).toContain("Bearer $LITTLEBOX_API_KEY");
    expect(fileCurl("https://orders-1a2b.example.net", "lbk_test")).toBe('curl -OJ -H "Authorization: Bearer lbk_test" "https://orders-1a2b.example.net/_littlebox/files/<file id>"');
  });

  test("a route with fields gets a curl whose body nests each input at its path", () => {
    const source = JSON.stringify({ version: 1, actions: { "payments.receive": {
      kind: "mutation", access: "anonymous", statement: "INSERT INTO payments (id, amount) VALUES (?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string" }, { source: "input", key: "amount", type: "number" }],
      http: { method: "POST", path: "/hooks/payments", signature: { scheme: "timestamped", header: "Stripe-Signature", secret: "payments_webhook", prefix: "t=", tag: "v1=", window: 300 }, fields: { id: "$.data.object.id" } },
    } } });
    const [route] = routeActions(source);
    expect(route.fields).toEqual({ id: "$.data.object.id" });
    expect(routeCurl("https://shop.example.net", route, "lbk_test")).toContain(`--data '{"data":{"object":{"id":"text"}},"amount":1}'`);
  });
});

describe("googleRefusal", () => {
  test("shows the Kernel's sentence for every case in google-client-cases.json, by the field it is about", () => {
    const refused = googleCases.cases.filter((item) => typeof item.error === "string");
    expect(refused.length).toBeGreaterThan(5);
    for (const item of refused) {
      const saving = item.input.name === "google_client_secret" ? "clientSecret" : "clientId";
      const cause = new McpError(item.error!, undefined, { code: item.code!, message: item.error! });
      const shown = googleRefusal(cause, saving);
      expect(shown?.message, item.case).toBe(item.error!);
      expect(shown?.field, item.case).toBe(item.tool === "secret_set" || item.tool === "secret_delete" ? saving : undefined);
      expect(ownerFixes(item.code), item.case).toBe(true);
    }
    // An answer without an owner's refusal code is shown as it is.
    expect(googleRefusal(new Error("Network down"))).toBeNull();
    expect(googleRefusal(new McpError("Unknown tool."))).toBeNull();
  });
});

describe("redirectUriLabel", () => {
  test("names the host, the custom domain and the path address", () => {
    expect(redirectUriLabel("https://notes-1.littlebox.com/_littlebox/auth/google/callback", 0)).toBe("Redirect URI");
    expect(redirectUriLabel("https://notes.example.org/_littlebox/auth/google/callback", 1)).toBe("Redirect URI for your custom domain");
    expect(redirectUriLabel("https://littlebox.com/p/0b6c/_littlebox/auth/google/callback", 1)).toBe("Redirect URI for the path address");
  });
});
