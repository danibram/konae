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
  new Function("window", "addEventListener", "parent", "document", source)(window, () => {}, undefined, undefined);
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
    expect(calls).toEqual(["auth.session", "auth.signIn", "auth.signOut", "actions.call", "files.list", "files.upload", "files.url", "files.update", "files.delete"]);
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
  const kernel = readdirSync(path.join(root, "kernel-rs/src")).filter((file) => file.endsWith(".rs")).map((file) => read(`kernel-rs/src/${file}`)).join("\n");
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
    const mcp = read("kernel-rs/src/mcp.rs");
    // platform_guide's topics live in guide.rs; mcp.rs hands the call to it.
    expect(mcp).toContain(`"platform_guide" => super::guide::topic(`);
    expect(read("kernel-rs/src/guide.rs")).toContain(`"sdk" => serde_json::from_str::<Value>(include_str!("sdk-guide.json"))?,`);
    expect(drift(namedCalls(mcp)).notInContract).toEqual([]);
    const guide = JSON.parse(read(renderedPaths.guide)) as { modules: { calls: { call: string }[] }[] };
    expect(drift(guide.modules.flatMap((module) => module.calls.map((call) => call.call.replace(/^miniapp\./, ""))))).toEqual(agreed);
  });
});
