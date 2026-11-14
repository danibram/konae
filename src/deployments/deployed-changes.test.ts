import { describe, expect, test } from "bun:test";
import { accessNotDeployed, changesNotDeployed, releasedAt, sameAsDeployed } from "./deployed-changes";
import { sourceDigest } from "./source-digest";
import { createDefaultManifest, withVisibility } from "./manifest";

const actions = '{\n  "version": 1,\n  "actions": {}\n}';
const project = (files: Record<string, string> = {}, visibility: "public" | "private" = "public") => ({
  id: "app_1",
  files: { "actions.json": actions, "index.html": "<p>one</p>", ...files },
  manifest: withVisibility(createDefaultManifest(), visibility),
});
const live = { manifest: withVisibility(createDefaultManifest(), "public"), actions, sourceDigest: await sourceDigest(project().files) };

describe("changes not deployed", () => {
  test("reads the release time, or the first publish from an older Kernel", () => {
    expect(releasedAt({ createdAt: 1, releasedAt: 5 })).toBe(5);
    expect(releasedAt({ createdAt: 1 })).toBe(1);
  });

  test("a draft has nothing to deploy against", () => {
    expect(changesNotDeployed(project(), null, null)).toBe(false);
    expect(sameAsDeployed(project(), null, null)).toBeNull();
  });

  test("an actions.json that differs from the release's is a change, as Export says", async () => {
    const changed = project({ "actions.json": actions.replace("{}", '{"a":{}}') });
    expect(accessNotDeployed(project(), live)).toBe(false);
    expect(accessNotDeployed(changed, live)).toBe(true);
    expect(changesNotDeployed(changed, live, await sourceDigest(changed.files))).toBe(true);
  });

  test("who can open the app and who can read uploads count", () => {
    expect(accessNotDeployed(project({}, "private"), live)).toBe(true);
    const shared = { ...project(), manifest: { ...project().manifest, fileAccess: "app-users" as const } };
    expect(accessNotDeployed(shared, live)).toBe(true);
    // Access differs, so the answer needs no digest.
    expect(changesNotDeployed(shared, live, null)).toBe(true);
  });

  test("other files compare against the digest the release names", async () => {
    expect(sameAsDeployed(project(), live, await sourceDigest(project().files))).toBe(true);
    const edited = project({ "index.html": "<p>two</p>" });
    expect(changesNotDeployed(edited, live, await sourceDigest(edited.files))).toBe(true);
    expect(accessNotDeployed(edited, live)).toBe(false);
    // A reverted edit is no change.
    expect(changesNotDeployed(project(), live, await sourceDigest({ ...project().files }))).toBe(false);
  });

  test("a release without a digest, or a digest not computed yet, says nothing", async () => {
    const edited = project({ "index.html": "<p>two</p>" });
    const { sourceDigest: _, ...old } = live;
    expect(sameAsDeployed(edited, old, await sourceDigest(edited.files))).toBeNull();
    expect(changesNotDeployed(edited, old, await sourceDigest(edited.files))).toBe(false);
    expect(sameAsDeployed(edited, live, null)).toBeNull();
  });
});
