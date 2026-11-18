import assert from "node:assert/strict";
import { chromium, webkit, devices } from "playwright";

const repo = process.argv[2];
if (!repo) throw new Error("Pass the Konae checkout path.");
const probe = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response("ok") });
const origin = `http://127.0.0.1:${probe.port}`;
probe.stop(true);
const vite = Bun.spawn([process.execPath, "run", "dev", "--", "--host", "127.0.0.1", "--port", String(new URL(origin).port), "--strictPort"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
const keys = { anthropic: "fixture-anthropic-key-28", openai: "fixture-openai-key-28", openrouter: "fixture-openrouter-key-28" };

function sse(provider, key) {
  if (provider === "anthropic") return [
    ["message_start", { type: "message_start", message: { id: "anthropic-fixture", type: "message", role: "assistant", model: "fixture", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 0 } } }],
    ["content_block_start", { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } }],
    ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "DO-NOT-STORE-THINKING" } }],
    ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "signature_delta", signature: "opaque-fixture-signature" } }],
    ["content_block_stop", { type: "content_block_stop", index: 0 }],
    ["content_block_start", { type: "content_block_start", index: 1, content_block: { type: "text", text: "" } }],
    ["content_block_delta", { type: "content_block_delta", index: 1, delta: { type: "text_delta", text: `echo ${key}` } }],
    ["content_block_stop", { type: "content_block_stop", index: 1 }],
    ["message_delta", { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 2 } }],
    ["message_stop", { type: "message_stop" }],
  ].map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`).join("");
  if (provider === "openai") return [
    { type: "response.created", response: { id: "response-fixture", object: "response", created_at: 1, status: "in_progress", model: "fixture", output: [], usage: null } },
    { type: "response.output_item.added", output_index: 0, item: { id: "message-fixture", type: "message", role: "assistant", content: [] } },
    { type: "response.content_part.added", item_id: "message-fixture", output_index: 0, content_index: 0, part: { type: "output_text", text: "" } },
    { type: "response.output_text.delta", item_id: "message-fixture", output_index: 0, content_index: 0, delta: `echo ${key}` },
    { type: "response.output_text.done", item_id: "message-fixture", output_index: 0, content_index: 0, text: `echo ${key}` },
    { type: "response.content_part.done", item_id: "message-fixture", output_index: 0, content_index: 0, part: { type: "output_text", text: `echo ${key}` } },
    { type: "response.output_item.done", output_index: 0, item: { id: "message-fixture", type: "message", role: "assistant", content: [{ type: "output_text", text: `echo ${key}`, annotations: [] }] } },
    { type: "response.completed", response: { id: "response-fixture", object: "response", created_at: 1, status: "completed", model: "fixture", output: [{ id: "message-fixture", type: "message", role: "assistant", content: [{ type: "output_text", text: `echo ${key}`, annotations: [] }] }], usage: { input_tokens: 1, output_tokens: 2, total_tokens: 3 } } },
  ].map((data) => `event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`).join("");
  return [
    { id: "chatcmpl-fixture", object: "chat.completion.chunk", created: 1, model: "fixture", choices: [{ index: 0, delta: { role: "assistant", content: `echo ${key}` }, finish_reason: null }] },
    { id: "chatcmpl-fixture", object: "chat.completion.chunk", created: 1, model: "fixture", choices: [{ index: 0, delta: {}, finish_reason: "stop" }] },
  ].map((data) => `data: ${JSON.stringify(data)}\n\n`).join("") + "data: [DONE]\n\n";
}

try {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline && !(await fetch(origin).then((response) => response.ok, () => false))) {
    if (vite.exitCode !== null) throw new Error("Vite exited during provider smoke startup.");
    await Bun.sleep(100);
  }
  for (const [browserType, device] of [[chromium, {}], [webkit, devices["iPhone 13"]]]) {
    const browser = await browserType.launch();
    try {
      const context = await browser.newContext(device);
      await context.route(origin + "/__provider_probe", route => route.fulfill({ contentType: "text/html", body: "<!doctype html><title>Provider adapter fixture</title>" }));
      const seen = [];
      await context.route("**/*", async (route) => {
        const url = route.request().url();
        if (url.startsWith(origin)) return route.continue();
        if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: { "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-allow-methods": "GET,POST,OPTIONS" } });
        const provider = url.includes("anthropic") ? "anthropic" : url.includes("openai.com") ? "openai" : "openrouter";
        const headers = route.request().headers();
        const supplied = headers["x-api-key"] ?? headers.authorization?.replace(/^Bearer /, "");
        let body = {};
        try { body = route.request().postDataJSON(); } catch { /* fixture request without JSON */ }
        seen.push({ provider, authorized: supplied === keys[provider], model: body.model });
        const responseBody = sse(provider, keys[provider]);
        await route.fulfill({ status: 200, headers: { "content-type": "text/event-stream", "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-allow-methods": "GET,POST,OPTIONS" }, body: responseBody });
      });
      const page = await context.newPage();
      await page.goto(origin + "/__provider_probe");
      const result = await page.evaluate(async (keys) => {
        const { createDurableModels, modelsWithConfiguredModel, durableModelFor } = await import("/src/agent/durable-models.ts");
        const outputs = {};
        for (const provider of ["anthropic", "openai", "openrouter"]) {
          const keyStore = { getKey: (id) => keys[id] ?? null, setKey() {}, clearKey() {} };
          const models = createDurableModels(keyStore);
          const config = { provider, modelId: "fixture-unqualified-custom" };
          const model = durableModelFor(config, models);
          const streamModels = modelsWithConfiguredModel(models, config, keyStore);
          const stream = streamModels.streamSimple(model, { messages: [{ role: "user", content: "Return the fixture reply." }] });
          const deltas = [];
          for await (const event of stream) {
            if (event.type === "text_delta") deltas.push(event.delta);
            if (event.type.startsWith("thinking")) throw new Error("thinking event escaped the provider boundary");
          }
          const response = await stream.result();
          outputs[provider] = { model: model.id, id: response.id, text: deltas.join(""), final: response.content };
        }
        return outputs;
      }, keys);
      assert.equal(seen.length, 3);
      assert.ok(seen.every((item) => item.authorized && item.model === "fixture-unqualified-custom"));
      for (const provider of ["anthropic", "openai", "openrouter"]) {
        const encoded = JSON.stringify(result[provider]);
        assert.ok(!encoded.includes(keys[provider]), `${provider} active key was persisted in stream/result data`);
        assert.ok(!encoded.includes("DO-NOT-STORE-THINKING"));
        assert.ok(!encoded.includes("opaque-fixture-signature"));
      }
      console.log(browserType.name(), browser.version(), "actual Durable model adapters: Anthropic/OpenAI/OpenRouter, local keys, custom IDs and stream redaction passed");
      await context.close();
    } finally { await browser.close(); }
  }
} finally { vite.kill(); await vite.exited; }
