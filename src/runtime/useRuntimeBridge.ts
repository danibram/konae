import { useEffect, useRef } from "react";
import { handlePreviewMcp, type PreviewMcpRequest } from "../mcp/preview";
import { MiniAppRuntimeHost } from "./host";
import { previewDocumentTracker } from "./preview-document";

/**
 * Takes the element, not a ref object: the preview mounts after the editor
 * opens, and an effect keyed on a ref would run once against a null iframe and
 * never again — leaving every `miniapp` call waiting for a port that was never
 * sent.
 */
export function useRuntimeBridge(
  iframe: HTMLIFrameElement | null,
  appId: string,
  schema: string,
  actions: string,
) {
  const hostRef = useRef<MiniAppRuntimeHost>(null!);
  const appIdRef = useRef(appId);
  if (!hostRef.current || appIdRef.current !== appId) {
    hostRef.current = new MiniAppRuntimeHost(appId, schema, actions);
    appIdRef.current = appId;
  }

  useEffect(() => {
    void hostRef.current.setSchema(schema);
  }, [schema]);

  useEffect(() => {
    hostRef.current.setActions(actions);
  }, [actions]);

  useEffect(() => {
    if (!iframe) return;
    let activePort: MessagePort | null = null;
    const documentStarted = previewDocumentTracker(hostRef.current);

    const connect = () => {
      activePort?.close();
      const channel = new MessageChannel();
      const capability = crypto.randomUUID();
      const host = hostRef.current;
      // `hello` and `load` both connect the same document. Only a new document
      // (a changed srcdoc, or a new iframe, which runs this effect again)
      // makes the previous failures history.
      documentStarted(iframe.srcdoc);
      activePort = channel.port1;

      const port = activePort;
      activePort.onmessage = async (event: MessageEvent<PreviewMcpRequest>) => {
        const request = event.data;
        if (!request || request.jsonrpc !== "2.0" || typeof request.method !== "string") return;
        const response = await handlePreviewMcp(host, request, capability);
        if (response) port.postMessage(response);
      };
      activePort.start();
      iframe.contentWindow?.postMessage(
        { channel: "konae-runtime", type: "connect", capability },
        "*",
        [channel.port2],
      );
    };

    /**
     * Three ways in, because no single one is reliable: a `srcDoc` document can
     * finish loading before this effect attaches the listener (the load event is
     * then already in the past), and the app's own greeting can equally arrive
     * before we are listening. Whichever happens last completes the handshake,
     * and the client accepts the newest port.
     */
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
      activePort?.close();
    };
  }, [appId, iframe]);

  return hostRef;
}
