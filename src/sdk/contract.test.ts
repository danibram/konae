import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { sdkPromptSection } from "../agent/sdk-section.generated";
import { buildSystemPrompt } from "../agent/system-prompt";
import { cloneStarter } from "../lib/starter";
import { runtimeClient } from "../runtime/preview";
import { contractCalls, sdkContract, type SdkContract, type SdkError } from "./contract";
import { renderAll, renderedPaths, renderPromptSection } from "./render";

const root = path.resolve(import.meta.dir, "../..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");
const publishedSdk = read("src/mcp/runtime.js");

/** Runs an SDK script against a bare window and lists the calls it puts on window.miniapp. */
function exposedCalls(source: string): string[] {
  const window: { miniapp?: Record<string, Record<string, unknown>> } = {};
  new Function("window", "location", "addEventListener", "parent", "document", source)(window, { pathname: "/" }, () => {}, undefined, undefined);
  return Object.entries(window.miniapp ?? {}).flatMap(([module, calls]) =>
    Object.entries(calls).filter(([, value]) => typeof value === "function").map(([call]) => `${module}.${call}`));
}

/** Calls one side names that the other does not, both ways. Empty when they agree. */
function drift(named: string[], contract: SdkContract = sdkContract) {
  const listed = contractCalls(contract);
  return {
    notInContract: [...new Set(named)].filter((call) => !listed.includes(call)).sort(),
    missing: listed.filter((call) => !named.includes(call)).sort(),
  };
}

/** Every miniapp.<module>.<call> a text names. */
function namedCalls(text: string): string[] {
  return [...text.matchAll(/\bminiapp\.([a-z]+)\.([a-zA-Z]+)\b/g)].map((match) => `${match[1]}.${match[2]}`);
}

const agreed = { notInContract: [], missing: [] };

/** A contract with one extra call, to prove each check notices a change. */
function withExtraCall(): SdkContract {
  const contract = structuredClone(sdkContract) as unknown as SdkContract;
  contract.modules[0].calls.push({ ...contract.modules[0].calls[0], name: "refresh", signature: "miniapp.auth.refresh()", examples: [{ code: "await miniapp.auth.refresh();" }] });
  return contract;
}

describe("the SDK contract", () => {
  test("lists each module and call once, with a signature and an example that name it", () => {
    const calls = contractCalls();
    expect(new Set(calls).size).toBe(calls.length);
    expect(calls).toEqual(["auth.session", "auth.signIn", "auth.signInWithGoogle", "auth.signOut", "actions.call", "files.list", "files.upload", "files.url", "files.update", "files.delete"]);
    for (const module of sdkContract.modules) {
      for (const call of module.calls) {
        const name = `miniapp.${module.name}.${call.name}`;
        expect(call.signature.startsWith(`${name}(`)).toBe(true);
        expect(call.examples.some((example) => example.code.includes(`${name}(`))).toBe(true);
      }
    }
  });

  test("names authentication-required on every call a signed-out visitor cannot make", () => {
    for (const module of sdkContract.modules) {
      for (const call of module.calls.filter((call) => !call.signedOut)) {
        expect(call.errors.map((error) => error.message)).toContain("authentication-required");
      }
    }
  });
});

