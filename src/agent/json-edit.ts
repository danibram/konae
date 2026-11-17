export type JsonChange = { op: "add" | "replace" | "remove"; pointer: string; value?: unknown; expected?: unknown };

export function editJson(source: string, changes: JsonChange[]): string {
  if (!changes.length || changes.length > 50) throw new Error("Provide between 1 and 50 JSON changes.");
  const document = JSON.parse(source);
  for (const change of changes) {
    if (!change.pointer.startsWith("/") || /~(?:[^01]|$)/.test(change.pointer)) throw new Error("Use a JSON Pointer starting with / and escape ~ as ~0 and / as ~1.");
    const keys = change.pointer.slice(1).split("/").map((key) => key.replaceAll("~1", "/").replaceAll("~0", "~"));
    if (keys.some((key) => ["__proto__", "constructor", "prototype"].includes(key))) throw new Error("That JSON path is reserved.");
    let parent = document;
    for (const key of keys.slice(0, -1)) {
      if (!parent || typeof parent !== "object" || !Object.hasOwn(parent, key)) throw new Error(`Missing JSON parent at ${change.pointer}.`);
      parent = parent[key];
    }
    const key = keys.at(-1)!;
    if (!parent || typeof parent !== "object") throw new Error(`Missing JSON parent at ${change.pointer}.`);
    if (Array.isArray(parent) && (!/^(0|[1-9]\d*)$/.test(key) || Number(key) >= parent.length || change.op !== "replace")) throw new Error("Array edits replace one existing index. Replace the whole array to add or remove items.");
    const exists = Object.hasOwn(parent, key);
    if (change.op === "add") {
      if (exists) throw new Error(`JSON path ${change.pointer} already exists. Use replace with expected.`);
    } else if (!exists || !Object.hasOwn(change, "expected") || JSON.stringify(parent[key]) !== JSON.stringify(change.expected)) {
      throw new Error(`JSON path ${change.pointer} did not match expected. Read it again; no changes were written.`);
    }
    if (change.op === "remove") delete parent[key];
    else {
      if (!Object.hasOwn(change, "value")) throw new Error("A JSON add or replace needs value.");
      parent[key] = change.value;
    }
  }
  return JSON.stringify(document, null, 2) + "\n";
}
