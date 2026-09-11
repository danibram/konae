import { useEffect, useRef } from "react";
import type { RuntimeRequest, RuntimeResponse } from "../lib/types";
import { MiniAppRuntimeHost } from "./host";

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
    hostRef.current.setSchema(schema);
  }, [schema]);

  useEffect(() => {
    hostRef.current.setActions(actions);
  }, [actions]);

  useEffect(() => {
    if (!iframe) return;
    let activePort: MessagePort | null = null;

    const connect = () => {
      activePort?.close();
      const channel = new MessageChannel();
      const host = hostRef.current;
      // A fresh document is starting: failures from the previous one are history.
      host.clearPreviewErrors();
      activePort = channel.port1;

      activePort.onmessage = async (event: MessageEvent<RuntimeRequest>) => {
        const request = event.data;
        if (!request || request.channel !== "littlebox-runtime" || request.type !== "request") return;

        let response: RuntimeResponse;
        try {
          const result = await host.handle(request.method, request.payload);
          response = {
            channel: "littlebox-runtime",
            type: "response",
            requestId: request.requestId,
            ok: true,
            result,
          };
        } catch (error) {
          response = {
            channel: "littlebox-runtime",
            type: "response",
            requestId: request.requestId,
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          };
        }
        activePort?.postMessage(response);
      };
      activePort.start();
      iframe.contentWindow?.postMessage(
        { channel: "littlebox-runtime", type: "connect" },
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
      if (!message || message.channel !== "littlebox-runtime" || message.type !== "hello") return;
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
