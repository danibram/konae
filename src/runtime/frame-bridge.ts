import { handlePreviewMcp, type PreviewMcpRequest } from "../mcp/preview";
import type { MiniAppRuntimeHost } from "./host";
import { previewDocumentTracker } from "./preview-document";

export function connectPreviewFrame(iframe: HTMLIFrameElement, host: MiniAppRuntimeHost) {
  let ports: MessagePort[] = [];
  let portsDocument: string | null = null;
  const documentStarted = previewDocumentTracker(host);

  const connect = () => {
    const channel = new MessageChannel();
    const capability = crypto.randomUUID();
    const currentHost = host;
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
      const response = await handlePreviewMcp(currentHost, request, capability);
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
}
