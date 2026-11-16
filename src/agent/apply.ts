import { directoriesForFiles } from "../lib/project-files";
import { keptEncodings, textFiles } from "../lib/source-files";
import type { MiniAppProject, ProjectFile } from "../lib/types";
import type { AgentProposal, CapsuleFiles } from "./contracts";

export type ProposalMerge =
  | { ok: true; files: CapsuleFiles; changedFiles: ProjectFile[] }
  /** Why the proposal was refused; every reason means "nothing was written". */
  | { ok: false; reason: "unvalidated" | "empty" | "drifted" };

/**
 * Landing a validated proposal on the Capsule.
 *
 * The gate's verdict is about a whole Capsule — the files the agent changed
 * *and* the ones it left alone, which the schema and the actions were checked
 * against. Writing only the changed files onto a Capsule that has moved since
 * produces a combination nothing ever validated: an action checked against a
 * column a hand edit has just removed passes the gate and breaks the app. So the
 * merge refuses unless the untouched files are still the ones that were seen.
 */
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
  for (const file of changed) files[file] = proposal.files[file];
  return { ok: true, files, changedFiles: [...changed] };
}

/**
 * The project after Apply. The agent saw the text files only, so the merge
 * checks those. A file that is not text stays as it is, with its encoding,
 * unless the proposal writes text at its path. Null when the merge refuses.
 */
export function applyProposal(project: MiniAppProject, proposal: AgentProposal, now = Date.now()): { project: MiniAppProject; changedFiles: ProjectFile[] } | null {
  const merge = mergeProposal(textFiles(project), proposal);
  if (!merge.ok) return null;
  const files = { ...project.files, ...merge.files };
  return {
    project: {
      ...project,
      files,
      encodings: keptEncodings(project.encodings, files, { replaced: merge.changedFiles }),
      directories: [...new Set([...project.directories, ...directoriesForFiles(files)])].sort(),
      updatedAt: now,
    },
    changedFiles: merge.changedFiles,
  };
}
