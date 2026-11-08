// A refusal of a Capsule carries a stable code beside its sentence
// (docs/adr/0004-the-gate-and-the-kernel-refuse-with-the-same-code-and-words.md). The site that refuses
// names the code, so a reworded sentence keeps its code and the editor
// branches on the code, never on the text. kernel-rs/kernel/src/refusal.rs is the
// Kernel's twin and reads the same table.
import codes from "../../kernel-rs/kernel/src/refusal-codes.json";

export type RefusalCode = keyof typeof codes;

/** Who clears a refusal: an edit to the Capsule, the owner outside it, or only the installation. */
export type RefusalFix = "capsule" | "owner" | "operator";

/** Who fixes it and how. `tool` names the MCP tool that clears it, when one does. */
export type RefusalHint = { fix: RefusalFix; remedy: string; tool?: string };

/** The shape every door answers a refusal with. */
export type Refusal = { code: RefusalCode; message: string; hint: RefusalHint };

const table: Record<string, RefusalHint> = codes as Record<string, RefusalHint>;

/** An Error whose message is the shared sentence, carrying the rule's code. */
export class RefusalError extends Error {
  constructor(readonly code: RefusalCode, message: string) {
    super(message);
    this.name = "RefusalError";
  }

  get hint(): RefusalHint {
    return hintOf(this.code);
  }

  toJSON(): Refusal {
    return { code: this.code, message: this.message, hint: this.hint };
  }
}

/** A refusal under `code`, ready to throw. */
export function refusal(code: RefusalCode, message: string): RefusalError {
  return new RefusalError(code, message);
}

/**
 * Several refusals named at once, such as every wrong key of a route's
 * signature, are one refusal: their sentences joined by a space, under the
 * first one's code. kernel-rs/kernel/src/refusal.rs `Refusal::join` does the same.
 */
export function joinRefusals(refusals: readonly RefusalError[]): RefusalError {
  return new RefusalError(refusals[0].code, refusals.map((item) => item.message).join(" "));
}

export function hintOf(code: RefusalCode): RefusalHint {
  return table[code];
}

/** Every code in the table, for the refusals topic and the tests. */
export function refusalCodes(): RefusalCode[] {
  return Object.keys(table) as RefusalCode[];
}

/** The refusal an error carries, or null when no Capsule rule refused it. */
export function refusalOf(error: unknown): Refusal | null {
  return error instanceof RefusalError ? error.toJSON() : null;
}

/** True when the owner, not an edit to the files, clears the refusal. */
export function ownerFixes(code: string | undefined): boolean {
  return code !== undefined && table[code]?.fix === "owner";
}
