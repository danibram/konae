import { describe, expect, test } from "bun:test";
import { readReleaseFacts } from "./release-facts";

const rules = { suffix: "apps.example.test", installationHosts: ["editor.example.test"] };

function reads(secrets: () => Promise<unknown>, domain: () => Promise<unknown>) {
  return { listSecrets: secrets, readDomain: domain } as unknown as Parameters<typeof readReleaseFacts>[2];
}

describe("readReleaseFacts", () => {
  test("names and use of the secrets, and the hosts once the app is published", async () => {
    const both = reads(async () => ({ projectId: "p1", secrets: [{ name: "orders_webhook", use: "route", updatedAt: 1 }] }), async () => ({ rules }));
    expect(await readReleaseFacts("p1", "d1", both)).toEqual({ secrets: [{ name: "orders_webhook", use: "route" }], rules });
  });

  test("a read that fails leaves only its own part out", async () => {
    const signedOut = reads(async () => { throw new Error("signed out"); }, async () => ({ rules }));
    expect(await readReleaseFacts("p1", "d1", signedOut)).toEqual({ rules });
    let asked = false;
    const unpublished = reads(async () => ({ projectId: "p1", secrets: [] }), async () => { asked = true; return { rules }; });
    expect(await readReleaseFacts("p1", undefined, unpublished)).toEqual({ secrets: [] });
    expect(asked).toBe(false);
  });
});
