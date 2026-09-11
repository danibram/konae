import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const maximumModelBytes = 50_000;
const allowedImport = "drizzle-orm/sqlite-core";

/** The repository root, so module resolution never depends on the Kernel's working directory. */
export function repoRoot(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
}

/** Rejects imports and syntax outside the supported Drizzle declaration subset. */
export function assertConstrainedModel(source: string, label: string): void {
  if (!source.trim()) throw new Error(`${label} is empty.`);
  if (Buffer.byteLength(source) > maximumModelBytes) throw new Error(`${label} is larger than 50 KB.`);

  const imports = [...source.matchAll(/\b(?:import|export)\s+[\s\S]*?\sfrom\s+["']([^"']+)["']/g)].map((match) => match[1]);
  if (imports.length === 0 || imports.some((specifier) => specifier !== allowedImport)) {
    throw new Error(`${label} may only import from ${allowedImport}.`);
  }
  if (/\b(?:import\s*\(|require\s*\(|process\b|globalThis\b|fetch\s*\(|WebSocket\b|XMLHttpRequest\b|Bun\b|Deno\b|node:)/.test(source)) {
    throw new Error(`${label} contains executable capabilities that Managed Projects do not allow.`);
  }
  assertDeclarationSyntax(source, label);
}

const helpers = new Set(["sqliteTable", "integer", "text", "real", "blob", "numeric", "index", "uniqueIndex", "primaryKey", "foreignKey", "unique", "check"]);
const methods = new Set(["notNull", "primaryKey", "unique", "default", "references", "on", "asc", "desc", "onDelete", "onUpdate"]);
const forbiddenNames = new Set(["constructor", "prototype", "__proto__"]);

function assertDeclarationSyntax(source: string, label: string) {
  const file = ts.createSourceFile("schema.ts", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const imported = new Set<string>();
  const tableFactories = new Set<string>();
  const tables = new Set<string>();
  const fail = (): never => { throw new Error(`${label} only permits Drizzle declarations, literals and references. Executable expressions are not allowed.`); };
  const safeName = (name: string) => { if (forbiddenNames.has(name)) fail(); };

  for (const statement of file.statements) {
    if (ts.isImportDeclaration(statement)) {
      if (!ts.isStringLiteral(statement.moduleSpecifier) || statement.moduleSpecifier.text !== allowedImport || statement.importClause?.name || !statement.importClause?.namedBindings || !ts.isNamedImports(statement.importClause.namedBindings)) fail();
      for (const element of (statement.importClause!.namedBindings as ts.NamedImports).elements) {
        if (!helpers.has((element.propertyName ?? element.name).text)) fail();
        safeName(element.name.text);
        imported.add(element.name.text);
        if ((element.propertyName ?? element.name).text === "sqliteTable") tableFactories.add(element.name.text);
      }
    } else if (ts.isVariableStatement(statement)) {
      if (!(statement.declarationList.flags & ts.NodeFlags.Const)) fail();
      for (const declaration of statement.declarationList.declarations) {
        if (!ts.isIdentifier(declaration.name) || !declaration.initializer) fail();
        safeName(declaration.name.getText(file));
        tables.add(declaration.name.getText(file));
      }
    } else fail();
  }

  const expression = (node: ts.Expression, locals: Set<string>): void => {
    if (ts.isStringLiteral(node) || ts.isNumericLiteral(node) || [ts.SyntaxKind.TrueKeyword, ts.SyntaxKind.FalseKeyword, ts.SyntaxKind.NullKeyword].includes(node.kind)) return;
    if (ts.isParenthesizedExpression(node)) return expression(node.expression, locals);
    if (ts.isPrefixUnaryExpression(node) && [ts.SyntaxKind.MinusToken, ts.SyntaxKind.PlusToken].includes(node.operator) && ts.isNumericLiteral(node.operand)) return;
    if (ts.isIdentifier(node)) { if (!tables.has(node.text) && !locals.has(node.text) && node.text !== "undefined") fail(); return; }
    if (ts.isObjectLiteralExpression(node)) {
      for (const property of node.properties) {
        if (!ts.isPropertyAssignment(property) || !(ts.isIdentifier(property.name) || ts.isStringLiteral(property.name) || ts.isNumericLiteral(property.name))) fail();
        const assignment = property as ts.PropertyAssignment;
        safeName((assignment.name as ts.Identifier).text);
        expression(assignment.initializer, locals);
      }
      return;
    }
    if (ts.isArrayLiteralExpression(node)) { for (const item of node.elements) expression(item, locals); return; }
    if (ts.isPropertyAccessExpression(node)) {
      safeName(node.name.text);
      if (node.questionDotToken || !ts.isIdentifier(node.expression) || (!tables.has(node.expression.text) && !locals.has(node.expression.text))) fail();
      return;
    }
    if (ts.isCallExpression(node)) {
      if (node.questionDotToken) fail();
      if (ts.isIdentifier(node.expression)) { if (!imported.has(node.expression.text)) fail(); }
      else if (ts.isPropertyAccessExpression(node.expression) && methods.has(node.expression.name.text) && ts.isCallExpression(node.expression.expression)) expression(node.expression.expression, locals);
      else fail();
      for (const argument of node.arguments) expression(argument, locals);
      return;
    }
    if (ts.isArrowFunction(node)) {
      if (node.modifiers?.length || ts.isBlock(node.body) || node.parameters.some(parameter => !ts.isIdentifier(parameter.name) || parameter.initializer || parameter.dotDotDotToken)) fail();
      const nested = new Set(locals);
      for (const parameter of node.parameters) { const name = (parameter.name as ts.Identifier).text; safeName(name); nested.add(name); }
      expression(node.body as ts.Expression, nested);
      return;
    }
    fail();
  };

  for (const statement of file.statements) if (ts.isVariableStatement(statement)) {
    for (const declaration of statement.declarationList.declarations) {
      const initializer = declaration.initializer!;
      if (!ts.isCallExpression(initializer) || !ts.isIdentifier(initializer.expression) || !tableFactories.has(initializer.expression.text)) fail();
      expression(initializer, new Set());
    }
  }
}
