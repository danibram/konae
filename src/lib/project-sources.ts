import { McpClient, platformMcp, type AgentExecution } from "../mcp/client";
import type { MiniAppProject } from "./types";
import type { DeploymentReceipt } from "../deployments/client";
import { checkSourceFiles } from "./source-rules";
import { checkSourceUpload } from "./size";
import { isEncoded } from "./source-files";

export type ProjectMetadata = { id: string; name: string; slug: string; manifest?: MiniAppProject["manifest"]; directories?: string[]; updatedAt?: number };
type SourceResult = { revision: number; transfer: Parameters<McpClient["download"]>[0] | null; project?: ProjectMetadata; cancelled?: boolean };
export type ReloadedSources = { revision: number; project: MiniAppProject | null; serverSnapshot?: string };
export class SaveSupersededError extends Error { constructor() { super("This save was replaced by a newer draft before it reached the server."); } }

export function sourceSavePayload(previous: MiniAppProject | undefined, next: MiniAppProject, cachedRevision: number | undefined, expectedRevision: number | undefined, hasSavedSources: boolean): unknown {
  if (!previous || !hasSavedSources || (expectedRevision !== undefined && cachedRevision !== expectedRevision)) return next;
  const files: Record<string, string> = Object.create(null);
  const encodings: Record<string, "base64"> = Object.create(null);
  for (const [path, contents] of Object.entries(next.files)) {
    const wasEncoded = isEncoded(previous, path as never);
    const nowEncoded = isEncoded(next, path as never);
    if (previous.files[path as never] === contents && wasEncoded === nowEncoded) continue;
    files[path] = contents;
    if (nowEncoded) encodings[path] = "base64";
  }
  const removedPaths = Object.keys(previous.files).filter((path) => !(path in next.files));
  return { format: "delta", metadata: { name: next.name, slug: next.slug, directories: next.directories, manifest: next.manifest, updatedAt: next.updatedAt }, files, ...(Object.keys(encodings).length ? { encodings } : {}), removedPaths };
}

type SaveJob = { project: MiniAppProject; snapshot: string; epoch: number; expectedRevision?: number; execution?: AgentExecution; resolve: (revision: number) => void; reject: (error: unknown) => void };

export async function readOwnerProjectCount(ownerId?: string): Promise<number> {
  const client = ownerId ? new McpClient("/mcp", { ownerId }) : platformMcp;
  const result = await client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
  return result.projects.length;
}

export class ProjectSources {
  private readonly client: McpClient;
  private readonly revisions = new Map<string, number>();
  private readonly snapshots = new Map<string, string>();
  private readonly documents = new Map<string, MiniAppProject>();
  private readonly epochs = new Map<string, number>();
  private readonly reloading = new Set<string>();
  private readonly pending = new Map<string, SaveJob>();
  private readonly running = new Set<string>();
  private readonly controllers = new Map<string, AbortController>();
  private readonly downloads = new Map<string, Set<AbortController>>();
  private readonly sourcesExist = new Set<string>();

  constructor(ownerId: string, endpoint = "/mcp", fetcher?: typeof fetch) { this.client = new McpClient(endpoint, { ownerId, fetch: fetcher }); }

  revision(projectId: string) { return this.revisions.get(projectId); }
  isLoaded(projectId: string) { return this.documents.has(projectId); }
  hasSources(projectId: string) { return this.sourcesExist.has(projectId); }
  document(projectId: string) { return this.documents.get(projectId); }

  markPublished(projectId: string, revision: number, project?: MiniAppProject) {
    this.revisions.set(projectId, revision);
    if (project) {
      const snapshot = JSON.stringify(project);
      this.snapshots.set(projectId, snapshot);
      this.documents.set(projectId, project);
      this.sourcesExist.add(projectId);
    }
  }

