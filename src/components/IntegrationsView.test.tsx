import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "bun:test";
import { ftpsWriteThenReadBack, type FtpsCredentialList } from "../deployments/client";
import { IntegrationsView } from "./IntegrationsView";

describe("FTPS integrations", () => {
  test("shows device credential controls only for a published app", () => {
    const html = renderToStaticMarkup(<IntegrationsView projectId="app" deploymentId="deployment" deploymentUrl="https://app.example" />);
    expect(html).toContain("FTPS device credentials");
    expect(html).toContain("Konae shows each password once.");
    expect(html).toContain("Loading FTPS credentials");
  });

  test("does not show device credential controls before publishing", () => {
    const html = renderToStaticMarkup(<IntegrationsView projectId="app" />);
    expect(html).not.toContain("FTPS device credentials");
  });

  test("keeps the one-time password before reading back the credential list", async () => {
    const order: string[] = [];
    const saved = { password: "one-time-secret" };
    const listed: FtpsCredentialList = { destinations: {}, credentials: [] };
    const result = await ftpsWriteThenReadBack(
      async () => { order.push("write"); return saved; },
      (value) => { expect(value).toBe(saved); order.push("retain-secret"); },
      async () => { order.push("read-back"); return listed; },
    );
    expect(order).toEqual(["write", "retain-secret", "read-back"]);
    expect(result).toBe(listed);
  });
});
