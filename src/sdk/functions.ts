export type FunctionContext = {
  readonly actor: { readonly userId: string | null };
  readonly actions: {
    call(name: string, input?: Record<string, unknown>): Promise<unknown>;
  };
};

export type FunctionHandler = (request: Request, context: FunctionContext) => Response | Promise<Response>;
export type FunctionShutdown = (signal: AbortSignal) => void | Promise<void>;
