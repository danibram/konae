/**
 * The address bar is the workshop's state, not decoration: a reload should put
 * the owner back in the project they were editing instead of the project list.
 */
export type Route =
  | { screen: "projects" }
  | { screen: "editor"; projectId: string }
  /** An address that names nothing here. The page says so instead of showing the list under a wrong address. */
  | { screen: "not-found"; what: "page" | "project"; path: string };

export function parseRoute(pathname: string): Route {
  if (pathname === "/" || pathname === "") return { screen: "projects" };
  const match = /^\/app\/([^/?#]+)/.exec(pathname);
  if (!match) return { screen: "not-found", what: /^\/app\/?$/.test(pathname) ? "project" : "page", path: pathname };
  let projectId = "";
  try { projectId = decodeURIComponent(match[1]); } catch { /* a malformed escape names no project */ }
  return projectId ? { screen: "editor", projectId } : { screen: "not-found", what: "project", path: pathname };
}

export function routePath(route: Route): string {
  if (route.screen === "not-found") return route.path;
  return route.screen === "editor" ? `/app/${encodeURIComponent(route.projectId)}` : "/";
}

/** Writes the route without reloading; identical paths are left alone so the history stays clean. */
export function pushRoute(route: Route, history: Pick<History, "pushState"> = window.history): void {
  const path = routePath(route);
  if (typeof location !== "undefined" && location.pathname === path) return;
  history.pushState({}, "", path);
}
