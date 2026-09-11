import type { SqlValue } from "sql.js";

export const runtimeLimits = {
  databaseBytes: 10 * 1024 * 1024,
  parameterBytes: 256 * 1024,
  parameters: 100,
  resultBytes: 2 * 1024 * 1024,
  resultRows: 1_000,
  sqlLength: 20_000,
} as const;

type RuntimeSqlOperation = "query" | "execute";

const allowedKeywords: Record<RuntimeSqlOperation, ReadonlySet<string>> = {
  query: new Set(["SELECT"]),
  execute: new Set(["DELETE", "INSERT", "REPLACE", "UPDATE"]),
};

export function assertRuntimeSql(operation: RuntimeSqlOperation, sql: string) {
  if (!sql.trim()) throw new Error("A SQL string is required.");
  if (sql.length > runtimeLimits.sqlLength) throw new Error("SQL statement is too large.");

  const keyword = firstSqlKeyword(sql);
  if (!allowedKeywords[operation].has(keyword)) {
    const allowed = [...allowedKeywords[operation]].join(", ");
    throw new Error(`Runtime ${operation} only allows: ${allowed}. Use schema.sql for structural changes.`);
  }
}

export function validateRuntimeParams(params: unknown): SqlValue[] {
  if (!Array.isArray(params)) throw new Error("SQL params must be an array.");
  if (params.length > runtimeLimits.parameters) throw new Error("Too many SQL parameters.");

  let bytes = 0;
  for (const value of params) {
    if (value !== null && typeof value !== "string" && typeof value !== "number" && !(value instanceof Uint8Array)) {
      throw new Error("SQL params may only contain strings, numbers, blobs or null.");
    }
    bytes += sqlValueBytes(value as SqlValue);
  }
  if (bytes > runtimeLimits.parameterBytes) throw new Error("SQL parameters are too large.");
  return params as SqlValue[];
}

export function assertRuntimeResultWithinLimits(rows: Record<string, SqlValue>[]) {
  if (rows.length > runtimeLimits.resultRows) {
    throw new Error(`Query returned more than ${runtimeLimits.resultRows} rows. Add a LIMIT clause.`);
  }
  let bytes = 0;
  for (const row of rows) {
    for (const value of Object.values(row)) bytes += sqlValueBytes(value);
    if (bytes > runtimeLimits.resultBytes) throw new Error("Query result is too large.");
  }
}

export function assertRuntimeDatabaseWithinLimits(value: Uint8Array | number) {
  const byteLength = typeof value === "number" ? value : value.byteLength;
  if (byteLength > runtimeLimits.databaseBytes) {
    throw new Error(`Database exceeds the ${runtimeLimits.databaseBytes / 1024 / 1024} MiB local runtime limit.`);
  }
}

/** The keyword a statement starts with, skipping leading comments. Uppercased, "" when there is none. */
export function firstSqlKeyword(sql: string): string {
  let remaining = sql.trimStart();
  while (remaining.startsWith("--") || remaining.startsWith("/*")) {
    if (remaining.startsWith("--")) {
      const newline = remaining.indexOf("\n");
      remaining = newline === -1 ? "" : remaining.slice(newline + 1).trimStart();
      continue;
    }
    const commentEnd = remaining.indexOf("*/", 2);
    remaining = commentEnd === -1 ? "" : remaining.slice(commentEnd + 2).trimStart();
  }
  return remaining.match(/^[a-z]+/i)?.[0]?.toUpperCase() ?? "";
}

function sqlValueBytes(value: SqlValue): number {
  if (value === null) return 0;
  if (value instanceof Uint8Array) return value.byteLength;
  if (typeof value === "number") return 8;
  return new TextEncoder().encode(value).byteLength;
}
