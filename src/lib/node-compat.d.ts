import type { Plugin } from "esbuild-wasm";
export function nodeCompatPlugin(loadProvider?: (name: string) => Promise<string | undefined> | string | undefined): Plugin;
export function nodeCompatGlobals(): string;
export function nodeCompatBuildError(error: unknown): unknown;
