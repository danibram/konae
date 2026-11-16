import type { ReleaseFacts } from "../agent/release-check";
import { listSecrets } from "./client";
import { readDomain } from "./domains";

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
