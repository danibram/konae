import { buildProjectInBrowser } from "../../src/runtime/browser-build.ts";
import { cases, filesFor, settings } from "./cases.mjs";

globalThis.konaeSpike = async (packageText) => {
  const results = await Promise.all(Object.keys(cases).map(async (name) => {
    try {
      const artifact = await buildProjectInBrowser({
        ...settings, updatedAt: 0, directories: [], files: filesFor([name], packageText),
      });
      return [name, { bundle: artifact.serverArtifact.bundles[name] }];
    } catch (error) {
      const { message, code } = error.errors?.[0]?.detail ?? error;
      return [name, { message, code }];
    }
  }));
  return Object.fromEntries(results);
};

globalThis.runKonaeBundle = async (name, bundle) => {
  const url = URL.createObjectURL(new Blob([bundle], { type: "text/javascript" }));
  try {
    const { handle } = await import(/* @vite-ignore */ url);
    const response = await handle(new Request(location.origin + "/api/" + name));
    return { status: response.status, body: await response.json() };
  } finally {
    URL.revokeObjectURL(url);
  }
};
