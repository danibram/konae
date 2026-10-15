import type { ReleaseFacts } from "../agent/release-check";
import { listSecrets } from "./client";
import { readDomain } from "./domains";

/**
 * What release_plan reads that the editor can read too, for the build agent's
 * Gate: the project's secret names (never values) and, once the app is
 * published, the installation's hosts. A read that fails leaves its part out,
 * and the Gate stays silent on it, as the Deploy dialog does.
 */
export async function readReleaseFacts(
  projectId: string,
  deploymentId: string | undefined,
  reads: { listSecrets: typeof listSecrets; readDomain: typeof readDomain } = { listSecrets, readDomain },
): Promise<ReleaseFacts> {
  const [secrets, domain] = await Promise.allSettled([
    reads.listSecrets(projectId),
    deploymentId ? reads.readDomain(deploymentId) : Promise.reject(new Error("not published")),
  ]);
  return {
    ...(secrets.status === "fulfilled"
      ? { secrets: secrets.value.secrets.map((secret) => ({ name: secret.name, use: (secret as { use?: string }).use })) }
      : {}),
    ...(domain.status === "fulfilled" && domain.value.rules ? { rules: domain.value.rules } : {}),
  };
}
