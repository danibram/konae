export type FtpsDestination = { kind: "member" | "device"; folder?: string };
export type FtpsDeclaration = { version: 1; destinations: Record<string, FtpsDestination> };

const maxDeclarationBytes = 64 * 1024;

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseFtpsDeclaration(source: string): FtpsDeclaration {
  if (new TextEncoder().encode(source).length > maxDeclarationBytes) {
    throw new Error("app/ftps.json exceeds 65536 bytes.");
  }
  let value: unknown;
  try {
    value = JSON.parse(source);
  } catch {
    throw new Error("app/ftps.json is not valid JSON.");
  }
  return validateFtpsDeclaration(value);
}

export function validateFtpsDeclaration(declaration: unknown): FtpsDeclaration {
  if (!isObject(declaration)) throw new Error("app/ftps.json must be an object.");
  if (Object.keys(declaration).some((key) => key !== "version" && key !== "destinations")) throw new Error("app/ftps.json has an unknown field.");
  if (declaration.version !== 1) throw new Error("app/ftps.json version must be 1.");
  const declaredDestinations = declaration.destinations;
  if (!isObject(declaredDestinations)) {
    throw new Error("app/ftps.json must declare between 1 and 32 destinations.");
  }
  const entries = Object.keys(declaredDestinations).sort();
  if (entries.length < 1 || entries.length > 32) {
    throw new Error("app/ftps.json must declare between 1 and 32 destinations.");
  }
  const destinations = entries.map((id): [string, FtpsDestination] => {
    const destination = declaredDestinations[id];
    if (id.length < 1 || id.length > 64 || /[^A-Za-z0-9_-]/.test(id)) throw new Error("FTPS destination ID must use 1 to 64 ASCII letters, digits, '_' or '-'.");
    if (!isObject(destination)) throw new Error(`FTPS destination ${id} must be an object.`);
    if (Object.keys(destination).some((key) => key !== "kind" && key !== "folder")) throw new Error(`FTPS destination ${id} has an unknown field.`);
    if (destination.kind !== "member" && destination.kind !== "device") throw new Error(`FTPS destination ${id} kind must be "member" or "device".`);
    const kind = destination.kind;
    if (destination.folder === undefined) return [id, { kind }];
    validateFolder(id, destination.folder);
    return [id, { kind, folder: destination.folder }];
  });
  return { version: 1, destinations: Object.fromEntries(destinations) };
}

function validateFolder(id: string, folder: unknown): asserts folder is string {
  if (typeof folder !== "string" || !folder || new TextEncoder().encode(folder).length > 1024 || folder.startsWith("/") || folder.includes("\\") || /[\x00-\x1f\x7f]/.test(folder)) {
    throw new Error(`FTPS destination ${id} folder must be a normalized relative path of at most 1024 bytes.`);
  }
  if (folder.split("/").some((part) => !part || part === "." || part === "..")) {
    throw new Error(`FTPS destination ${id} folder must not contain empty, '.' or '..' components.`);
  }
}
