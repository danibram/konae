import { describe, expect, test } from "bun:test";
import { parseRoute, pushRoute, routePath } from "./routing";

describe("routing", () => {
  test("reads a project out of the path", () => {
    expect(parseRoute("/app/abc-123")).toEqual({ screen: "editor", projectId: "abc-123" });
    expect(parseRoute("/app/abc-123/")).toEqual({ screen: "editor", projectId: "abc-123" });
    expect(parseRoute("/app/abc%20123")).toEqual({ screen: "editor", projectId: "abc 123" });
  });

  test("anything else is the project list", () => {
    for (const path of ["/", "/app", "/app/", "/spaces/x", ""]) {
      expect(parseRoute(path)).toEqual({ screen: "projects" });
    }
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
