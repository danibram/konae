import { McpClient, platformMcp } from "../mcp/client";
import type { MiniAppProject } from "./types";
import type { DeploymentReceipt } from "../deployments/client";
import { repairDroppedPreviewUpload } from "./starter";
import { checkSourceFiles } from "./source-rules";
import { checkSourceUpload } from "./size";
import { isEncoded } from "./source-files";

export type ProjectMetadata = { id: string; name: string; slug: string; manifest?: MiniAppProject["manifest"]; directories?: string[]; updatedAt?: number };
type SourceResult = { revision: number; transfer: Parameters<McpClient["download"]>[0] | null; project?: ProjectMetadata };
export type ReloadedSources = { revision: number; project: MiniAppProject | null; serverSnapshot?: string };
export class SaveSupersededError extends Error { constructor() { super("This save was replaced by a newer draft before it reached the server."); } }

type SaveJob = { project: MiniAppProject; snapshot: string; epoch: number; resolve: (revision: number) => void; reject: (error: unknown) => void };

/** How many Kernel projects the signed-in owner has, including ones without editor sources. */
export async function readOwnerProjectCount(ownerId?: string): Promise<number> {
  const client = ownerId ? new McpClient("/mcp", { ownerId }) : platformMcp;
  const result = await client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
  return result.projects.length;
}

/** One instance remains bound to the owner that opened the workspace. */
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

  constructor(ownerId: string, endpoint = "/mcp") { this.client = new McpClient(endpoint, { ownerId }); }

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

  /** Loads project cards and deployment summaries without reading app/ files. */
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
    try {
      // This call may create a download before its response arrives. Keep the
      // request alive until the transfer ID is known, then cancel through download().
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId });
      this.revisions.set(projectId, saved.revision);
      if (!saved.transfer) { activeSignal.throwIfAborted(); this.sourcesExist.delete(projectId); return null; }
      const bytes = await this.client.download(saved.transfer, activeSignal);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== projectId) throw new Error("The saved source does not match its project.");
      const repaired = repairDroppedPreviewUpload(source);
      this.documents.set(projectId, repaired);
      this.sourcesExist.add(projectId);
      this.snapshots.set(projectId, JSON.stringify(source));
      return repaired;
    } finally {
      active.delete(controller);
      if (!active.size) this.downloads.delete(projectId);
    }
  }

  async inspectRevision(projectId: string) {
    const inspected = await this.client.call<{ sourcesRevision: number }>("project_inspect", { projectId });
    return inspected.sourcesRevision;
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
    const pending = this.pending.get(projectId);
    if (pending) { this.pending.delete(projectId); pending.reject(new Error("This save was canceled by a source reload.")); }
    this.controllers.get(projectId)?.abort();
    try {
      while (this.running.has(projectId)) await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId });
      if (!saved.transfer) { controller.signal.throwIfAborted(); return { revision: saved.revision, project: null }; }
      const bytes = await this.client.download(saved.transfer, controller.signal);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== projectId) throw new Error("The saved source does not match its project.");
      return { revision: saved.revision, project: repairDroppedPreviewUpload(source), serverSnapshot: JSON.stringify(source) };
    } catch (error) {
      this.reloading.delete(projectId);
      throw error;
    } finally {
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

  /** Queues one save per project and replaces only snapshots that have not started. */
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

  private async pump(projectId: string): Promise<void> {
    if (this.running.has(projectId)) return;
    this.running.add(projectId);
    try {
      while (this.pending.has(projectId)) {
        const job = this.pending.get(projectId)!;
        this.pending.delete(projectId);
        try {
          if (this.snapshots.get(projectId) === job.snapshot && this.revisions.has(projectId)) job.resolve(this.revisions.get(projectId)!);
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
      const revision = this.revisions.get(projectId);
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
      let payload: unknown = project;
      if (this.sourcesExist.has(projectId) && this.snapshots.has(projectId)) payload = this.delta(JSON.parse(this.snapshots.get(projectId)!), project);
      const bytes = new TextEncoder().encode(JSON.stringify(payload));
      checkSourceUpload(bytes.length);
      const upload = await this.client.upload(bytes, projectId, undefined, controller.signal);
      try {
        checkEpoch();
        const result = await this.client.call<{ revision: number }>("project_sources_save", { projectId, transferId: upload.transferId, expectedRevision: revision }, controller.signal);
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

  private delta(previous: MiniAppProject, next: MiniAppProject) {
    const files: Record<string, string> = {};
    const encodings: Record<string, "base64"> = {};
    for (const [path, contents] of Object.entries(next.files)) {
      const wasEncoded = isEncoded(previous, path);
      const nowEncoded = isEncoded(next, path);
      if (previous.files[path] === contents && wasEncoded === nowEncoded) continue;
      files[path] = contents;
      if (nowEncoded) encodings[path] = "base64";
    }
    const removedPaths = Object.keys(previous.files).filter((path) => !(path in next.files));
    return { format: "delta", metadata: { name: next.name, slug: next.slug, directories: next.directories, manifest: next.manifest, updatedAt: next.updatedAt }, files, ...(Object.keys(encodings).length ? { encodings } : {}), removedPaths };
  }

  private commitSnapshot(projectId: string, job: SaveJob, revision: number) {
    this.revisions.set(projectId, revision);
    this.snapshots.set(projectId, job.snapshot);
    this.documents.set(projectId, job.project);
  }

  /** Aborts active work and rejects a queued snapshot. Cancellation reaches the owner transfer endpoint. */
  cancelProject(projectId: string) {
    const pending = this.pending.get(projectId);
    if (pending) { this.pending.delete(projectId); pending.reject(new Error("This queued save was canceled because the editor is closing.")); }
    this.controllers.get(projectId)?.abort();
    for (const controller of this.downloads.get(projectId) ?? []) controller.abort();
  }

  cancelAll() { for (const projectId of new Set([...this.pending.keys(), ...this.controllers.keys(), ...this.downloads.keys()])) this.cancelProject(projectId); }

  /** Deletes a project after its current save finishes, then confirms it disappeared. */
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
