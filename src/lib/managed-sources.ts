import { McpClient } from "../mcp/client";
import type { MiniAppProject } from "./types";
import type { DeploymentReceipt } from "../deployments/client";
import { checkEjectPackage, type EjectPlan, type EjectReceipt } from "./eject-package";
import { createTar } from "./tar";

/** Set by the Kernel's eject_apply: who ejected the project, when, and at which saved-sources revision. */
export type EjectRecord = { at: number; by: string; via: "session" | "token"; planId: string; sourcesRevision: number };
export type ProjectMetadata = { id: string; name: string; slug: string; sourceAuthority: "managed" | "external"; ejected?: EjectRecord };
type SourceResult = { revision: number; transfer: Parameters<McpClient["download"]>[0] | null };

/** One instance remains bound to the owner that opened the workspace. */
export class ManagedSources {
  private readonly client: McpClient;
  private readonly revisions = new Map<string, number>();
  private readonly tails = new Map<string, Promise<void>>();
  private readonly ejected = new Set<string>();

  constructor(ownerId: string, endpoint = "/mcp") {
    this.client = new McpClient(endpoint, { ownerId });
  }

  async load() {
    const result = await this.client.call<{ projects: { project: ProjectMetadata }[] }>("projects_list");
    // A project that was always External has no sources here. An ejected one
    // keeps its last saved sources, frozen, and opens read-only.
    const external = result.projects.filter(({ project }) => project.sourceAuthority === "external" && !project.ejected).map(({ project }) => project);
    const ejected: Record<string, EjectRecord> = {};
    const missing: ProjectMetadata[] = [];
    const projects: MiniAppProject[] = [];
    const deployments: Record<string, DeploymentReceipt> = {};
    for (const { project } of result.projects) {
      const inspected = await this.client.call<{ deployment: DeploymentReceipt | null }>("project_inspect", { projectId: project.id });
      if (inspected.deployment) deployments[project.id] = inspected.deployment;
      if (project.ejected) ejected[project.id] = project.ejected;
      else if (project.sourceAuthority !== "managed") continue;
      const saved = await this.client.call<SourceResult>("project_sources_read", { projectId: project.id });
      this.revisions.set(project.id, saved.revision);
      if (!saved.transfer) { missing.push(project); continue; }
      const bytes = await this.client.download(saved.transfer);
      const source = JSON.parse(new TextDecoder().decode(bytes)) as MiniAppProject;
      if (source.id !== project.id) throw new Error("The saved source does not match its project.");
      projects.push(source);
    }
    for (const id of Object.keys(ejected)) this.ejected.add(id);
    return { projects, external, missing, deployments, ejected };
  }

  save(project: MiniAppProject): Promise<void> {
    if (this.ejected.has(project.id)) return Promise.reject(new Error("This project was ejected. Its code is edited outside Littlebox now."));
    const previous = this.tails.get(project.id) ?? Promise.resolve();
    const next = previous.then(async () => {
      if (!this.revisions.has(project.id)) {
        const metadata = await this.client.call<ProjectMetadata>("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest, sourceAuthority: "managed" });
        if (metadata.sourceAuthority !== "managed") throw new Error("This project is maintained externally.");
        const saved = await this.client.call<SourceResult>("project_sources_read", { projectId: project.id });
        if (saved.transfer) {
          await this.client.cancel(saved.transfer.transferId);
          throw new Error("This project already has saved sources. Reload before saving.");
        }
        this.revisions.set(project.id, saved.revision);
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
   * Plans an Eject at the revision this tab last saved, then downloads and
   * checks the code package and packs it as a tar. Nothing on the server
   * changes yet; a tab that is behind is refused here.
   */
  async prepareEject(projectId: string, slug: string): Promise<{ plan: EjectPlan; archive: Uint8Array }> {
    await this.tails.get(projectId)?.catch(() => {});
    const expectedRevision = this.revisions.get(projectId) ?? 0;
    const plan = await this.client.call<EjectPlan>("eject_plan", { projectId, expectedRevision });
    const checked = await checkEjectPackage(await this.client.download(plan.transfer), plan);
    const folder = slug || projectId;
    const archive = createTar(Object.entries(checked.files).map(([path, contents]) => ({ path: `${folder}/${path}`, contents })));
    return { plan, archive };
  }

  /** Applies a prepared plan. From here the Kernel refuses this project's saves. */
  async applyEject(plan: EjectPlan): Promise<EjectReceipt> {
    const receipt = plan.applied && plan.receipt ? plan.receipt : await this.client.call<EjectReceipt>("eject_apply", { planId: plan.planId });
    this.ejected.add(plan.projectId);
    return receipt;
  }

  retry(projectId: string) {
    this.tails.delete(projectId);
  }
}
