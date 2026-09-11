import { describe, expect, test } from "bun:test";
import { createPreviewDocument } from "./preview";
import { cloneStarter } from "../lib/starter";

const source = createPreviewDocument(cloneStarter());
const bridge = await Bun.file(new URL("./useRuntimeBridge.ts", import.meta.url)).text();

/**
 * The preview once looked alive and did nothing: `miniapp` existed, the markup
 * was there, no error was raised — but the runtime port never arrived, so every
 * call waited forever. Two independent causes, both pinned here.
 */
describe("runtime handshake", () => {
  test("the host takes the iframe element, not a ref object", () => {
    // A ref object cannot be an effect dependency: the editor opens on the
    // project list, so the effect ran once against a null iframe and never again.
    expect(bridge).toContain("iframe: HTMLIFrameElement | null");
    expect(bridge).not.toContain("iframeRef: RefObject");
    expect(bridge).toContain("}, [appId, iframe]);");
  });

  test("the host offers three ways to complete the handshake", () => {
    // srcDoc can finish loading before the listener is attached, and the app's
    // greeting can arrive before the host listens. Whichever is last wins.
    expect(bridge).toContain('iframe.addEventListener("load", connect)');
    expect(bridge).toContain('window.addEventListener("message", greeting)');
    expect(bridge).toMatch(/\n\s*connect\(\);\n/);
  });

  test("failures are cleared per document, not per connect", () => {
    // hello and load both connect the same document; clearing on each wiped a
    // top-level throw. preview-document.test.ts pins the behavior.
    expect(bridge).toContain("documentStarted(iframe.srcdoc);");
    expect(bridge).not.toContain("clearPreviewErrors()");
  });

  test("the app greets its host and accepts a replacement port", () => {
    expect(source).toContain("type: 'hello'");
    // Refusing a second port would strand the app on a closed one.
    expect(source).toContain("if (runtimePort) runtimePort.close();");
    expect(source).not.toContain("|| runtimePort ||");
  });
});