describe("runtime.js and the preview client expose exactly the contract", () => {
  test("the published SDK puts every contract call on window.miniapp and nothing else", () => {
    expect(drift(exposedCalls(publishedSdk))).toEqual(agreed);
  });

  test("the editor preview client exposes the same calls", () => {
    expect(drift(exposedCalls(runtimeClient))).toEqual(agreed);
  });

  test("the published SDK calls exactly the runtime tools the contract lists", () => {
    const used = [...new Set([...publishedSdk.matchAll(/\bcall\('([a-z_]+)'/g)].map((match) => match[1]))].sort();
    const listed = [...new Set(sdkContract.modules.flatMap((module) => module.calls.flatMap((call) => call.tools)))].sort();
    expect(used).toEqual(listed);
  });

  test("a call the contract does not list, or one it lists that the SDK lacks, is reported", () => {
    expect(drift([...exposedCalls(publishedSdk), "files.remove"]).notInContract).toEqual(["files.remove"]);
    expect(drift(exposedCalls(publishedSdk), withExtraCall()).missing).toEqual(["auth.refresh"]);
  });
});

describe("every documented message is one the code returns", () => {
  const kernel = ["kernel-rs/kernel/src", "kernel-rs/konae/src"]
    .flatMap((dir) => readdirSync(path.join(root, dir)).filter((file) => file.endsWith(".rs")).map((file) => read(`${dir}/${file}`)))
    .join("\n");
  const preview = ["src/runtime/host.ts", "src/runtime/security.ts", "src/kernel/actions.ts", "src/mcp/preview.ts"].map(read).join("\n");

  /**
   * Matches a message in source: <key>, <name>, <names>, <field> or <kind> stand for a Rust {key}, a JS ${...}
   * or the same word written out, <limit> stands for a configured size that Rust formats with {}, a number
   * may be a JS ${...}, and a brace may be doubled, as Rust's
   * format! escapes it.
   */
  function inSource(message: string, source: string) {
    const pattern = message
      .split(/(<key>|<names>|<name>|<field>|<kind>|<limit>|\d+)/)
      .map((part) => {
        // A configured size, which Rust writes with a `{}` argument.
        if (part === "<limit>") return String.raw`(?:\{\w*\}|\$\{[^}]+\})`;
        if (part === "<key>" || part === "<name>" || part === "<names>" || part === "<field>" || part === "<kind>") return String.raw`(?:\{\w+\}|\$\{[^}]+\}|${part})`;
        if (/^\d+$/.test(part)) return String.raw`(?:${part}|\$\{[^}]+\})`;
        return part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\\([{}])/g, "\\$1\\$1?");
      })
      .join("");
    return new RegExp(pattern).test(source);
  }

  const errors = [
    ...sdkContract.errors,
    ...sdkContract.modules.flatMap((module) => module.calls.flatMap((call) => call.errors)),
    ...sdkContract.modules.flatMap((module) => module.declarations.flatMap((declaration): readonly SdkError[] => ("errors" in declaration ? declaration.errors : []))),
  ];

  test.each(errors.map((error) => [error.message, error] as const))("%s", (_, error) => {
    expect(inSource(error.message, error.source === "kernel" ? kernel : publishedSdk)).toBe(true);
    for (const message of error.preview ?? []) expect(inSource(message, preview)).toBe(true);
  });

  test("a message the code does not return is caught", () => {
    expect(inSource("Server action not declared.", kernel)).toBe(true);
    expect(inSource("Server action is missing.", kernel)).toBe(false);
    expect(inSource("Files must be between 1 byte and <limit>.", kernel)).toBe(true);
    expect(inSource("Files must be between 2 bytes and <limit>.", kernel)).toBe(false);
  });
});

describe("the rendered readers", () => {
  test("the checked-in prompt section, platform_guide topic and docs/sdk.md are the current rendering", () => {
    const rendered = renderAll();
    expect(Object.keys(rendered).sort()).toEqual(Object.values(renderedPaths).sort());
    for (const [file, contents] of Object.entries(rendered)) {
      expect({ file, contents: read(file) }).toEqual({ file, contents });
    }
  });

  test("editing the contract changes every reader", () => {
    const before = renderAll();
    const after = renderAll(withExtraCall());
    for (const file of Object.values(renderedPaths)) {
      expect(after[file]).not.toBe(before[file]);
      expect(after[file]).toContain("miniapp.auth.refresh()");
    }
  });

  test("the build agent's prompt carries the rendered section and names no other call", () => {
    const prompt = buildSystemPrompt(cloneStarter());
    // The generated module escapes the section into a template literal; the value must survive it.
    expect(sdkPromptSection).toBe(renderPromptSection());
    expect(prompt).toContain(sdkPromptSection);
    expect(drift(namedCalls(prompt))).toEqual(agreed);
  });

  test("platform_guide serves the rendered JSON, and the starter app in mcp.rs uses only contract calls", () => {
    const mcp = read("kernel-rs/konae/src/mcp.rs");
    // platform_guide's topics live in guide.rs; mcp.rs hands the call to it.
    expect(mcp).toContain(`"platform_guide" => super::guide::topic(`);
    expect(read("kernel-rs/konae/src/guide.rs")).toContain(`"sdk" => serde_json::from_str::<Value>(include_str!("sdk-guide.json"))?,`);
    expect(drift(namedCalls(mcp)).notInContract).toEqual([]);
    const guide = JSON.parse(read(renderedPaths.guide)) as { modules: { calls: { call: string }[] }[] };
    expect(drift(guide.modules.flatMap((module) => module.calls.map((call) => call.call.replace(/^miniapp\./, ""))))).toEqual(agreed);
  });
});

describe("the published SDK prefixes every Kernel URL with the page's base path", () => {
  /** Runs runtime.js on a page at pathname and records the URLs it asks the Kernel for. */
  async function sdkOn(pathname: string) {
    const requested: string[] = [];
    const assigned: string[] = [];
    const location = { pathname, assign: (url: string) => assigned.push(url) };
    const window: { miniapp?: Record<string, Record<string, (...args: unknown[]) => Promise<unknown>>> } = {};
    const original = globalThis.fetch;
    globalThis.fetch = (async (url: string) => {
      requested.push(String(url));
      return new Response("{}", { status: 500 });
    }) as typeof fetch;
    try {
      new Function("window", "location", "addEventListener", publishedSdk)(window, location, () => {});
      const fileUrl = await window.miniapp!.files.url("file-1");
      await window.miniapp!.actions.call("tasks.feed").catch(() => undefined);
      await window.miniapp!.auth.signIn();
      return { fileUrl, requested, assigned };
    } finally {
      globalThis.fetch = original;
    }
  }

  test("the home and a generated hostname use the root paths", async () => {
    const home = await sdkOn("/");
    expect(home).toEqual({ fileUrl: "/_konae/files/file-1", requested: ["/_konae/mcp"], assigned: ["/_konae/login"] });
  });

  test("a path address uses its /p/<segment> prefix for the endpoint, the file address and sign-in", async () => {
    for (const pathname of ["/p/notes", "/p/notes/", "/p/notes/about/deep.html"]) {
      const onPath = await sdkOn(pathname);
      expect(onPath.fileUrl).toBe("/p/notes/_konae/files/file-1");
      expect(onPath.requested).toEqual(["/p/notes/_konae/mcp"]);
      expect(onPath.assigned).toEqual(["/p/notes/_konae/login"]);
    }
  });

  test("a path that only looks like a prefix keeps the root paths", async () => {
    expect((await sdkOn("/pages/notes")).fileUrl).toBe("/_konae/files/file-1");
    expect((await sdkOn("/p/")).requested).toEqual(["/_konae/mcp"]);
  });
});

