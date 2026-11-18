import { describe, expect, test } from "bun:test";
import { createAssistantMessageEventStream } from "@konae/pi-ai-durable";
import { forwardSanitizedStream } from "./durable-models";

function assistant(content: any[] = [], stopReason: "stop" | "error" = "stop") {
  return { role: "assistant", content, api: "anthropic-messages", provider: "anthropic", model: "fixture", usage: {}, stopReason, timestamp: Date.now() } as any;
}

describe("Durable provider event boundary", () => {
  test("streams ordinary safe text promptly and holds a credential split across chunks", async () => {
    const secret = "sk-ant-fixture-rotated-key-0123456789";
    const source = createAssistantMessageEventStream();
    const output = createAssistantMessageEventStream();
    const seen: string[] = [];
    const consume = (async () => { for await (const event of output) if (event.type === "text_delta") seen.push(event.delta); })();
    const forwarded = forwardSanitizedStream(source as any, output, [secret]);
    source.push({ type: "start", partial: assistant() } as any);
    source.push({ type: "text_start", contentIndex: 0, partial: assistant([{ type: "text", text: "" }]) } as any);
    source.push({ type: "text_delta", contentIndex: 0, delta: "A short safe answer is visible now. ", partial: assistant([{ type: "text", text: "A short safe answer is visible now. " }]) } as any);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(seen.join("")).toContain("A short safe answer is visible now.");

    source.push({ type: "text_delta", contentIndex: 0, delta: secret.slice(0, 14), partial: assistant([{ type: "text", text: "A short safe answer is visible now. " + secret.slice(0, 14) }]) } as any);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(seen.join("")).not.toContain(secret.slice(0, 14));
    source.push({ type: "text_delta", contentIndex: 0, delta: secret.slice(14), partial: assistant([{ type: "text", text: "A short safe answer is visible now. " + secret }]) } as any);
    source.push({ type: "text_end", contentIndex: 0, content: "A short safe answer is visible now. " + secret, partial: assistant([{ type: "text", text: "A short safe answer is visible now. " + secret }]) } as any);
    source.push({ type: "thinking_delta", contentIndex: 1, delta: "PRIVATE-THINKING", partial: assistant([{ type: "thinking", thinking: "PRIVATE-THINKING" }]) } as any);
    source.push({ type: "error", reason: "error", error: assistant([{ type: "thinking", thinking: "PRIVATE-ERROR-THINKING" }, { type: "text", text: `provider echoed ${secret}` }], "error"), partial: assistant([{ type: "thinking", thinking: "PRIVATE-ERROR-THINKING" }], "error") } as any);
    source.end(assistant([{ type: "thinking", thinking: "PRIVATE-FINAL-THINKING" }, { type: "text", text: `provider echoed ${secret}` }], "error"));
    await Promise.all([forwarded, consume]);
    expect(seen.join("")).not.toContain(secret);
    expect(seen.join("")).toContain("[redacted credential]");
    const final = await output.result();
    expect(JSON.stringify(final)).not.toContain(secret);
    expect(JSON.stringify(final)).not.toContain("PRIVATE-FINAL-THINKING");
  });

  test("scrubs each rotated active key at that stream's dispatch boundary", async () => {
    for (const secret of ["fixture-key-before-rotation", "fixture-key-after-rotation"]) {
      const source = createAssistantMessageEventStream();
      const output = createAssistantMessageEventStream();
      const forwarded = forwardSanitizedStream(source as any, output, [secret]);
      source.push({ type: "text_delta", contentIndex: 0, delta: `echo ${secret}`, partial: assistant([{ type: "text", text: `echo ${secret}` }]) } as any);
      source.push({ type: "text_end", contentIndex: 0, content: `echo ${secret}`, partial: assistant([{ type: "text", text: `echo ${secret}` }]) } as any);
      source.end(assistant([{ type: "text", text: `echo ${secret}` }]));
      await forwarded;
      expect(JSON.stringify(await output.result())).not.toContain(secret);
    }
  });
});
