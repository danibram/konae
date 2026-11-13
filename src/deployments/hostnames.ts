import { platformMcp } from "../mcp/client";

export type HostnameMove = {
  deploymentId: string;
  hostname: string;
  url: string;
  changed: boolean;
  previousHostnames: string[];
  googleCallback: string;
};

/** Moves the app's generated hostname to `name`, one label under the installation's suffix. The Kernel checks the name. */
export const setHostname = (deploymentId: string, name: string) => platformMcp.call<HostnameMove>("hostname_set", { deploymentId, name });

/** The label of a generated hostname under its suffix: notes for https://notes.apps.example.com. */
export const hostnameLabel = (url: string) => new URL(url).hostname.split(".")[0] ?? "";
