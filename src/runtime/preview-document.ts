import type { MiniAppRuntimeHost } from "./host";

/**
 * Decides when the preview's failures stop applying. The host connects on the
 * app's `hello` and again on the iframe's `load`, and both fire for the same
 * document: clearing on every connect wiped a top-level throw that the first
 * port had already reported. Failures are cleared once per new document, and
 * a redundant connect for the same one keeps them.
 */
export function previewDocumentTracker(host: Pick<MiniAppRuntimeHost, "clearPreviewErrors">) {
  let current: string | undefined;
  let started = false;
  /** Call on every connect with the document's source. Returns whether it was a new document. */
  return (document: string): boolean => {
    if (started && document === current) return false;
    started = true;
    current = document;
    host.clearPreviewErrors();
    return true;
  };
}
