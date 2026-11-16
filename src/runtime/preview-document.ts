import type { MiniAppRuntimeHost } from "./host";

export function previewDocumentTracker(host: Pick<MiniAppRuntimeHost, "clearPreviewErrors">) {
  let current: string | undefined;
  let started = false;
  return (document: string): boolean => {
    if (started && document === current) return false;
    started = true;
    current = document;
    host.clearPreviewErrors();
    return true;
  };
}
