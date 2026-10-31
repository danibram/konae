// The name an app may answer on under <domain>/p/<name> (ADR-0024). The
// Kernel's kernel-rs/src/path_name.rs applies the same rules with the same
// words, and both test suites run kernel-rs/src/path-cases.json.

import type { RefusalCode } from "../kernel/refusal";

export type PathNameCheck = { ok: true; name: string } | { ok: false; code: RefusalCode; message: string };

const reserved = ["_konae", "p", "app", "mcp", "www", "api", "admin", "runtime.js"];
const uuidShape = /^(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32})$/;
const minLength = 3;
const maxLength = 40;

export function checkPathName(raw: string): PathNameCheck {
  const name = raw.trim().toLowerCase();
  const refuse = (code: RefusalCode, message: string): PathNameCheck => ({ ok: false, code, message });
  if (!name) return refuse("path.missing", "Name the path, such as spring-menu.");
  if (reserved.includes(name)) {
    return refuse("path.reserved", `${name} is reserved for Konae's own addresses. Choose another name, such as spring-menu.`);
  }
  const length = [...name].length;
  if (length < minLength) return refuse("path.length", `${name} is too short. Use ${minLength} to ${maxLength} characters.`);
  if (length > maxLength) return refuse("path.length", `${name} is too long. Use ${minLength} to ${maxLength} characters.`);
  if (!/^[a-z0-9-]+$/.test(name)) {
    return refuse("path.name-invalid", "Use only letters, digits and hyphens in the path name, such as spring-menu.");
  }
  if (name.startsWith("-")) return refuse("path.hyphen-edge", "Start the path name with a letter or digit, not a hyphen.");
  if (name.endsWith("-")) return refuse("path.hyphen-edge", "End the path name with a letter or digit, not a hyphen.");
  if (uuidShape.test(name)) {
    return refuse("path.uuid-shape", `${name} has the shape of an app id. Choose a name that does not look like one, such as spring-menu.`);
  }
  return { ok: true, name };
}
