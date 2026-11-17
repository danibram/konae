import type { MiniAppProject } from "../lib/types";
import type { MiniAppRuntimeHost, PreviewActor, PreviewFailure } from "../runtime/host";
import { compileProjectInBrowser } from "../runtime/browser-build";
import { connectPreviewFrame } from "../runtime/frame-bridge";
import { createPreviewDocument } from "../runtime/preview";

export type PreviewStep =
  | { op: "fill"; selector: string; value: string }
  | { op: "click"; selector: string }
  | { op: "text"; selector: string; contains: string }
  | { op: "action"; name: string; input?: Record<string, unknown>; expectedError?: string };
export type PreviewTest = { actor?: PreviewActor; width?: number; steps: PreviewStep[] };
export type PreviewTestReport = {
  revision: string;
  scope: "isolated-candidate";
  ok: boolean;
  actor: PreviewActor;
  steps: Array<{ step: PreviewStep; ok: boolean; result?: unknown; error?: string }>;
  actions: Array<{ name: string; input: unknown; ok: boolean; result?: unknown; error?: string; expected?: boolean }>;
  errors: PreviewFailure[];
  text: string;
  viewport: { width: number; scrollWidth: number };
};

export async function previewRevision(project: MiniAppProject): Promise<string> {
  const files = Object.keys(project.files).sort().map((key) => [key, project.files[key], project.encodings?.[key]]);
  const bytes = new TextEncoder().encode(JSON.stringify([project.manifest, files]));
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(hash), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function runPreviewTest(project: MiniAppProject, applied: MiniAppRuntimeHost, test: PreviewTest, signal?: AbortSignal): Promise<PreviewTestReport> {
  signal?.throwIfAborted();
  const revision = await previewRevision(project);
  const compiled = await compileProjectInBrowser(project);
  signal?.throwIfAborted();
  const host = await applied.cloneForProposal(`test:${crypto.randomUUID()}`, project.files["schema.sql"], project.files["actions.json"], project.manifest.fileAccess);
  const actor = test.actor ?? "alice";
  host.setPreviewActor(actor);
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "allow-scripts allow-forms");
  frame.setAttribute("aria-hidden", "true");
  frame.style.cssText = `position:fixed;left:-100000px;top:0;width:${test.width ?? 375}px;height:900px;border:0;opacity:0;pointer-events:none`;
  let disconnect: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let receive: ((event: MessageEvent) => void) | undefined;
  let abort: (() => void) | undefined;
  try {
    signal?.throwIfAborted();
    const result = new Promise<Omit<PreviewTestReport, "revision" | "scope" | "actor" | "errors" | "ok">>((resolve, reject) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = (event: MessageEvent<Omit<PreviewTestReport, "revision" | "scope" | "actor" | "errors" | "ok">>) => {
        channel.port1.close();
        resolve(event.data);
      };
      channel.port1.start();
      abort = () => { channel.port1.close(); reject(signal?.reason ?? new Error("Preview test cancelled.")); };
      signal?.addEventListener("abort", abort, { once: true });
      receive = (event) => {
        if (event.source !== frame.contentWindow || event.data?.channel !== "konae-test" || event.data?.type !== "ready") return;
        window.removeEventListener("message", receive!);
        frame.contentWindow!.postMessage({ channel: "konae-test", steps: test.steps }, "*", [channel.port2]);
      };
      window.addEventListener("message", receive);
      timer = setTimeout(() => { channel.port1.close(); reject(new Error("Preview test timed out. The candidate did not finish its steps.")); }, 25_000);
    });
    const policy = '<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'unsafe-inline\'; style-src \'unsafe-inline\'; img-src data: blob:; font-src data:; connect-src \'none\'; form-action \'none\'; base-uri \'none\'">';
    frame.srcdoc = createPreviewDocument({ ...project, files: { ...project.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script } }, testDriver).replace(/<head>/i, `<head>${policy}`);
    document.body.appendChild(frame);
    disconnect = connectPreviewFrame(frame, host);
    const report = await result;
    const errors = host.previewErrors();
    return { ...report, revision, scope: "isolated-candidate", actor, errors, ok: report.steps.every((step) => step.ok) && report.actions.every((action) => action.ok || action.expected) && errors.length === 0 && report.viewport.scrollWidth <= report.viewport.width };
  } finally {
    if (timer) clearTimeout(timer);
    if (receive) window.removeEventListener("message", receive);
    if (abort) signal?.removeEventListener("abort", abort);
    disconnect?.();
    frame.remove();
    await host.dispose(true);
  }
}

const testDriver = String.raw`
;(() => {
  const pending = new Set();
  const actions = [];
  const call = miniapp.actions.call;
  miniapp.actions.call = (name, input = {}) => {
    const entry = { name, input, ok: false };
    actions.push(entry);
    const request = call(name, input).then(result => { entry.ok = true; entry.result = result; return result; }, error => { entry.error = error.message; throw error; });
    pending.add(request);
    request.then(() => pending.delete(request), () => pending.delete(request));
    return request;
  };
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const settle = async () => {
    const end = Date.now() + 3000;
    do { await pause(200); } while (pending.size && Date.now() < end);
    if (pending.size) throw new Error('The app still has unfinished Runtime calls.');
  };
  const find = async selector => {
    const end = Date.now() + 3000;
    do {
      const elements = [...document.querySelectorAll(selector)];
      if (elements.length > 1) throw new Error('Selector matched more than one element: ' + selector);
      const element = elements[0];
      if (element && element.getClientRects().length && !element.disabled) return element;
      await pause(50);
    } while(Date.now() < end);
    throw new Error('No enabled visible element: ' + selector);
  };
  window.confirm = () => true;
  const receiveTest = async event => {
    if (event.source !== parent || event.data?.channel !== 'konae-test' || !event.ports[0]) return;
    removeEventListener('message', receiveTest);
    const port = event.ports[0];
    const results = [];
    await settle();
    for (const step of event.data.steps) {
      const entry = {step, ok:false}; results.push(entry);
      try {
        if (step.op === 'action') {
          try {
            entry.result = await miniapp.actions.call(step.name, step.input || {});
            if (step.expectedError) throw new Error('Expected a refusal, but the action succeeded.');
          } catch(error) {
            if (!step.expectedError || !error.message.includes(step.expectedError)) throw error;
            const action = actions[actions.length - 1];
            if (action.ok) throw error;
            action.expected = true; entry.result = {refused:error.message};
          }
        } else {
          const element = await find(step.selector);
          if (step.op === 'fill') {
            const setter = Object.getOwnPropertyDescriptor(element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype, 'value').set;
            setter.call(element, step.value);
            element.dispatchEvent(new Event('input', {bubbles:true}));
            element.dispatchEvent(new Event('change', {bubbles:true}));
          } else if(step.op === 'click') element.click();
          else if(step.op === 'text') {
            const end = Date.now() + 3000;
            const text = () => document.querySelector(step.selector)?.textContent || '';
            while(!text().includes(step.contains) && Date.now() < end) await pause(50);
            if(!text().includes(step.contains)) throw new Error('Expected text ' + step.contains + '; saw ' + text().slice(0,500));
          }
        }
        await settle(); entry.ok = true;
      } catch(error) { entry.error = error.message; break; }
    }
    port.postMessage({steps:results, actions, text:document.body.innerText.slice(0,8000), viewport:{width:innerWidth,scrollWidth:document.documentElement.scrollWidth}});
    port.close();
  };
  addEventListener('message', receiveTest);
  addEventListener('load', () => parent.postMessage({channel:'konae-test',type:'ready'}, '*'), {once:true});
})();`;
