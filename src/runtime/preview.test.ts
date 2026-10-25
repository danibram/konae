import { describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { createHostedArtifact } from "./artifact";
import { createPreviewDocument } from "./preview";

const project = cloneStarter();

/**
 * The SDK a document installs on `window.miniapp`, as dotted names. Both
 * bootstrap scripts are run against a stub global, which is the only way to
 * compare what a Capsule can actually call in each place.
 */
function sdkSurfaceOf(html: string): string[] {
  const source = html.match(/<script data-littlebox-runtime>([\s\S]*?)<\/script>/)?.[1];
  if (!source) throw new Error("no bootstrap script in the document");

  const host: Record<string, any> = {};
  const scope: Record<string, unknown> = {
    window: host,
    addEventListener: () => {},
    parent: { postMessage: () => {} },
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    document: { createElement: () => ({ style: {}, dataset: {} }), body: { appendChild: () => {} } },
    FormData: class {},
    FileReader: class {},
  };
  new Function(...Object.keys(scope), source)(...Object.values(scope));

  return Object.entries(host.miniapp as Record<string, Record<string, unknown>>)
    .flatMap(([group, members]) => Object.keys(members).map((member) => `${group}.${member}`))
    .sort();
}

describe("preview document", () => {
  /**
   * The bug this pins: app.js used to run through `new AsyncFunction(source)`,
   * which parses as a function body while a Deployment parses it as a module.
   * A capsule could then work published and be silently dead in the preview —
   * the construction throws before any listener attaches, so the app does
   * nothing and shows no error.
   */
  test("runs app.js as a module, the same way a Deployment does", () => {
    const document = createPreviewDocument(project);
    expect(document).toContain('<script type="module">');
    expect(document).not.toContain("AsyncFunction");
    // The source is inlined, not passed as a JSON string.
    expect(document).toContain(project.files["app.js"].trim().split("\n")[0]);
  });

  test("reports runtime failures instead of failing silently", () => {
    const document = createPreviewDocument(project);
    expect(document).toContain("addEventListener('error'");
    expect(document).toContain("addEventListener('unhandledrejection'");
    expect(document).toContain("preview.error");
  });

  /**
   * The bug this pins: the preview kept a pre-Server-Action `miniapp.db` that a
   * Deployment has never had, so a Capsule using it worked here and died the
   * moment it was published — and nothing in the pipeline reads app.js to warn.
   */
  test("offers a Capsule exactly the SDK a Deployment offers", () => {
    const preview = sdkSurfaceOf(createPreviewDocument(project));
    expect(preview).toEqual(sdkSurfaceOf(createHostedArtifact({ body: "", styles: "", script: "" }).files["index.html"]));
    expect(preview).toEqual(["actions.call", "auth.session", "auth.signIn", "auth.signInWithGoogle", "auth.signOut", "files.delete", "files.list", "files.update", "files.upload", "files.url"]);
  });

  test("keeps a closing script tag inside app.js from ending the script element", () => {
    const hostile = {
      ...project,
      files: { ...project.files, "app.js": "const html = '</script><img onerror=alert(1)>';" },
    };
    expect(createPreviewDocument(hostile)).not.toContain("</script><img");
  });

  /**
   * The bug this pins: the preview wrapped the compiled module in try { }. esbuild
   * can emit a top-level `export { x }`, which is a SyntaxError inside a block, so
   * the app was dead in the preview and fine once published; and a startup throw
   * was caught locally, so preview_errors never heard of it.
   */
  test("emits the compiled module exactly as a Deployment does", () => {
    const script = "const x = 1;\nif (!document.querySelector('#app')) throw new Error('boom');\nexport { x };";
    const body = "<main id=app></main>";
    const styles = "main { color: red }";
    const preview = createPreviewDocument({ ...project, files: { ...project.files, "index.html": body, "styles.css": styles, "app.js": script } });
    const deployed = createHostedArtifact({ body, styles, script }).files["index.html"];

    const moduleOf = (html: string) => html.match(/<script type="module">([\s\S]*?)<\/script>/)?.[1];
    expect(moduleOf(preview)).toBe(script);
    expect(moduleOf(deployed)).toBe(script);
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(moduleOf(preview)!)).not.toThrow();
    const withoutRuntime = (html: string) => html.replace(/<script data-littlebox-runtime>[\s\S]*?<\/script>/, "");
    expect(withoutRuntime(preview)).toBe(withoutRuntime(deployed));
  });
});
