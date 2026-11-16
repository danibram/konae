import { useEffect, useRef } from "react";
import { handlePreviewMcp, type PreviewMcpRequest } from "../mcp/preview";
import type { MiniAppRuntimeHost } from "./host";
import { previewDocumentTracker } from "./preview-document";

export function useRuntimeBridge(
  iframe: HTMLIFrameElement | null,
  host: MiniAppRuntimeHost,
) {
  const hostRef = useRef(host);
  hostRef.current = host;

  useEffect(() => {
    if (!iframe) return;
    let ports: MessagePort[] = [];
    let portsDocument: string | null = null;
    const documentStarted = previewDocumentTracker(hostRef.current);

    const connect = () => {
      const channel = new MessageChannel();
      const capability = crypto.randomUUID();
      const host = hostRef.current;
      documentStarted(iframe.srcdoc);
      if (portsDocument !== iframe.srcdoc) {
        for (const old of ports) old.close();
        ports = [];
        portsDocument = iframe.srcdoc;
      }
      const port = channel.port1;
      ports.push(port);
      port.onmessage = async (event: MessageEvent<PreviewMcpRequest>) => {
        const request = event.data;
        if (!request || request.jsonrpc !== "2.0" || typeof request.method !== "string") return;
        const response = await handlePreviewMcp(host, request, capability);
        if (response) port.postMessage(response);
      };
      port.start();
      iframe.contentWindow?.postMessage(
        { channel: "konae-runtime", type: "connect", capability },
        "*",
        [channel.port2],
      );
    };

    const greeting = (event: MessageEvent) => {
      if (event.source !== iframe.contentWindow) return;
      const message = event.data as { channel?: string; type?: string } | null;
      if (!message || message.channel !== "konae-runtime" || message.type !== "hello") return;
      connect();
    };

    window.addEventListener("message", greeting);
    iframe.addEventListener("load", connect);
    connect();

    return () => {
      window.removeEventListener("message", greeting);
      iframe.removeEventListener("load", connect);
      for (const port of ports) port.close();
    };
  }, [host, iframe]);

  return hostRef;
}
