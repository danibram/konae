import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { chromium, webkit, devices } from "playwright";

const root = path.resolve("browser-dist");
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url ?? "/", "http://127.0.0.1").pathname;
  const file = pathname === "/" ? path.join(root, "index.html") : path.join(root, pathname);
  try {
    await stat(file);
    response.setHeader("content-type", file.endsWith(".js") ? "text/javascript" : "text/html");
    response.end(await readFile(file));
  } catch {
    if (pathname === "/") {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><button id="run">Load</button><pre id="result">idle</pre><script type="module" src="/entry.js"></script>');
    } else { response.statusCode = 404; response.end("not found"); }
  }
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
if (!address || typeof address === "string") throw new Error("no test server address");
const baseURL = `http://127.0.0.1:${address.port}`;
const devicesToRun = [
  { name: "Chromium desktop", browserType: chromium, options: {} },
  { name: "WebKit iPhone 13 emulation", browserType: webkit, options: devices["iPhone 13"] },
];
const results: unknown[] = [];
try {
  for (const target of devicesToRun) {
    const browser = await target.browserType.launch({ headless: true });
    const context = await browser.newContext(target.options);
    const page = await context.newPage();
    const requests: Array<{ url: string; model?: string; authorization?: boolean }> = [];
    await page.route("**/*", async (route) => {
      const req = route.request();
      if (req.url().startsWith(baseURL)) return route.continue();
      let body: Record<string, unknown> = {};
      try { body = req.postDataJSON() as Record<string, unknown>; } catch { /* fixture provider request */ }
      const provider = req.url().includes("anthropic") ? "anthropic" : req.url().includes("openai.com") ? "openai" : "openrouter";
      const headers = req.headers();
      const suppliedKey = headers["x-api-key"] ?? headers.authorization?.replace(/^Bearer /, "");
      requests.push({ url: req.url(), model: typeof body.model === "string" ? body.model : undefined, authorization: Boolean(suppliedKey), keyInjected: suppliedKey === `fixture-key-${provider}` });
      const url = new URL(req.url());
      let stream = "";
      if (url.hostname.includes("anthropic") || url.pathname.includes("messages")) {
        stream = [
          ['message_start', { type: 'message_start', message: { id: 'msg_fixture', type: 'message', role: 'assistant', model: 'fixture', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 0 } } }],
          ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }],
          ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'fixture-ok' } }],
          ['content_block_stop', { type: 'content_block_stop', index: 0 }],
          ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 2 } }],
          ['message_stop', { type: 'message_stop' }],
        ].map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`).join("");
      } else if (url.pathname.includes("responses")) {
        const events = [
          { type: 'response.created', response: { id: 'resp_fixture', object: 'response', created_at: 1, status: 'in_progress', model: 'fixture', output: [], usage: null } },
          { type: 'response.output_item.added', output_index: 0, item: { id: 'msg_fixture', type: 'message', role: 'assistant', content: [] } },
          { type: 'response.content_part.added', item_id: 'msg_fixture', output_index: 0, content_index: 0, part: { type: 'output_text', text: '' } },
          { type: 'response.output_text.delta', item_id: 'msg_fixture', output_index: 0, content_index: 0, delta: 'fixture-ok' },
          { type: 'response.output_text.done', item_id: 'msg_fixture', output_index: 0, content_index: 0, text: 'fixture-ok' },
          { type: 'response.content_part.done', item_id: 'msg_fixture', output_index: 0, content_index: 0, part: { type: 'output_text', text: 'fixture-ok' } },
          { type: 'response.output_item.done', output_index: 0, item: { id: 'msg_fixture', type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'fixture-ok', annotations: [] }] } },
          { type: 'response.completed', response: { id: 'resp_fixture', object: 'response', created_at: 1, status: 'completed', model: 'fixture', output: [{ id: 'msg_fixture', type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'fixture-ok', annotations: [] }] }], usage: { input_tokens: 1, output_tokens: 2, total_tokens: 3 } } },
        ];
        stream = events.map((data) => `event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`).join("");
      } else {
        stream = [
          { id: 'chatcmpl_fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta: { role: 'assistant', content: 'fixture-ok' }, finish_reason: null }] },
          { id: 'chatcmpl_fixture', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] },
        ].map((data) => `data: ${JSON.stringify(data)}\n\n`).join("") + "data: [DONE]\n\n";
      }
      if ((req.headers()["x-api-key"] ?? req.headers().authorization ?? "").includes("fixture-key-openai")) await new Promise((resolve) => setTimeout(resolve, 250));
      await route.fulfill({ status: 200, headers: { "content-type": "text/event-stream", "access-control-allow-origin": "*", "access-control-allow-headers": "*", "access-control-allow-methods": "GET,POST,OPTIONS" }, body: stream });
    });
    const observedRequests: string[] = [];
    page.on("request", (request) => observedRequests.push(request.url()));
    await page.goto(baseURL);
    const beforeClick = [...observedRequests];
    await page.evaluate(() => {
      for (const provider of ["anthropic", "openai", "openrouter"]) localStorage.setItem(`konae:agent-key:${provider}`, `fixture-key-${provider}`);
    });
    const requestsBeforeClick = observedRequests.length;
    const runtimeChunksBeforeClick = observedRequests.filter((url) => url.includes("runtime-")).length;
    await page.locator("#run").click();
    await page.waitForFunction(() => document.querySelector("#result")?.textContent?.includes('"outputs"') || document.querySelector("#result")?.textContent?.includes('"error"'), { timeout: 30000 });
    const text = await page.locator("#result").textContent();
    results.push({ browser: target.name, version: browser.version(), viewport: page.viewportSize(), result: text ? JSON.parse(text) : null, requests, initialRequests: beforeClick, lazyBeforeClick: requestsBeforeClick > 0 && runtimeChunksBeforeClick === 0 && !beforeClick.some((url) => /api\.anthropic|api\.openai|openrouter|runtime-/i.test(url)) });
    await context.close();
    await browser.close();
  }
} finally {
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}
console.log(JSON.stringify(results, null, 2));
