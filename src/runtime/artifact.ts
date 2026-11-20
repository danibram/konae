import hostedRuntime from "../mcp/runtime.js?raw";
import { renderCapsuleDocument, type StaticArtifact } from "../lib/static-artifact.js";

export type BuildArtifact = StaticArtifact & { serverArtifact?: import("../kernel/functions").ServerArtifact };

export { renderCapsuleDocument, runtimeScriptAttribute } from "../lib/static-artifact.js";
export { hostedRuntime };

export function deploymentHead(deploymentId?: string): string {
  return deploymentId ? `<meta name="konae-deployment" content="${escapeHtml(deploymentId)}">` : "";
}

export function createHostedArtifact(input: {
  body: string;
  styles: string;
  script: string;
  deploymentId?: string;
  sdk?: boolean;
}): BuildArtifact {
  const deploymentMeta = deploymentHead(input.deploymentId);
  return {
    version: 1,
    entrypoint: "index.html",
    files: {
      "index.html": renderCapsuleDocument({ ...input, head: deploymentMeta, runtime: input.sdk === false ? "" : hostedRuntime }),
    },
  };
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[character] ?? character);
}
