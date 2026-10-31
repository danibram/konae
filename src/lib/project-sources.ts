import { McpClient, platformMcp } from "../mcp/client";
import type { MiniAppProject } from "./types";
import type { DeploymentReceipt } from "../deployments/client";
import { repairDroppedPreviewUpload } from "./starter";

export type ProjectMetadata = { id: string; name: string; slug: string };
type SourceResult = { revision: number; transfer: Parameters<McpClient["download"]>[0] | null };

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
  private readonly tails = new Map<string, Promise<void>>();

  constructor(ownerId: string, endpoint = "/mcp") {
    this.client = new McpClient(endpoint, { ownerId });
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
      projects.push(repairDroppedPreviewUpload(source));
    }
    return { projects, missing, deployments };
  }

  save(project: MiniAppProject): Promise<void> {
    const previous = this.tails.get(project.id) ?? Promise.resolve();
    const next = previous.then(async () => {
      if (!this.revisions.has(project.id)) {
        const upload = await this.client.upload(new TextEncoder().encode(JSON.stringify(project)));
        try {
          const created = await this.client.call<ProjectMetadata & { sourcesRevision: number }>("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest, sourcesTransferId: upload.transferId });
          this.revisions.set(project.id, created.sourcesRevision);
        } finally {
          await this.client.cancel(upload.transferId).catch(() => {});
        }
        return;
      }
      const upload = await this.client.upload(new TextEncoder().encode(JSON.stringify(project)), project.id);
      try {
        const result = await this.client.call<{ revision: number }>("project_sources_save", { projectId: project.id, transferId: upload.transferId, expectedRevision: this.revisions.get(project.id) });
        this.revisions.set(project.id, result.revision);
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
    this.tails.delete(projectId);
  }

  retry(projectId: string) {
    this.tails.delete(projectId);
  }
}
