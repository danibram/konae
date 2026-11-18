import { directoriesForFiles } from "../lib/project-files";
import { keptEncodings, textFiles } from "../lib/source-files";
import type { MiniAppProject, ProjectFile } from "../lib/types";
import type { AgentProposal, CapsuleFiles } from "./contracts";

export type ProposalMerge =
  | { ok: true; files: CapsuleFiles; changedFiles: ProjectFile[] }
  /** Why the proposal was refused; every reason means "nothing was written". */
  | { ok: false; reason: "unvalidated" | "empty" | "drifted" };

export function mergeProposal(current: CapsuleFiles, proposal: AgentProposal): ProposalMerge {
  if (!proposal.validation?.ok) return { ok: false, reason: "unvalidated" };
  if (proposal.changedFiles.length === 0) return { ok: false, reason: "empty" };

  const changed = new Set<ProjectFile>(proposal.changedFiles);
  const allFiles = new Set([...Object.keys(current), ...Object.keys(proposal.files)]);
  const drifted = [...allFiles].some(
    (file) => !changed.has(file) && (current[file] ?? "") !== (proposal.files[file] ?? ""),
  );
  if (drifted) return { ok: false, reason: "drifted" };

  const files = { ...current };
  for (const file of changed) {
    if (Object.hasOwn(proposal.files, file)) files[file] = proposal.files[file];
    else delete files[file];
  }
  return { ok: true, files, changedFiles: [...changed] };
}

export function applyProposal(project: MiniAppProject, proposal: AgentProposal, now = Date.now()): { project: MiniAppProject; changedFiles: ProjectFile[] } | null {
  // An explicit encoding map marks a full source snapshot. In particular, an
  // empty map means restoration intentionally contains no binary files.
  const current = proposal.encodings !== undefined ? project.files : textFiles(project);
  const merge = mergeProposal(current, proposal);
  if (!merge.ok) return null;
  const files = { ...project.files, ...merge.files };
  for (const file of proposal.changedFiles) if (!Object.hasOwn(proposal.files, file)) delete files[file];
  const encodings = { ...(keptEncodings(project.encodings, files, { replaced: merge.changedFiles }) ?? {}), ...(proposal.encodings ?? {}) };
  return {
    project: {
      ...project,
      files,
      encodings: Object.keys(encodings).length ? encodings : undefined,
      directories: [...new Set([...project.directories, ...directoriesForFiles(files)])].sort(),
      updatedAt: now,
    },
    changedFiles: merge.changedFiles,
  };
}
