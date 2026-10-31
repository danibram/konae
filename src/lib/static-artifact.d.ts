export type RoutingMode = "pages" | "spa";

export type StaticArtifact = {
  version: 1;
  entrypoint: "index.html";
  files: Record<string, string>;
  /** Paths whose value in `files` is base64. Every other file is UTF-8 text. */
  encodings?: Record<string, "base64">;
};

export const runtimeScriptAttribute: "data-konae-runtime";
export const runtimeFile: "runtime.js";
export const routingModes: RoutingMode[];
export const routingMessage: string;
export const pathPrefixPattern: RegExp;

export function checkRouting(value: unknown): RoutingMode;
export function servedPath(source: string): string | null;
export function isDocument(html: string): boolean;
export function renderCapsuleDocument(input: { body: string; styles: string; script: string; runtime: string; head?: string }): string;
export function splitFragmentHead(fragment: string): { head: string; body: string };
export function basePath(pathname: string): string;
export function runtimeLoaderScript(): string;
export function assembleStaticArtifact(input: {
  paths: string[];
  read: (path: string) => Promise<string | Uint8Array>;
  compile: () => Promise<{ styles: string; script: string }>;
  bundle: (path: string) => Promise<{ script: string; styles: string }>;
  runtime: string;
  head?: string;
  routing?: unknown;
  warn?: (message: string) => void;
}): Promise<StaticArtifact>;
export function stripModulePaths(code: string, held: Set<string>): string;
export function resolveReference(raw: string, from: string): string | null;
export function scanHtml(html: string, from: string, reference: (raw: string, from: string, kind: "asset" | "module") => void): void;
export function scanCss(css: string, from: string, reference: (raw: string, from: string, kind: "asset" | "module") => void): void;
export function isRootRelativeUrl(raw: string): boolean;
export function rootRelativeWarnings(files: Record<string, string>): { source: string; message: string }[];
