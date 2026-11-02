import type { ActionContract, ContractAction, ContractColumn, ContractInput, ContractOutput } from "./action-contract";

/** The file next to the Action Contract that types `miniapp.actions.call`. */
export const ACTION_TYPES_FILE = "actions.d.ts";

/**
 * TypeScript declarations for `miniapp.actions.call`, from the Action
 * Contract: each action a page may call, by name, with its input and what it
 * answers. A query answers its rows with the columns the contract read from
 * schema.sql. A column whose type the generator cannot tell is `any`.
 *
 * Only `miniapp.actions` is typed. The other modules stay `any`, so a page
 * that uses them keeps compiling.
 */
export function renderActionTypes(contract: ActionContract): string {
  const callable = contract.actions.filter(callableFromPage);
  const lines = [
    "// Generated from actions.json and schema.sql. Do not edit it, because konae dev, konae contract and the export write it again.",
    "// It types miniapp.actions.call by action name, input and the columns a query returns.",
    "",
    "export interface KonaeActions {",
    ...callable.flatMap((action) => [
      `  /** ${action.kind}, ${action.access}${action.actorBound ? ", needs a signed-in caller" : ""}. */`,
      `  ${JSON.stringify(action.name)}: {`,
      `    input: ${inputType(action.inputs)};`,
      `    output: ${outputType(action.output)};`,
      "  };",
    ]),
    "}",
    "",
    "type KonaeInput<Name extends keyof KonaeActions> = KonaeActions[Name][\"input\"];",
    "type KonaeInputArgument<Name extends keyof KonaeActions> = {} extends KonaeInput<Name> ? [input?: KonaeInput<Name>] : [input: KonaeInput<Name>];",
    "",
    "export interface KonaeMiniapp {",
    "  actions: {",
    "    call<Name extends keyof KonaeActions>(name: Name, ...input: KonaeInputArgument<Name>): Promise<KonaeActions[Name][\"output\"]>;",
    "  };",
    "  [module: string]: any;",
    "}",
    "",
    "declare global {",
    "  var miniapp: KonaeMiniapp;",
    "}",
    "",
  ];
  return lines.join("\n");
}

/**
 * Whether a page's `miniapp.actions.call` reaches the action. A scheduled
 * mutation, a call's then mutation and an action whose route is its only
 * door refuse it, so they are left out of the type.
 */
function callableFromPage(action: ContractAction): boolean {
  return !action.schedule && !action.thenOf && action.http?.onlyDoor !== "always";
}

function inputType(inputs: ContractInput[]): string {
  if (inputs.length === 0) return "Record<string, never>";
  const scalar = { string: "string", number: "number", boolean: "boolean" } as const;
  return `{ ${inputs.map((input) => input.required
    ? `${JSON.stringify(input.key)}: ${scalar[input.type]}`
    : `${JSON.stringify(input.key)}?: ${scalar[input.type]}${input.nullable ? " | null" : ""}`).join("; ")} }`;
}

function outputType(output: ContractOutput): string {
  switch (output.kind) {
    case "rows": return `Array<{ ${output.columns.map((column) => `${JSON.stringify(column.name)}: ${columnType(column)}`).join("; ")} }>`;
    case "changes": return "{ changes: number; lastInsertId: number }";
    case "signal": return "{ recorded: true }";
    case "queued": return "{ callId: string; queued: true }";
  }
}

/** A blob arrives as a hex string, as the Kernel writes it. */
function columnType(column: ContractColumn): string {
  const types: Record<ContractColumn["type"], string> = { integer: "number", real: "number", numeric: "number", text: "string", blob: "string", any: "any", unknown: "any" };
  const type = types[column.type];
  return type !== "any" && column.nullable !== false ? `${type} | null` : type;
}
