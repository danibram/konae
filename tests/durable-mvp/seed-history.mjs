import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

export const oldestLabel = "History fixture message #0001";
export const latestLabel = "History fixture message #0995";
export const inertLabel = "<b>Konae fixture markup stays as text</b>";
export const originalTail = "Original tool response ends with fixture tail marker";
export const modelTail = "Model sees this exact bounded-response marker";
const term = "Synthetic small-app row content. Keep the numbered source order.\n";
const originalText = term.repeat(500) + originalTail;
const modelText = originalText.slice(0, 20_000) + "\n… cut at 20000 characters. Ask for a smaller result.\n" + modelTail;

/** Populate a temporary owner project via the native authenticated transport. */
export async function seedHistory({ fetcher, projectId, historyCount = 1000, title = "History fixture 1,000 entries", label = inertLabel, recordOffset = 0, prefix = "History fixture message" }) {
  const conversationId = crypto.randomUUID();
  const collection = "/_konae/projects/" + encodeURIComponent(projectId) + "/conversations";
  const path = collection + "/" + conversationId;
  const executorId = "fixture-history-seed-writer";
  function send(url, body, method = "POST") {
    return fetcher(url, { method, headers:{"content-type":"application/json"}, body:JSON.stringify(body) });
  }
  async function json(response, what) {
    const raw = await response.text();
    assert.ok(response.ok, `${what} failed with ${response.status}: ${raw.slice(0,300)}`);
    return JSON.parse(raw);
  }
  await json(await send(collection, {
    id:conversationId,formatVersion:1,piVersion:"1.5.2",registryVersion:"1",title,
  }),"Create conversation");
  const previous = await json(await fetcher(path+"/lease"),"Read project generation");
  const {generation} = await json(await send(path+"/claim",{
    expectedGeneration:previous.generation,executorId,continueExplicitly:true,
  }),"Claim fixture setup lease");

  const native = JSON.parse(await readFile(new URL("./fixtures/native.json",import.meta.url),"utf8"));
  const original = {
    original:{ response:originalText },
    effectiveArguments:{arguments:{path:"/workspace/index.html"},effectiveArguments:{path:"/workspace/index.html"},confirmation:"not-required"},
    callId:"draft-file-fixture-call",taskId:12,
    truncation:{ method:"head",originalLength:originalText.length,retainedLength:modelText.length,truncated:true,redacted:false },
  };
  const bytes = new TextEncoder().encode(JSON.stringify(original));
  const digest = createHash("sha256").update(bytes).digest("hex");
  const bulk = await fetcher(path+"/blobs/"+digest,{method:"PUT",headers:{"content-type":"application/octet-stream"},body:bytes});
  assert.ok(bulk.ok, "Fixture writes its immutable large detail");
  const resultEntry = native.batches.flat().find(({type,value})=>type==="entry"&&value.kind==="pi.tool-result").value;
  if (recordOffset) for (const record of native.batches.flat()) if (record.type === "entry") {
    record.value.id += recordOffset;
    if (typeof record.value.head === "number") record.value.head += recordOffset;
  }
  resultEntry.model[0].content=[{type:"text",text:modelText}];
  const detailReference = { kind:"immutable-detail", digest, byteLength:bytes.byteLength };
  resultEntry.model[0].details=detailReference;
  resultEntry.data={ ...resultEntry.data, details:detailReference };

  const core = native.batches.flat().filter(({type,value})=>type==="entry"&&value.model?.[0]?.role!=="system").length;
  const supplemental = historyCount-core;
  assert.ok(supplemental>1);
  const fillers = Array.from({length:supplemental},(_,offset)=>{
    const number=offset+1;
    const text=number===supplemental ? label : prefix+" #"+String(number).padStart(4,"0");
    return [{type:"entry",value:{
      id:100+offset+recordOffset,conversationId:1,kind:"konae.owner-message",
      model:[{role:"user",content:text,timestamp:1700000000000+offset}],
    }}];
  });
  const [root,...sample] = native.batches;
  const batches = [root,...fillers,...sample];
  let expectedSeq=1;
  try {
    for(let offset=0;offset<batches.length;offset+=48){
      const writes=batches.slice(offset,offset+48);
      const receipt=await json(await send(path+"/commit",{
        requestId:"history-fixture-"+offset,generation,expectedSeq,writes,
      }),"Store timeline batch");
      assert.equal(receipt.stored,writes.length,JSON.stringify(receipt).slice(0,300));
      assert.equal(receipt.lastSeq,expectedSeq+writes.length-1);
      expectedSeq=receipt.lastSeq+1;
    }
  } finally { await json(await send(path+"/release",{generation,executorId}),"Release fixture setup"); }
  return { path, conversationId, generation, digest, originalText, modelText, toolEntryId:resultEntry.id, total:historyCount };
}
