export interface FunctionActionsByName {}

export type FunctionContext<Name extends string = string> = {
  readonly actor: { readonly userId: string | null };
  readonly actions: Name extends keyof FunctionActionsByName ? FunctionActionsByName[Name] : {
    call(name: string, input?: Record<string, unknown>): Promise<unknown>;
  };
};

export type FunctionHandler = (request: Request, context: FunctionContext) => Response | Promise<Response>;
export type FunctionShutdown = (signal: AbortSignal) => void | Promise<void>;

export const ACTIONS_VIRTUAL_MODULE = "konae:actions";

export function actionsModuleBody(): string {
  return "export function actions(context) { return context.actions; }\n";
}
