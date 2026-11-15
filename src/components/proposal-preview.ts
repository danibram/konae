import { applyProposal } from "../agent/apply";
import type { AgentProposal } from "../agent/contracts";
import type { MiniAppProject } from "../lib/types";

export type ProposalPreview = {
  id: string;
  proposal: AgentProposal;
  project: MiniAppProject;
};

/** Builds the complete preview project while preserving files the agent cannot read. */
export function proposalPreviewProject(project: MiniAppProject, proposal: AgentProposal): MiniAppProject | null {
  return applyProposal(project, proposal)?.project ?? null;
}
