import { anonymousActor, authorizeKernelIntent, type KernelDecision, type KernelIntent } from "./access";
import type { KernelIdentity } from "./auth";

export class KonaeKernel {
  constructor(private readonly identity: KernelIdentity) {}

  async authorize(sessionToken: string | null | undefined, intent: KernelIntent): Promise<KernelDecision> {
    const actor = await this.identity.authenticate(sessionToken) ?? anonymousActor;
    return authorizeKernelIntent(actor, intent);
  }
}
