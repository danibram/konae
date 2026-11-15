import { MiniAppRuntimeHost } from "./host";

export type PreviewSource = {
  schema: string;
  actions: string;
  fileAccess?: "app-users";
};

/** Keeps the applied preview alive while each proposal gets a disposable copy. */
export class ProposalHostLifecycle {
  readonly appliedHost: MiniAppRuntimeHost;
  private proposal: { id: string; host: MiniAppRuntimeHost } | null = null;
  private pendingId: string | null = null;
  private generation = 0;
  private disposalGeneration = 0;
  private disposed = false;

  constructor(appId: string, source: PreviewSource) {
    this.appliedHost = new MiniAppRuntimeHost(appId, source.schema, source.actions, { fileAccess: source.fileAccess });
  }

  get proposalHost() {
    return this.proposal?.host ?? null;
  }

  isProposalReady(id: string) {
    return this.proposal?.id === id;
  }

  async prepareProposal(id: string, source: PreviewSource): Promise<MiniAppRuntimeHost | null> {
    if (this.disposed) return null;
    await this.discardProposal();
    const generation = ++this.generation;
    this.pendingId = id;
    let host: MiniAppRuntimeHost | null = null;
    try {
      host = await this.appliedHost.cloneForProposal(id, source.schema, source.actions, source.fileAccess);
      if (this.disposed || generation !== this.generation) {
        await host.dispose(true);
        return null;
      }
      this.proposal = { id, host };
      return host;
    } catch (error) {
      if (host) await host.dispose(true);
      throw error;
    } finally {
      if (generation === this.generation) this.pendingId = null;
    }
  }

  async discardProposal(id?: string) {
    if (id && this.proposal?.id !== id && this.pendingId !== id) return;
    this.generation += 1;
    this.pendingId = null;
    const proposal = this.proposal;
    this.proposal = null;
    await proposal?.host.dispose(true);
  }

  async applyProposal(id: string, source: PreviewSource) {
    if (!this.isProposalReady(id) || this.disposed) return false;
    await this.discardProposal(id);
    this.appliedHost.setFileAccess(source.fileAccess);
    this.appliedHost.setActions(source.actions);
    await this.appliedHost.setSchema(source.schema);
    return true;
  }

  async updateAppliedSource(source: PreviewSource) {
    if (this.disposed) return;
    this.appliedHost.setFileAccess(source.fileAccess);
    this.appliedHost.setActions(source.actions);
    await this.appliedHost.setSchema(source.schema);
  }

  /** React StrictMode replays effect cleanup and setup; defer release one microtask. */
  retain() {
    this.disposalGeneration += 1;
    return () => {
      const generation = ++this.disposalGeneration;
      queueMicrotask(() => {
        if (generation === this.disposalGeneration) void this.dispose();
      });
    };
  }

  async dispose() {
    if (this.disposed) return;
    this.disposed = true;
    await this.discardProposal();
    await this.appliedHost.dispose();
  }
}
