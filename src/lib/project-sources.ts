import { McpClient, platformMcp } from "../mcp/client";
import type { MiniAppProject } from "./types";
import type { DeploymentReceipt } from "../deployments/client";
import { repairDroppedPreviewUpload } from "./starter";
import { checkSourceFiles } from "./source-rules";
import { checkSourceUpload } from "./size";

export type ProjectMetadata = { id: string; name: string; slug: string };
type SourceResult = { revision: number; transfer: Parameters<McpClient["download"]>[0] | null };
export type ReloadedSources = { revision: number; project: MiniAppProject | null };

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
  private readonly tails = new Map<string, Promise<number>>();
  private readonly snapshots = new Map<string, string>();
  private readonly epochs = new Map<string, number>();
  private readonly reloading = new Set<string>();

  constructor(ownerId: string, endpoint = "/mcp") {
    this.client = new McpClient(endpoint, { ownerId });
  }

  revision(projectId: string) { return this.revisions.get(projectId); }

  markPublished(projectId: string, revision: number, project?: MiniAppProject) {
    this.revisions.set(projectId, revision);
    if (project) this.snapshots.set(projectId, JSON.stringify(project));
  }

  async load() {
    const result = await this.client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
    const missing: ProjectMetadata[] = [];
    const projects: MiniAppProject[] = [];
    const deployments: Record<string, DeploymentReceipt> = {};
    for (const { project } of result.projects) {
      const inspected = await this.client.call<{ deployment: DeploymentReceipt | null }>("project_inspect", { projectId: project.id });
      if (inspected.deployment) deployments[project.id] = inspected.deployment;
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId: project.id });
      this.revisions.set(project.id, saved.revision);
      if (!saved.transfer) { missing.push(project); continue; }
      const bytes = await this.client.download(saved.transfer);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== project.id) throw new Error("The saved source does not match its project.");
      const repaired = repairDroppedPreviewUpload(source);
      this.snapshots.set(project.id, JSON.stringify(repaired));
      projects.push(repaired);
    }
    return { projects, missing, deployments };
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
    try {
      await this.tails.get(projectId)?.catch(() => {});
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId });
      if (!saved.transfer) return { revision: saved.revision, project: null };
      const bytes = await this.client.download(saved.transfer);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== projectId) throw new Error("The saved source does not match its project.");
      return { revision: saved.revision, project: repairDroppedPreviewUpload(source) };
    } catch (error) {
      this.reloading.delete(projectId);
      throw error;
    }
  }

  acceptReload(projectId: string, saved: ReloadedSources) {
    if (!this.reloading.has(projectId)) throw new Error("The source reload is no longer active.");
    this.revisions.set(projectId, saved.revision);
    if (saved.project) this.snapshots.set(projectId, JSON.stringify(saved.project));
    else this.snapshots.delete(projectId);
    this.reloading.delete(projectId);
  }

  cancelReload(projectId: string) {
    this.reloading.delete(projectId);
  }

  /** Saves the project's sources. They meet the Kernel's source rules before a byte travels, and a refusal names the file or the size and the limit. */
  save(project: MiniAppProject): Promise<number> {
    if (this.reloading.has(project.id)) return Promise.reject(new Error("Saved sources are loading. Resolve the source conflict before saving this draft."));
    const epoch = this.epochs.get(project.id) ?? 0;
    const previous = this.tails.get(project.id) ?? Promise.resolve(this.revisions.get(project.id) ?? 0);
    const snapshot = JSON.stringify(project);
    const next = previous.then(async () => {
      if (epoch !== (this.epochs.get(project.id) ?? 0) || this.reloading.has(project.id)) throw new Error("This save was superseded by a source reload.");
      const revision = this.revisions.get(project.id);
      if (revision !== undefined && this.snapshots.get(project.id) === snapshot) return revision;
      checkSourceFiles(project);
      const bytes = new TextEncoder().encode(JSON.stringify(project));
      checkSourceUpload(bytes.length);
      if (!this.revisions.has(project.id)) {
        const upload = await this.client.upload(bytes);
        try {
          const created = await this.client.call<ProjectMetadata & { sourcesRevision: number }>("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest, sourcesTransferId: upload.transferId });
          this.revisions.set(project.id, created.sourcesRevision);
          this.snapshots.set(project.id, snapshot);
          return created.sourcesRevision;
        } finally {
          await this.client.cancel(upload.transferId).catch(() => {});
        }
      }
      const upload = await this.client.upload(bytes, project.id);
      try {
        if (epoch !== (this.epochs.get(project.id) ?? 0) || this.reloading.has(project.id)) throw new Error("This save was superseded by a source reload.");
        const result = await this.client.call<{ revision: number }>("project_sources_save", { projectId: project.id, transferId: upload.transferId, expectedRevision: revision });
        if (epoch !== (this.epochs.get(project.id) ?? 0)) throw new Error("This save was superseded by a source reload.");
        this.revisions.set(project.id, result.revision);
        this.snapshots.set(project.id, snapshot);
        return result.revision;
      } finally {
        await this.client.cancel(upload.transferId).catch(() => {});
      }
    });
    this.tails.set(project.id, next);
    return next;
  }

  /**
   * Deletes a project on the Kernel, with its Deployment and data when it has
   * one, then reads the project list back and only resolves once it is gone.
   */
  async deleteProject(projectId: string, name: string): Promise<void> {
    await this.tails.get(projectId)?.catch(() => {});
    await this.client.call("project_delete", { projectId, name });
    const listed = await this.client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
    if (listed.projects.some(({ project }) => project.id === projectId)) throw new Error("The Kernel still lists this project. Reload and try again.");
    this.revisions.delete(projectId);
    this.snapshots.delete(projectId);
    this.tails.delete(projectId);
  }

  retry(projectId: string) {
    this.tails.delete(projectId);
  }
}
