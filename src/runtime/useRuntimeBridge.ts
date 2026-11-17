import { useEffect, useRef } from "react";
import type { MiniAppRuntimeHost } from "./host";
import { connectPreviewFrame } from "./frame-bridge";

export function useRuntimeBridge(iframe: HTMLIFrameElement | null, host: MiniAppRuntimeHost) {
  const hostRef = useRef(host);
  hostRef.current = host;
  useEffect(() => {
    if (!iframe) return;
    return connectPreviewFrame(iframe, host);
  }, [host, iframe]);
  return hostRef;
}
