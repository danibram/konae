import { describe, expect, test } from "bun:test";
import { McpError } from "../mcp/client";
import { isSharedOriginRefusal, pathAddressLabel, pathNeedsDomain, type PathView } from "./paths";

const view = (overrides: Partial<PathView> = {}): PathView => ({
  deploymentId: "deployment-1",
  enabled: false,
  name: null,
  acceptSharedOrigin: false,
  addresses: [],
  domainConfigured: true,
  ...overrides,
});

describe("path address state", () => {
  test("the installation without a domain shows no switch, whether the path is on or off", () => {
    expect(pathNeedsDomain(view({ domainConfigured: false }))).toBe(true);
    expect(pathNeedsDomain(view({ domainConfigured: false, enabled: true }))).toBe(true);
    expect(pathNeedsDomain(view())).toBe(false);
  });

  test("a shared-origin refusal is recognised by its code and no other", () => {
    const refusal = (code: string) => new McpError("message", 409, { code, message: "message" });
    expect(isSharedOriginRefusal(refusal("path.shared-origin"))).toBe(true);
    expect(isSharedOriginRefusal(refusal("path.taken"))).toBe(false);
    expect(isSharedOriginRefusal(new Error("path.shared-origin"))).toBe(false);
    expect(isSharedOriginRefusal(new McpError("message", 401))).toBe(false);
  });

  test("the id address is labelled first and the name second", () => {
    expect([0, 1].map(pathAddressLabel)).toEqual(["By id", "By name"]);
  });
});
