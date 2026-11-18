import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable/storage/memory";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";
import { registerStorageConformance } from "@earendil-works/pi-durable/testing";

function backend() {
  const db = new Database(":memory:");
  db.run("CREATE TABLE batch (seq INTEGER PRIMARY KEY, request TEXT NOT NULL, writes TEXT NOT NULL); CREATE TABLE id (n INTEGER NOT NULL); INSERT INTO id VALUES (1)");
  const persist = db.transaction((request, batches) => {
    for (const [seq, writes] of batches) db.run("INSERT INTO batch VALUES (?, ?, ?)", [seq, request, JSON.stringify(writes)]);
  });
  return {
    loseNextReceipt: false,
    log: () => db.query("SELECT seq,writes FROM batch ORDER BY seq").all(),
    receipt: (id) => db.query("SELECT seq FROM batch WHERE request=? ORDER BY seq").all(id),
    mint: (candidate) => {
      const next = db.query("UPDATE id SET n=max(n+1,?) WHERE n < 9007199254740991 RETURNING n").get(candidate)?.n;
      if (!Number.isSafeInteger(next)) throw new Error("ID namespace exhausted");
      return next;
    },
    append(request, batches) {
      persist(request, batches);
      if (this.loseNextReceipt) {
        this.loseNextReceipt = false;
        throw new Error("transport lost acknowledgement");
      }
      return this.receipt(request);
    },
    close: () => db.close(),
  };
}

function open(remote, afterAppend = undefined) {
  const memory = new MemoryStorage();
  for (const { seq, writes } of remote.log()) memory.prepareCommit(JSON.parse(writes), seq).apply();
  let tail = Promise.resolve();
  const ordered = (run) => {
    const result = tail.then(run);
    tail = result.then(() => {}, () => {});
    return result;
  };
  return new Proxy(memory, {
    get(target, prop) {
      if (prop === "commit") return (batches, receive) => ordered(async () => {
        const accepted = [];
        let refused;
        for (const batch of batches) {
          try {
            const prepared = memory.prepareCommit(batch);
            prepared.apply();
            accepted.push([prepared.seq, prepared.writes]);
          } catch (error) {
            refused = error;
            break;
          }
        }
        if (accepted.length) {
          const request = crypto.randomUUID();
          let receipts;
          try {
            receipts = remote.append(request, accepted);
          } catch (error) {
            receipts = remote.receipt(request);
            if (receipts.length !== accepted.length) {
              await memory.close(BACKGROUND_CONTEXT);
              throw error;
            }
          }
          await afterAppend?.();
          receipts.forEach(({ seq }, index) => receive(index, seq));
        }
        return { stored: accepted.length, ...(refused ? { refused } : {}) };
      });
      if (prop === "mintId") return () => ordered(async () => {
        const candidate = await memory.mintId();
        return remote.mint(candidate);
      });
      const fn = Reflect.get(target, prop, target);
      return typeof fn === "function" ? (...args) => ordered(() => fn.apply(target, args)) : fn;
    },
  });
}

registerStorageConformance({ describe, expect, it }, "SQLite-backed native Pi batch projection", async (test) => {
  const remote = backend();
  const first = open(remote);
  try { await test(first); } finally { await first.close(BACKGROUND_CONTEXT); remote.close(); }
});

it("checks receipt on lost response and restores the same immutable entry without a duplicate", async () => {
  const remote = backend();
  const storage = open(remote);
  remote.loseNextReceipt = true;
  try {
    const id = await storage.mintId();
    const entryBatch = [
      { type: "conversation", value: { id: ROOT_CONVERSATION_ID } },
      { type: "entry", value: { id, conversationId: ROOT_CONVERSATION_ID, kind: "fixture", data: { scratch: "after", baseRevision: 3, result: "actual" } } },
    ];
    const seen = [];
    expect(await storage.commit([entryBatch], (index, seq) => seen.push([index, seq]), BACKGROUND_CONTEXT)).toEqual({ stored: 1 });
    expect(remote.log()).toHaveLength(1);
    expect(seen).toHaveLength(1);
    await storage.close(BACKGROUND_CONTEXT);
    const recovered = open(remote);
    expect((await recovered.entry(id, BACKGROUND_CONTEXT)).entry).toEqual(entryBatch[1].value);
    expect(await recovered.mintId()).toBeGreaterThan(id);
    await recovered.close(BACKGROUND_CONTEXT);
  } finally { remote.close(); }
});

