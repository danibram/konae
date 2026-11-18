import assert from "node:assert/strict";

// Entirely synthetic policy fixture. This is not connected to Konae or any
// owner-history route and deliberately contains recognizable secret markers.
const records = [
  { owner: "owner-a", project: "project-a", conversation: "a-1", entry: "entry-a-1", category: "stale-source", outcome: "refusal", durationMs: 640, text: "SYNTHETIC_A_PRIVATE_TEXT", payload: "SYNTHETIC_A_PAYLOAD", intent: null },
  { owner: "owner-a", project: "project-a", conversation: "a-2", entry: "entry-a-2", category: "interrupted-recovery", outcome: "recovered", durationMs: 1800, text: "SYNTHETIC_A_OTHER_TEXT", payload: "SYNTHETIC_A_OTHER_PAYLOAD", intent: { value: "repair form", sourceEntry: "entry-a-2" } },
  { owner: "owner-b", project: "project-b", conversation: "b-1", entry: "entry-b-1", category: "stale-source", outcome: "refusal", durationMs: 7200, text: "SYNTHETIC_B_PRIVATE_TEXT", payload: "SYNTHETIC_B_PAYLOAD", intent: null },
  { owner: "owner-b", project: "project-b", conversation: "b-2", entry: "entry-b-2", category: "interrupted-recovery", outcome: "recovered", durationMs: null, text: "SYNTHETIC_B_OTHER_TEXT", payload: "SYNTHETIC_B_OTHER_PAYLOAD", intent: { value: "ship list", sourceEntry: "entry-b-2" } },
  ...["owner-c", "owner-d", "owner-e"].map((owner, index) => ({
    owner, project: `project-${owner}`, conversation: `c-${index}`, entry: `entry-${index}`,
    category: "stale-source", outcome: "refusal", durationMs: 2400,
    text: `SYNTHETIC_${owner}_PRIVATE_TEXT`, payload: `SYNTHETIC_${owner}_PAYLOAD`, intent: null,
  })),
];
const reportRecipient = { kind: "maintainer-report", id: "maintainers-category-report" };

const actors = {
  ownerA: { kind: "owner", id: "owner-a", role: "owner" },
  ownerB: { kind: "owner", id: "owner-b", role: "owner" },
  operator: { kind: "operator", id: "operator-1", role: "operator" },
  delegatedRead: { kind: "delegated", id: "token-1", role: "read", projects: ["project-a", "project-b"] },
};

function currentOwnerDetail(actor, record) {
  if (actor.kind === "owner" && actor.id === record.owner) return { text: record.text, payload: record.payload };
  throw new Error("owner session and conversation ownership required");
}

function proposedAggregate(actor, records, grants, now, minimumOwners = 5) {
  if (actor.kind !== "maintainer-report" || actor.id !== "maintainers-category-report") return [];
  const allowed = records.filter((record) => {
    const grant = grants[record.owner];
    return record.deletedAt == null && grant?.revokedAt == null && grant?.purpose === "category-report" &&
      grant?.recipient === actor.id &&
      grant?.fields?.includes("category") && grant?.expiresAt > now &&
      grant?.retainUntil > now && grant?.windowStart <= record.receivedAt &&
      record.receivedAt <= grant?.windowEnd;
  });
  const buckets = new Map();
  for (const record of allowed) {
    const bucket = buckets.get(record.category) ?? new Set();
    bucket.add(record.owner);
    buckets.set(record.category, bucket);
  }
  return [...buckets].flatMap(([category, owners]) => owners.size < minimumOwners ? [] : [{ category, owners: owners.size }]);
}

function resolveFutureDetail(actor, record, grant, now) {
  if (actor.kind !== "owner" || actor.id !== record.owner) throw new Error("owner detail authentication required");
  if (!grant || grant.revokedAt != null || grant.expiresAt <= now || grant.retainUntil <= now || grant.purpose !== "owner-detail") throw new Error("valid owner detail authorization required");
  if (grant.provenance?.owner !== actor.id || grant.provenance?.project !== record.project ||
      grant.provenance?.conversation !== record.conversation || grant.provenance?.entry !== record.entry ||
      grant.sourceEntry !== record.entry) throw new Error("detail provenance mismatch");
  return record.text;
}

// Existing contract: own owner can read; independent owner, operator role,
// ordinary delegated read and app-account-like principal cannot.
assert.deepEqual(currentOwnerDetail(actors.ownerA, records[0]), { text: "SYNTHETIC_A_PRIVATE_TEXT", payload: "SYNTHETIC_A_PAYLOAD" });
for (const actor of [actors.ownerB, actors.operator, actors.delegatedRead, { kind: "app-account", id: "actor-1", role: "owner" }]) {
  assert.throws(() => currentOwnerDetail(actor, records[0]), /ownership required/);
}
assert.throws(() => currentOwnerDetail(actors.ownerA, records[2]), /ownership required/);

