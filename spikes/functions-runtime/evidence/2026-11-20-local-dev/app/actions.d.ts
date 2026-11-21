// Generated from actions.json and schema.sql. Do not edit it, because konae dev, konae contract and the export write it again.
// It types miniapp.actions.call by action name, input and the columns a query returns.

export interface KonaeActions {
  /** mutation, authenticated, needs a signed-in caller. */
  "notes.batch": {
    input: { "titlesJson": string };
    output: { changes: number; lastInsertId: number };
  };
  /** mutation, authenticated, needs a signed-in caller. */
  "notes.create": {
    input: { "title": string };
    output: { changes: number; lastInsertId: number };
  };
  /** mutation, authenticated, needs a signed-in caller. */
  "notes.delete": {
    input: { "id": number };
    output: { changes: number; lastInsertId: number };
  };
  /** query, authenticated, needs a signed-in caller. */
  "notes.list": {
    input: Record<string, never>;
    output: Array<{ "id": number; "title": string; "created_at": number }>;
  };
}

type KonaeInput<Name extends keyof KonaeActions> = KonaeActions[Name]["input"];
type KonaeInputArgument<Name extends keyof KonaeActions> = {} extends KonaeInput<Name> ? [input?: KonaeInput<Name>] : [input: KonaeInput<Name>];

export interface KonaeMiniapp {
  actions: {
    call<Name extends keyof KonaeActions>(name: Name, ...input: KonaeInputArgument<Name>): Promise<KonaeActions[Name]["output"]>;
  };
  [module: string]: any;
}

declare global {
  var miniapp: KonaeMiniapp;
}
