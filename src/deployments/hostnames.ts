import { platformMcp } from "../mcp/client";

export type HostnameMove = {
  deploymentId: string;
  hostname: string;
  url: string;
  changed: boolean;
  previousHostnames: string[];
  googleCallback: string;
};


export const setHostname = (deploymentId: string, name: string, expected: string) => platformMcp.call<HostnameMove>("hostname_set", { projectId: deploymentId, name, expected });


export const hostnameLabel = (url: string) => new URL(url).hostname.split(".")[0] ?? "";
