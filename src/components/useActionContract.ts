import { useMemo } from "react";
import { generateActionContract, renderActionContractMarkdown, type ContractWarning } from "../lib/action-contract";

export type ActionContractState =
  | { ok: true; markdown: string; actions: number; warnings: ContractWarning[] }
  | { ok: false; reason: string };

/**
 * The Action Contract of the Capsule as it stands in the editor, written again
 * whenever actions.json or schema.sql changes. It is never stored: the export
 * and the CLI write it from the same sources.
 */
export function useActionContract(actionsSource: string | undefined, schemaSource: string | undefined, project: { id: string; name: string }): ActionContractState {
  const { id, name } = project;
  return useMemo(() => {
    if (actionsSource === undefined) return { ok: false, reason: "This project has no actions.json, so it has no Server Actions to describe." };
    try {
      const { contract, warnings } = generateActionContract({ actionsSource, schemaSource: schemaSource ?? "", project: { id, name } });
      return { ok: true, markdown: renderActionContractMarkdown(contract), actions: contract.actions.length, warnings };
    } catch (error) {
      return { ok: false, reason: `actions.json does not parse, so there is no contract to show. ${error instanceof Error ? error.message : String(error)}` };
    }
  }, [actionsSource, schemaSource, id, name]);
}