// A role string is not authorization. Proposed aggregate access requires each
// owner to grant the exact purpose/fields/window/expiry/retention explicitly.
const now = 1000;
const received = records.map((record, index) => ({ ...record, receivedAt: 100 + index }));
const grants = Object.fromEntries(["owner-a", "owner-b", "owner-c", "owner-d", "owner-e"].map((id) => [id, {
  recipient: reportRecipient.id,
  purpose: "category-report", fields: ["category", "outcome", "durationBucket"],
  windowStart: 0, windowEnd: 500, expiresAt: 2000, retainUntil: 1800,
  revokedAt: null,
} ]));
const visible = proposedAggregate(reportRecipient, received, grants, now);
assert.deepEqual(visible, [{ category: "stale-source", owners: 5 }]);
assert.deepEqual(proposedAggregate(actors.operator, received, grants, now), []);
assert.deepEqual(proposedAggregate({ kind: "maintainer-report", id: "unapproved-recipient" }, received, grants, now), []);
assert.deepEqual(proposedAggregate(reportRecipient, received, {}, now), []);
assert.deepEqual(proposedAggregate(reportRecipient, received, { "owner-a": { ...grants["owner-a"], role: "operator" } }, now), []);
assert.ok(!JSON.stringify(visible).includes("SYNTHETIC_"));

// Start each negative case from a visible result, then show the state change
// makes it disappear instead of passing vacuously below the cohort threshold.
const revoked = structuredClone(grants);
revoked["owner-b"].revokedAt = 900;
assert.deepEqual(proposedAggregate(reportRecipient, received, revoked, now), []);
const expired = structuredClone(grants);
expired["owner-a"].expiresAt = now;
assert.deepEqual(proposedAggregate(reportRecipient, received, expired, now), []);
const retentionElapsed = structuredClone(grants);
retentionElapsed["owner-a"].retainUntil = now;
assert.deepEqual(proposedAggregate(reportRecipient, received, retentionElapsed, now), []);
const deleted = received.map((record) => record.owner === "owner-a" ? { ...record, deletedAt: now } : record);
assert.deepEqual(proposedAggregate(reportRecipient, deleted, grants, now), []);

// Even proposed detail is owner-authenticated, purpose-scoped and tied to the
// owner/conversation/source entry; an aggregate row is not a bearer link.
const detailGrant = {
  purpose: "owner-detail", expiresAt: 2000, retainUntil: 1800, revokedAt: null, sourceEntry: "entry-a-2",
  provenance: { owner: "owner-a", project: "project-a", conversation: "a-2", entry: "entry-a-2" },
};
assert.equal(resolveFutureDetail(actors.ownerA, records[1], detailGrant, now), "SYNTHETIC_A_OTHER_TEXT");
assert.throws(() => resolveFutureDetail(actors.operator, records[1], detailGrant, now), /authentication required/);
assert.throws(() => resolveFutureDetail(actors.ownerA, records[2], detailGrant, now), /authentication required/);
assert.throws(() => resolveFutureDetail(actors.ownerA, records[1], { ...detailGrant, provenance: { ...detailGrant.provenance, project: "project-b" } }, now), /provenance mismatch/);
assert.throws(() => resolveFutureDetail(actors.ownerA, { ...records[1], entry: "forged-entry" }, detailGrant, now), /provenance mismatch/);
assert.throws(() => resolveFutureDetail(actors.ownerA, records[1], { ...detailGrant, revokedAt: now }, now), /authorization required/);
assert.throws(() => resolveFutureDetail(actors.ownerA, records[1], { ...detailGrant, retainUntil: now }, now), /authorization required/);

// Optional intent is retained only when its declared source is the actual
// message entry; absent intent stays absent and is not inferred.
const visibleIntent = (record, actualEntry) => record.intent?.sourceEntry === actualEntry ? record.intent.value : undefined;
assert.equal(visibleIntent(records[1], "entry-a-2"), "repair form");
assert.equal(visibleIntent(records[0], "entry-a-1"), undefined);
assert.equal(visibleIntent({ ...records[1], intent: { value: "guessed", sourceEntry: "other" } }, "entry-a-2"), undefined);

console.log("Synthetic authorization fixture passed: current owner boundary, visible explicitly authorized aggregate, recipient and role denial, revocation, expiry, retention, deletion, low-cohort policy, record-bound detail provenance, redaction, and source-bound optional intent.");
