/**
 * The address bar is the workshop's state, not decoration: a reload should put
 * the owner back in the project they were editing instead of the project list.
 */
export type Route =
  | { screen: "projects" }
  | { screen: "editor"; projectId: string };

export function parseRoute(pathname: string): Route {
  const match = /^\/app\/([^/?#]+)/.exec(pathname);
  if (!match) return { screen: "projects" };
  const projectId = decodeURIComponent(match[1]);
  return projectId ? { screen: "editor", projectId } : { screen: "projects" };
}

export function routePath(route: Route): string {
  return route.screen === "editor" ? `/app/${encodeURIComponent(route.projectId)}` : "/";
}

/** Writes the route without reloading; identical paths are left alone so the history stays clean. */
export function pushRoute(route: Route, history: Pick<History, "pushState"> = window.history): void {
  const path = routePath(route);
  if (typeof location !== "undefined" && location.pathname === path) return;
  history.pushState({}, "", path);
}
