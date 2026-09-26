import hostedRuntime from "../mcp/runtime.js?raw";
import { renderCapsuleDocument, type StaticArtifact } from "../lib/static-artifact.js";

export type BuildArtifact = StaticArtifact;

export { renderCapsuleDocument, runtimeScriptAttribute } from "../lib/static-artifact.js";
export { hostedRuntime };

/** The `<meta>` that names the Deployment in the head of every page the editor builds. */
export function deploymentHead(deploymentId?: string): string {
  return deploymentId ? `<meta name="littlebox-deployment" content="${escapeHtml(deploymentId)}">` : "";
}

export function createHostedArtifact(input: {
  body: string;
  styles: string;
  script: string;
  deploymentId?: string;
}): BuildArtifact {
  const deploymentMeta = deploymentHead(input.deploymentId);
  return {
    version: 1,
    entrypoint: "index.html",
    files: {
      "index.html": renderCapsuleDocument({ ...input, head: deploymentMeta, runtime: hostedRuntime }),
    },
  };
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[character] ?? character);
}
