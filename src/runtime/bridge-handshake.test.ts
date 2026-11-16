import { describe, expect, test } from "bun:test";
import { createPreviewDocument } from "./preview";
import { cloneStarter } from "../lib/starter";

const source = createPreviewDocument(cloneStarter());
const bridge = await Bun.file(new URL("./useRuntimeBridge.ts", import.meta.url)).text();

describe("runtime handshake", () => {
  test("the host takes the iframe element, not a ref object", () => {
    expect(bridge).toContain("iframe: HTMLIFrameElement | null");
    expect(bridge).not.toContain("iframeRef: RefObject");
    expect(bridge).toContain("}, [host, iframe]);");
  });

  test("the host offers three ways to complete the handshake", () => {
    expect(bridge).toContain('iframe.addEventListener("load", connect)');
    expect(bridge).toContain('window.addEventListener("message", greeting)');
    expect(bridge).toMatch(/\n\s*connect\(\);\n/);
  });

  test("failures are cleared per document, not per connect", () => {
    expect(bridge).toContain("documentStarted(iframe.srcdoc);");
    expect(bridge).not.toContain("clearPreviewErrors()");
  });

  test("the app greets its host and accepts a replacement port", () => {
    expect(source).toContain("type: 'hello'");
    expect(source).not.toContain("|| runtimePort ||");
  });

  test("a replaced port stays open, so a request sent on it still gets its answer", () => {
    expect(source).not.toContain("runtimePort.close()");
    expect(bridge).not.toContain("activePort?.close()");
    expect(bridge).toContain("if (portsDocument !== iframe.srcdoc)");
  });
});
