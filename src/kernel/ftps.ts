export type FtpsDestination = { kind: "member" | "device"; folder?: string };
export type FtpsDeclaration = { version: 1; destinations: Record<string, FtpsDestination> };

const idPattern = /^[A-Za-z0-9_-]{1,64}$/;

export function parseFtpsDeclaration(source: string): FtpsDeclaration {
  let value: unknown;
  try {
    value = JSON.parse(source);
  } catch {
    throw new Error("app/ftps.json is not valid JSON.");
  }
  return validateFtpsDeclaration(value);
}

export function validateFtpsDeclaration(value: unknown): FtpsDeclaration {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("app/ftps.json must be an object.");
  const declaration = value as Record<string, unknown>;
  if (Object.keys(declaration).some((key) => key !== "version" && key !== "destinations")) throw new Error("app/ftps.json has an unknown field.");
  if (declaration.version !== 1) throw new Error("app/ftps.json version must be 1.");
  const destinations = declaration.destinations;
  if (!destinations || typeof destinations !== "object" || Array.isArray(destinations) || Object.keys(destinations).length < 1 || Object.keys(destinations).length > 32) {
    throw new Error("app/ftps.json must declare between 1 and 32 destinations.");
  }
  for (const [id, raw] of Object.entries(destinations)) {
    if (!idPattern.test(id)) throw new Error("FTPS destination ID must use 1 to 64 ASCII letters, digits, '_' or '-'.");
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error(`FTPS destination ${id} must be an object.`);
    const destination = raw as Record<string, unknown>;
    if (Object.keys(destination).some((key) => key !== "kind" && key !== "folder")) throw new Error(`FTPS destination ${id} has an unknown field.`);
    if (destination.kind !== "member" && destination.kind !== "device") throw new Error(`FTPS destination ${id} kind must be "member" or "device".`);
    if (destination.folder !== undefined) validateFolder(id, destination.folder);
  }
  return declaration as FtpsDeclaration;
}

function validateFolder(id: string, folder: unknown): asserts folder is string {
  if (typeof folder !== "string" || !folder || new TextEncoder().encode(folder).length > 1024 || folder.startsWith("/") || folder.includes("\\") || /[\x00-\x1f\x7f]/.test(folder)) {
    throw new Error(`FTPS destination ${id} folder must be a normalized relative path of at most 1024 bytes.`);
  }
  if (folder.split("/").some((part) => !part || part === "." || part === "..")) {
    throw new Error(`FTPS destination ${id} folder must not contain empty, '.' or '..' components.`);
  }
}