describe("files.upload sends the file's bytes in one request", () => {
  /** Runs runtime.js at pathname against a Kernel whose transfer path answers putStatus, and uploads one file. */
  async function uploadOn(pathname: string, putStatus: number, putError = "Use authenticated MCP for platform operations.") {
    const requests: string[] = [];
    const tools: string[] = [];
    const window: { miniapp?: { files: { upload: (file: File) => Promise<{ id: string; url: string }> } } } = {};
    const original = globalThis.fetch;
    globalThis.fetch = (async (url: string, init: RequestInit) => {
      requests.push(`${init.method} ${url}`);
      if (String(url).includes("/_konae/transfers/")) {
        if (putStatus !== 200) return Response.json({ error: putError }, { status: putStatus });
        return Response.json({ result: { transferId: "t-1", complete: true } });
      }
      const call = JSON.parse(String(init.body));
      if (!call.id) return new Response(null, { status: 202 });
      const name = call.params?.name;
      if (name) tools.push(name);
      const result = name === "transfer_begin" ? { transferId: "t-1", chunkBytes: 65536 } : name === "files_upload" ? { id: "file-1", name: "a.txt" } : {};
      return Response.json({ jsonrpc: "2.0", id: call.id, result: call.method === "tools/call" ? { structuredContent: { result }, isError: false } : {} });
    }) as typeof fetch;
    try {
      new Function("window", "location", "addEventListener", publishedSdk)(window, { pathname }, () => {});
      const stored = await window.miniapp!.files.upload(new File(["hello"], "a.txt", { type: "text/plain" })).catch((error: Error) => error);
      return { stored, requests, tools };
    } finally {
      globalThis.fetch = original;
    }
  }

  test("one PUT carries the bytes, and upload returns the path files.url gives without reading the file back", async () => {
    const { stored, requests, tools } = await uploadOn("/p/notes/", 200);
    expect(stored).toEqual({ id: "file-1", name: "a.txt", url: "/p/notes/_konae/files/file-1" });
    expect(requests).toContain("PUT /p/notes/_konae/transfers/t-1");
    expect(tools).toEqual(["transfer_begin", "files_upload", "transfer_cancel"]);
  });

  test("a transfer path that answers 404 fails the upload and cancels the transfer", async () => {
    const { stored, tools } = await uploadOn("/", 404, "Not found.");
    expect((stored as Error).message).toBe("Not found.");
    expect(tools).toEqual(["transfer_begin", "transfer_cancel"]);
  });

  test("a refusal of the bytes fails the upload with the Kernel's words and cancels the transfer", async () => {
    const { stored, tools } = await uploadOn("/", 400, "Upload size or SHA-256 mismatch.");
    expect((stored as Error).message).toBe("Upload size or SHA-256 mismatch.");
    expect(tools).toEqual(["transfer_begin", "transfer_cancel"]);
  });
});

describe("files.list returns metadata without opening file transfers", () => {
  test("adds the same-origin URL and calls only files_list", async () => {
    const calls: string[] = [];
    const window: { miniapp?: { files: { list: () => Promise<Array<{ id: string; url: string }>> } } } = {};
    const original = globalThis.fetch;
    globalThis.fetch = (async (_url: string, init: RequestInit) => {
      const request = JSON.parse(String(init.body));
      if (request.method === "notifications/initialized") return new Response(null, { status: 202 });
      if (request.method === "tools/call") {
        const name = request.params.name;
        calls.push(name);
        return Response.json({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result: [{ id: "file-1", name: "photo.png", size: 10 }] } } });
      }
      return Response.json({ jsonrpc: "2.0", id: request.id, result: {} });
    }) as typeof fetch;
    try {
      new Function("window", "location", "addEventListener", publishedSdk)(window, { pathname: "/p/gallery" }, () => {});
      await expect(window.miniapp!.files.list()).resolves.toEqual([{ id: "file-1", name: "photo.png", size: 10, url: "/p/gallery/_konae/files/file-1" }]);
      expect(calls).toEqual(["files_list"]);
    } finally {
      globalThis.fetch = original;
    }
  });
});
