import { describe, expect, test } from "bun:test";
import { parseRoute, pushRoute, routePath } from "./routing";

describe("routing", () => {
  test("reads a project out of the path", () => {
    expect(parseRoute("/app/abc-123")).toEqual({ screen: "editor", projectId: "abc-123" });
    expect(parseRoute("/app/abc-123/")).toEqual({ screen: "editor", projectId: "abc-123" });
    expect(parseRoute("/app/abc%20123")).toEqual({ screen: "editor", projectId: "abc 123" });
  });

  test("the root is the project list", () => {
    for (const path of ["/", ""]) expect(parseRoute(path)).toEqual({ screen: "projects" });
  });

  test("an address that names nothing is not found, not the project list", () => {
    expect(parseRoute("/nope")).toEqual({ screen: "not-found", what: "page", path: "/nope" });
    expect(parseRoute("/spaces/x")).toEqual({ screen: "not-found", what: "page", path: "/spaces/x" });
    for (const path of ["/app", "/app/"]) expect(parseRoute(path)).toEqual({ screen: "not-found", what: "project", path });
    expect(parseRoute("/app/%E0%A4%A")).toEqual({ screen: "not-found", what: "project", path: "/app/%E0%A4%A" });
  });

  test("round-trips a route through its path", () => {
    const route = { screen: "editor", projectId: "a b/c" } as const;
    expect(parseRoute(routePath(route))).toEqual({ screen: "editor", projectId: "a b/c" });
    expect(routePath({ screen: "projects" })).toBe("/");
  });

  test("does not push a history entry for the path already shown", () => {
    const calls: string[] = [];
    const history = { pushState: (_state: unknown, _title: string, url?: string | URL | null) => calls.push(String(url)) };
    pushRoute({ screen: "editor", projectId: "x" }, history);
    expect(calls).toEqual(["/app/x"]);
  });
});
