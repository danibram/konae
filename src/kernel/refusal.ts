import codes from "../../kernel-rs/kernel/src/refusal-codes.json" with { type: "json" };

export type RefusalCode = keyof typeof codes;

export type RefusalFix = "capsule" | "owner" | "operator";

export type RefusalHint = { fix: RefusalFix; remedy: string; tool?: string };

export type Refusal = { code: RefusalCode; message: string; hint: RefusalHint };

const table: Record<string, RefusalHint> = codes as Record<string, RefusalHint>;

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

export function refusal(code: RefusalCode, message: string): RefusalError {
  return new RefusalError(code, message);
}

export function joinRefusals(refusals: readonly RefusalError[]): RefusalError {
  return new RefusalError(refusals[0].code, refusals.map((item) => item.message).join(" "));
}

export function hintOf(code: RefusalCode): RefusalHint {
  return table[code];
}

export function refusalCodes(): RefusalCode[] {
  return Object.keys(table) as RefusalCode[];
}

export function refusalOf(error: unknown): Refusal | null {
  return error instanceof RefusalError ? error.toJSON() : null;
}

export function ownerFixes(code: string | undefined): boolean {
  return code !== undefined && table[code]?.fix === "owner";
}
