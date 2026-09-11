import type { DatabaseAdminHost } from "../runtime/host";

const schemaSnapshotQuery = `
SELECT type, sql
FROM sqlite_master
WHERE type IN ('table', 'index', 'view', 'trigger')
  AND name NOT GLOB 'sqlite_*'
  AND sql IS NOT NULL
ORDER BY CASE type
  WHEN 'table' THEN 0
  WHEN 'index' THEN 1
  WHEN 'view' THEN 2
  WHEN 'trigger' THEN 3
  ELSE 4
END, name
`;

export async function createSchemaSnapshot(database: Pick<DatabaseAdminHost, "runSql">): Promise<string> {
  const result = await database.runSql(schemaSnapshotQuery);
  const statements = result.rows.flatMap((row) => {
    const type = row.type;
    const source = row.sql;
    if (typeof type !== "string" || typeof source !== "string" || !source.trim()) return [];
    return [makeIdempotent(type, source.trim())];
  });
  return statements.length ? `${statements.join("\n\n")}\n` : "";
}

function makeIdempotent(type: string, source: string): string {
  const createPrefix: Record<string, RegExp> = {
    table: /^(\s*CREATE\s+(?:TEMP(?:ORARY)?\s+)?(?:VIRTUAL\s+)?TABLE\s+)(?!IF\s+NOT\s+EXISTS\b)/i,
    index: /^(\s*CREATE\s+(?:UNIQUE\s+)?INDEX\s+)(?!IF\s+NOT\s+EXISTS\b)/i,
    view: /^(\s*CREATE\s+VIEW\s+)(?!IF\s+NOT\s+EXISTS\b)/i,
    trigger: /^(\s*CREATE\s+TRIGGER\s+)(?!IF\s+NOT\s+EXISTS\b)/i,
  };
  const prefix = createPrefix[type];
  if (!prefix) return source;
  const idempotent = source.replace(prefix, "$1IF NOT EXISTS ");
  return idempotent.endsWith(";") ? idempotent : `${idempotent};`;
}
