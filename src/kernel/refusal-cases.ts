// What the editor's case runners compare for each case of a shape case file
// in kernel-rs/src: null for a case that passes, else the code and the
// sentence. kernel-rs/src/refusal.rs `assert_case` is the Kernel's twin.
import { RefusalError } from "./refusal";

export type Outcome = { code: string; message: string } | null;

/** A case with no code compares unequal to every refusal, so it fails. */
export const MISSING_CODE = "(the refusal case names no code)";

export function expectedOutcome(item: { error: string | null; code?: string }): Outcome {
  return item.error === null ? null : { code: item.code ?? MISSING_CODE, message: item.error };
}

/** What a door answered: null when `run` returns, else the thrown refusal's code and sentence. */
export function outcomeOf(run: () => unknown): Outcome {
  try {
    run();
    return null;
  } catch (error) {
    return refusalOutcome(error);
  }
}

/** An error a door threw, as an outcome. An error that is not a refusal answers no code. */
export function refusalOutcome(error: unknown): Outcome {
  if (error instanceof RefusalError) return { code: error.code, message: error.message };
  return { code: "(not a refusal)", message: error instanceof Error ? error.message : String(error) };
}