import { createRegistry, defineExtension, defineTool, Harness, ToolResultEntry } from "@earendil-works/pi-durable";
import { createModels, fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import { obj } from "@earendil-works/pi-schema";

it("commits an actual built-in tool result with an immutable scratch delta and reopens the same model context read-only", async () => {
  const remote = backend();
  const models = createModels();
  const faux = fauxProvider();
  models.setProvider(faux.provider);
  let nextModelMessage;
  faux.setResponses([
    fauxAssistantMessage(fauxToolCall("edit_file", {}, {id:"edit-fixture"}), {stopReason:"toolUse"}),
    (request) => {
      nextModelMessage = request.messages.find((entry) => entry.role === "toolResult");
      return fauxAssistantMessage("The fixture edit is ready.");
    }
  ]);
  const original = {text:"o".repeat(20001), refusal:{code:"fixture-refusal",detail:"fixture detail"}};
  const modelText = original.text.slice(0, 20_000);
  const files = {baseRevision:17, delta:{"index.html":"<main>After</main>"}};
  const edit = defineTool({
    name:"edit_file", description:"Return an immutable file delta as part of its tool result.", parameters:obj({}),
    replay:"unsafe",
    execute: () => ({
      output:[{type:"text",text:modelText}],
      details:{original,files,truncation:{method:"head",originalLength:20001,retainedLength:20000}},
    }),
  });
  const extension = defineExtension({name:"fixture",tools:[edit]});
  const registry = createRegistry();
  registry.install(extension);
  const options = {models,registry};
  const harness = await Harness.open(open(remote),options,BACKGROUND_CONTEXT);
  try {
    const root = await harness.root(BACKGROUND_CONTEXT,{agent:{model:{provider:"faux",modelId:"faux-1"},extensions:[extension]}});
    const submission = await root.submit({type:"input",content:"Edit the fixture", requestId:"file-delta"},BACKGROUND_CONTEXT);
    await submission.wait(BACKGROUND_CONTEXT);
    const entry = (await root.entries({},50,undefined,BACKGROUND_CONTEXT)).items.find((item)=>ToolResultEntry.is(item));
    expect(entry).toBeDefined();
    expect(nextModelMessage).toEqual(entry.model[0]);
    expect(entry.model[0].content[0].text).toEqual(modelText);
    expect(entry.model[0].details).toEqual({original,files,truncation:{method:"head",originalLength:20001,retainedLength:20000}});
    const commits = remote.log().filter(({writes}) => JSON.parse(writes).some((write)=>write.type==="entry" && write.value.id===entry.id));
    expect(commits).toHaveLength(1);
    expect(commits[0].writes).toContain("baseRevision");
    expect(commits[0].writes).toContain("<main>After</main>");
    await harness.close(BACKGROUND_CONTEXT);
    const viewer = await Harness.open(open(remote), {...options, readOnly:true},BACKGROUND_CONTEXT);
    try {
      const recovered = await viewer.root(BACKGROUND_CONTEXT);
      const context = await recovered.context(BACKGROUND_CONTEXT);
      expect(context.messages).toContainEqual(nextModelMessage);
      expect(context.entries.find((item)=>item.id===entry.id).model[0].details).toEqual(entry.model[0].details);
      expect(() => viewer.resume()).toThrow();
      expect(faux.state.callCount).toBe(2);
    } finally {
      await viewer.close(BACKGROUND_CONTEXT);
    }
  } finally {
    await harness.close(BACKGROUND_CONTEXT);
    remote.close();
  }
});

import { defineTask } from "@earendil-works/pi-durable";

it("unsupported task versions stay inspectable and are not executed", async () => {
  const remote = backend();
  let executions = 0;
  const definition = (version) => defineTask({name:"fixture.future",version,initial:()=>({phase:"main"}),phases:{
    main:async (_task,runtime,context)=>{
      executions++;
      await runtime.commit(()=>({status:"terminal",outcome:{status:"completed",result:null}}),context);
    },
  }});
  const models = createModels();
  const options = (version) => {
    const task = definition(version);
    const registry = createRegistry();
    registry.install(defineExtension({name:"future-fixture",tasks:[task]}));
    return {models,registry,task};
  };
  const newer = options(2);
  const first = await Harness.open(open(remote),newer,BACKGROUND_CONTEXT);
  try {
    const root = await first.root(BACKGROUND_CONTEXT);
    await root.commit((tx)=>tx.createTask(newer.task,{}, {ownership:{kind:"conversation"}}),BACKGROUND_CONTEXT);
    expect((await first.inspect(BACKGROUND_CONTEXT)).scheduling).toBe("paused");
    await first.close(BACKGROUND_CONTEXT);
    const older = options(1);
    const resumed = await Harness.open(open(remote),older,BACKGROUND_CONTEXT);
    try {
      resumed.resume();
      const pending = (await resumed.inspect(BACKGROUND_CONTEXT)).tasks.find(({record})=>record.kind==="fixture.future");
      expect(pending.record.version).toBe(2);
      expect(pending.state).toEqual({kind:"blocked",reason:"task_too_old"});
      expect(executions).toBe(0);
    } finally { await resumed.close(BACKGROUND_CONTEXT); }
  } finally { await first.close(BACKGROUND_CONTEXT); remote.close(); }
});

it("storage commit failure closes the Harness and refuses new shared work", async () => {
  const memory = new MemoryStorage();
  let failNext = false;
  const storage = new Proxy(memory, {
    get(target, prop) {
      if (prop === "commit") return async (...args) => {
        if (failNext) {
          failNext = false;
          throw new Error("fixture storage unavailable");
        }
        return target.commit(...args);
      };
      const value = Reflect.get(target, prop, target);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
  const models = createModels();
  const harness = await Harness.open(storage, { models, registry: createRegistry() }, BACKGROUND_CONTEXT);
  try {
    const root = await harness.root(BACKGROUND_CONTEXT);
    failNext = true;
    await expect(root.submit({ type: "input", content: "must not dispatch", requestId: "failed-commit" }, BACKGROUND_CONTEXT)).rejects.toThrow("fixture storage unavailable");
    const end = await harness.closed;
    expect(end.kind).toBe("failed");
  } finally {
    await harness.close(BACKGROUND_CONTEXT);
  }
});
