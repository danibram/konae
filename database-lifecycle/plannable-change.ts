/**
 * Some changes to a Database Model have two readings, and Drizzle resolves them
 * by asking a human: dropping `title` and adding `label` is either a rename that
 * carries the rows over or a replacement that discards them. A CLI can ask. The
 * Kernel serves HTTP requests and has nobody to ask, so it must decide the policy
 * itself — before Drizzle's differ reaches for a prompt that would never be
 * answered.
 *
 * The policy is to refuse. The two readings differ in what happens to the owner's
 * rows, and Littlebox does not guess about the owner's data.
 */

type Snapshot = {
  tables?: Record<string, { columns?: Record<string, unknown> }>;
  views?: Record<string, unknown>;
};

export function assertPlannableChange(previous: unknown, next: unknown): void {
  const before = previous as Snapshot;
  const after = next as Snapshot;

  const tablesBefore = Object.keys(before?.tables ?? {});
  const tablesAfter = Object.keys(after?.tables ?? {});
  if (isAmbiguous(tablesBefore, tablesAfter)) throw ambiguity("a table");

  for (const table of tablesBefore.filter((name) => tablesAfter.includes(name))) {
    const columnsBefore = Object.keys(before.tables?.[table]?.columns ?? {});
    const columnsAfter = Object.keys(after.tables?.[table]?.columns ?? {});
    if (isAmbiguous(columnsBefore, columnsAfter)) throw ambiguity(`a column of ${table}`);
  }

  if (isAmbiguous(Object.keys(before?.views ?? {}), Object.keys(after?.views ?? {}))) throw ambiguity("a view");
}

/** Ambiguous means something went and something else arrived: either could be the other, renamed. */
function isAmbiguous(before: string[], after: string[]): boolean {
  return before.some((name) => !after.includes(name)) && after.some((name) => !before.includes(name));
}

function ambiguity(what: string): Error {
  return new Error(
    `This change removes ${what} and adds another in the same step, so Littlebox cannot tell a rename from a replacement — and the two do different things to existing rows. Apply the addition and the removal as separate changes.`,
  );
}