  async load() {
    const result = await this.client.call<{ projects: { project: { id: string; name: string; slug: string } }[] }>("projects_list");
    const missing: ProjectMetadata[] = [];
    const projects: MiniAppProject[] = [];
    const deployments: Record<string, DeploymentReceipt> = {};
    for (const { project } of result.projects) {
      const inspected = await this.client.call<{ deployment: DeploymentReceipt | null }>("project_inspect", { projectId: project.id });
      if (inspected.deployment) deployments[project.id] = inspected.deployment;
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId: project.id, metadataOnly: true });
      this.revisions.set(project.id, saved.revision);
      if (!saved.project) { missing.push({ ...project, manifest: { version: 1, visibility: "private" } as MiniAppProject["manifest"] }); continue; }
      this.sourcesExist.add(project.id);
      const stub: MiniAppProject = {
        id: saved.project.id, name: saved.project.name, slug: saved.project.slug,
        files: {}, encodings: {}, directories: saved.project.directories ?? [],
        manifest: saved.project.manifest ?? ({ version: 1, visibility: "private" } as MiniAppProject["manifest"]), updatedAt: saved.project.updatedAt ?? 0,
      };
      projects.push(stub);
    }
    return { projects, missing, deployments };
  }

  async loadProject(projectId: string, signal?: AbortSignal): Promise<MiniAppProject | null> {
    const cached = this.documents.get(projectId);
    if (cached) return cached;
    const controller = new AbortController();
    const active = this.downloads.get(projectId) ?? new Set<AbortController>();
    active.add(controller);
    this.downloads.set(projectId, active);
    const activeSignal = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
    const requestId = crypto.randomUUID();
    const cancelRead = () => { void this.client.cancelSourceRead(projectId, requestId).catch(() => {}); };
    activeSignal.addEventListener("abort", cancelRead, { once: true });
    try {
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId, requestId });
      if (activeSignal.aborted && saved.transfer) await this.client.cancel(saved.transfer.transferId).catch(() => {});
      activeSignal.throwIfAborted();
      if (saved.cancelled) throw new Error("This project source read was canceled.");
      this.revisions.set(projectId, saved.revision);
      if (!saved.transfer) { this.sourcesExist.delete(projectId); return null; }
      const bytes = await this.client.download(saved.transfer, activeSignal);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== projectId) throw new Error("The saved source does not match its project.");
      this.documents.set(projectId, source);
      this.sourcesExist.add(projectId);
      this.snapshots.set(projectId, JSON.stringify(source));
      return source;
    } finally {
      activeSignal.removeEventListener("abort", cancelRead);
      active.delete(controller);
      if (!active.size) this.downloads.delete(projectId);
    }
  }

  async inspectRevision(projectId: string) {
    const inspected = await this.client.call<{ sourcesRevision: number }>("project_inspect", { projectId });
    return inspected.sourcesRevision;
  }

  /** Read the current saved sources without changing this instance's cache or canceling queued saves. */
  async observeSaved(projectId: string, execution?: AgentExecution): Promise<{ revision: number; project: MiniAppProject | null }> {
    const requestId = crypto.randomUUID();
    const saved = await (execution ? this.client.callForAgent<SourceResult>("project_sources_read", { projectId, requestId }, execution) : this.client.call<SourceResult>("project_sources_read", { projectId, requestId }));
    try {
      if (!saved.transfer) return { revision: saved.revision, project: null };
      const bytes = await this.client.download(saved.transfer);
      const project = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (project.id !== projectId) throw new Error("The saved source does not match its project.");
      return { revision: saved.revision, project };
    } finally { if (saved.transfer) await this.client.cancel(saved.transfer.transferId).catch(() => {}); }
  }

  async reload(projectId: string): Promise<ReloadedSources> {
    if (this.reloading.has(projectId)) throw new Error("Saved sources are already loading.");
    const epoch = (this.epochs.get(projectId) ?? 0) + 1;
    this.epochs.set(projectId, epoch);
    this.reloading.add(projectId);
    const controller = new AbortController();
    const active = this.downloads.get(projectId) ?? new Set<AbortController>();
    active.add(controller);
    this.downloads.set(projectId, active);
    const requestId = crypto.randomUUID();
    const cancelRead = () => { void this.client.cancelSourceRead(projectId, requestId).catch(() => {}); };
    controller.signal.addEventListener("abort", cancelRead, { once: true });
    const pending = this.pending.get(projectId);
    if (pending) { this.pending.delete(projectId); pending.reject(new Error("This save was canceled by a source reload.")); }
    this.controllers.get(projectId)?.abort();
    try {
      while (this.running.has(projectId)) await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId, requestId });
      if (controller.signal.aborted && saved.transfer) await this.client.cancel(saved.transfer.transferId).catch(() => {});
      controller.signal.throwIfAborted();
      if (saved.cancelled) throw new Error("This project source read was canceled.");
      if (!saved.transfer) { controller.signal.throwIfAborted(); return { revision: saved.revision, project: null }; }
      const bytes = await this.client.download(saved.transfer, controller.signal);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== projectId) throw new Error("The saved source does not match its project.");
      return { revision: saved.revision, project: source, serverSnapshot: JSON.stringify(source) };
    } catch (error) {
      this.reloading.delete(projectId);
      throw error;
    } finally {
      controller.signal.removeEventListener("abort", cancelRead);
      active.delete(controller);
      if (!active.size) this.downloads.delete(projectId);
    }
  }

  acceptReload(projectId: string, saved: ReloadedSources) {
    if (!this.reloading.has(projectId)) throw new Error("The source reload is no longer active.");
    this.revisions.set(projectId, saved.revision);
    if (saved.project) {
      this.snapshots.set(projectId, saved.serverSnapshot ?? JSON.stringify(saved.project));
      this.documents.set(projectId, saved.project);
      this.sourcesExist.add(projectId);
    } else {
      this.snapshots.delete(projectId);
      this.documents.delete(projectId);
      this.sourcesExist.delete(projectId);
    }
    this.reloading.delete(projectId);
  }

  cancelReload(projectId: string) { this.reloading.delete(projectId); }

  save(project: MiniAppProject): Promise<number> {
    if (this.reloading.has(project.id)) return Promise.reject(new Error("Saved sources are loading. Resolve the source conflict before saving this draft."));
    const snapshot = JSON.stringify(project);
    const epoch = this.epochs.get(project.id) ?? 0;
    if (this.snapshots.get(project.id) === snapshot && this.revisions.has(project.id) && !this.running.has(project.id) && !this.pending.has(project.id)) return Promise.resolve(this.revisions.get(project.id)!);
    try {
      checkSourceFiles(project);
      const bytes = new TextEncoder().encode(snapshot);
      checkSourceUpload(bytes.length);
    } catch (error) {
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const obsolete = this.pending.get(project.id);
      if (obsolete) obsolete.reject(new SaveSupersededError());
      this.pending.set(project.id, { project, snapshot, epoch, resolve, reject });
      void this.pump(project.id);
    });
  }

  /** Save a recovered proposal against the revision it was validated against. */
  saveExpected(project: MiniAppProject, expectedRevision: number, execution?: AgentExecution): Promise<number> {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 0) return Promise.reject(new Error("The proposal's saved source revision is invalid."));
    if (this.reloading.has(project.id)) return Promise.reject(new Error("Saved sources are loading. Resolve the source conflict before saving this draft."));
    if (this.snapshots.get(project.id) === JSON.stringify(project) && this.revisions.has(project.id) && !this.running.has(project.id) && !this.pending.has(project.id)) return Promise.resolve(this.revisions.get(project.id)!);
    try { checkSourceFiles(project); checkSourceUpload(new TextEncoder().encode(JSON.stringify(project)).length); }
    catch (error) { return Promise.reject(error); }
    const snapshot = JSON.stringify(project);
    const epoch = this.epochs.get(project.id) ?? 0;
    return new Promise((resolve, reject) => {
      const obsolete = this.pending.get(project.id);
      if (obsolete && obsolete.expectedRevision === undefined) {
        reject(new Error("A hand-edited source save is already queued. Keep that draft and revalidate the recovered proposal against its saved result."));
        return;
      }
      if (obsolete) obsolete.reject(new SaveSupersededError());
      this.pending.set(project.id, { project, snapshot, epoch, expectedRevision, execution, resolve, reject });
      void this.pump(project.id);
    });
  }

  private async pump(projectId: string): Promise<void> {
    if (this.running.has(projectId)) return;
    this.running.add(projectId);
    try {
      while (this.pending.has(projectId)) {
        const job = this.pending.get(projectId)!;
        this.pending.delete(projectId);
        try {
          if (job.expectedRevision === undefined && this.snapshots.get(projectId) === job.snapshot && this.revisions.has(projectId)) job.resolve(this.revisions.get(projectId)!);
          else job.resolve(await this.saveOne(projectId, job));
        }
        catch (error) { job.reject(error); }
      }
    } finally {
      this.running.delete(projectId);
      if (this.pending.has(projectId)) void this.pump(projectId);
    }
  }

  private async saveOne(projectId: string, job: SaveJob): Promise<number> {
    const checkEpoch = () => {
      if (job.epoch !== (this.epochs.get(projectId) ?? 0) || this.reloading.has(projectId)) throw new Error("This save was superseded by a source reload.");
    };
    checkEpoch();
    const controller = new AbortController();
    this.controllers.set(projectId, controller);
    const project = job.project;
    try {
      const revision = job.expectedRevision ?? this.revisions.get(projectId);
      if (revision === undefined) {
        const bytes = new TextEncoder().encode(job.snapshot);
        const upload = await this.client.upload(bytes, undefined, undefined, controller.signal);
        try {
          checkEpoch();
          const created = await this.client.call<ProjectMetadata & { sourcesRevision: number }>("project_create", { projectId, name: project.name, slug: project.slug, manifest: project.manifest, sourcesTransferId: upload.transferId }, controller.signal);
          checkEpoch();
          this.commitSnapshot(projectId, job, created.sourcesRevision);
          this.sourcesExist.add(projectId);
          return created.sourcesRevision;
        } finally { await this.client.cancel(upload.transferId).catch(() => {}); }
      }
      const cachedRevision = this.revisions.get(projectId);
      const previous = this.snapshots.has(projectId) ? JSON.parse(this.snapshots.get(projectId)!) as MiniAppProject : undefined;
      const payload = sourceSavePayload(previous, project, cachedRevision, job.expectedRevision, this.sourcesExist.has(projectId));
      const bytes = new TextEncoder().encode(JSON.stringify(payload));
      checkSourceUpload(bytes.length);
      const upload = await this.client.upload(bytes, projectId, undefined, controller.signal);
      try {
        checkEpoch();
        const saveArgs = { projectId, transferId: upload.transferId, expectedRevision: revision };
        const result = await (job.execution ? this.client.callForAgent<{ revision: number }>("project_sources_save", saveArgs, job.execution, controller.signal) : this.client.call<{ revision: number }>("project_sources_save", saveArgs, controller.signal));
        checkEpoch();
        this.commitSnapshot(projectId, job, result.revision);
        this.sourcesExist.add(projectId);
        return result.revision;
      } finally { await this.client.cancel(upload.transferId).catch(() => {}); }
    } catch (error) {
      if (job.epoch !== (this.epochs.get(projectId) ?? 0) || this.reloading.has(projectId)) throw new Error("This save was superseded by a source reload.");
      throw error;
    } finally {
      if (this.controllers.get(projectId) === controller) this.controllers.delete(projectId);
    }
  }

  private commitSnapshot(projectId: string, job: SaveJob, revision: number) {
    this.revisions.set(projectId, revision);
    this.snapshots.set(projectId, job.snapshot);
    this.documents.set(projectId, job.project);
  }

  cancelProject(projectId: string) {
    const pending = this.pending.get(projectId);
    if (pending) { this.pending.delete(projectId); pending.reject(new Error("This queued save was canceled because the editor is closing.")); }
    this.controllers.get(projectId)?.abort();
    for (const controller of this.downloads.get(projectId) ?? []) controller.abort();
  }

  cancelAll() { for (const projectId of new Set([...this.pending.keys(), ...this.controllers.keys(), ...this.downloads.keys()])) this.cancelProject(projectId); }

  async deleteProject(projectId: string, name: string): Promise<void> {
    this.cancelProject(projectId);
    while (this.running.has(projectId)) await new Promise((resolve) => setTimeout(resolve, 0));
    await this.client.call("project_delete", { projectId, name });
    const listed = await this.client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
    if (listed.projects.some(({ project }) => project.id === projectId)) throw new Error("The Kernel still lists this project. Reload and try again.");
    this.revisions.delete(projectId); this.snapshots.delete(projectId); this.documents.delete(projectId); this.sourcesExist.delete(projectId);
  }

  retry(projectId: string) { this.cancelProject(projectId); }
}
