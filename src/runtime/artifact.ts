export type BuildArtifact = {
  version: 1;
  entrypoint: "index.html";
  files: Record<string, string>;
};

import hostedRuntime from "../mcp/runtime.js?raw";

/** The attribute that marks the SDK script, so tools that rebuild a document can find it. */
export const runtimeScriptAttribute = "data-littlebox-runtime";

export function createHostedArtifact(input: {
  body: string;
  styles: string;
  script: string;
  deploymentId?: string;
}): BuildArtifact {
  const deploymentMeta = input.deploymentId
    ? `<meta name="littlebox-deployment" content="${escapeHtml(input.deploymentId)}">`
    : "";
  return {
    version: 1,
    entrypoint: "index.html",
    files: {
      "index.html": renderCapsuleDocument({ ...input, head: deploymentMeta, runtime: hostedRuntime }),
    },
  };
}

/**
 * The one document shape a Capsule runs in, shared by the preview and every
 * Deployment. The compiled module is emitted as is: esbuild output may end in a
 * top-level `export`, which is only legal at module scope, and a startup throw
 * must reach the runtime's window error reporting instead of a local catch.
 */
export function renderCapsuleDocument(input: {
  body: string;
  styles: string;
  script: string;
  runtime: string;
  head?: string;
}): string {
  return `<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${input.head ?? ""}<style>${input.styles}</style></head>
<body>${input.body}<script ${runtimeScriptAttribute}>${escapeInlineScript(input.runtime)}</script><script type="module">${escapeInlineScript(input.script)}</script></body></html>`;
}

function escapeInlineScript(value: string) {
  return value.replaceAll("</script", "<\\/script");
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[character] ?? character);
}
