import { RefusalError } from "./refusal";

export type Outcome = { code: string; message: string } | null;

export const MISSING_CODE = "(the refusal case names no code)";

export function expectedOutcome(item: { error: string | null; code?: string }): Outcome {
  return item.error === null ? null : { code: item.code ?? MISSING_CODE, message: item.error };
}

export function outcomeOf(run: () => unknown): Outcome {
  try {
    run();
    return null;
  } catch (error) {
    return refusalOutcome(error);
  }
}

export function refusalOutcome(error: unknown): Outcome {
  if (error instanceof RefusalError) return { code: error.code, message: error.message };
  return { code: "(not a refusal)", message: error instanceof Error ? error.message : String(error) };
}
