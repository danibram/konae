import { Database } from "bun:sqlite";
import { MemoryStorage } from "@earendil-works/pi-durable/storage/memory";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";

const db = new Database(":memory:");
db.run("CREATE TABLE replay (seq INTEGER PRIMARY KEY, writes TEXT NOT NULL, kind TEXT NOT NULL, excerpt TEXT NOT NULL)");
const put = db.query("INSERT INTO replay (writes,kind,excerpt) VALUES(?,?,?)");
const initial = [
  {type:"conversation",value:{id:ROOT_CONVERSATION_ID}},
  {type:"entry",value:{id:2,conversationId:ROOT_CONVERSATION_ID,kind:"user",data:{baseRevision:17,baseFiles:{"index.html":"<main>Before</main>"}}}},
];
put.run(JSON.stringify(initial),"user","Make the same sample app");
db.transaction(() => {
  for (let index=1; index<1000; index++) {
    const tool = index % 10 === 7;
    const kind = tool ? "tool" : "assistant";
    const excerpt = `Fixture entry ${index} ${"a".repeat(230)}`;
    const data = tool ? {excerpt, modelVisible:"b".repeat(20000), originalDetailId:`detail-${index}`} : {excerpt};
    const writes = [{type:"entry",value:{id:index+2,conversationId:ROOT_CONVERSATION_ID,kind,data}}];
    put.run(JSON.stringify(writes),kind,excerpt);
  }
})();
const ordinaryCommit = JSON.stringify([{type:"entry",value:{
  id:1010,conversationId:ROOT_CONVERSATION_ID,kind:"tool",data:{scratchDelta:{html:"a".repeat(12000)},bounded:"b".repeat(500),original:"c".repeat(3000)},
}}]);
const rtt = 120, uploadBits = 2_000_000, downloadBits = 10_000_000;
const bytes = (body) => Buffer.byteLength(body);
const leg = (count,bps) => count * 8_000 / bps;
const server = Bun.serve({hostname:"127.0.0.1",port:0,async fetch(request) {
  const path = new URL(request.url).pathname;
  const data = await request.text();
  await Bun.sleep(rtt / 2 + leg(bytes(data),uploadBits));
  let payload;
  if (path === "/claim") payload = {generation:2};
  if (path === "/history") payload = db.query("SELECT seq,kind,excerpt FROM replay WHERE seq<=1000 ORDER BY seq DESC LIMIT 50").all();
  if (path === "/replay") payload = db.query("SELECT seq,writes FROM replay WHERE seq<=1000 ORDER BY seq").all();
  if (path === "/commit") {
    const record = put.run(data,"tool","Synthetic commit");
    payload = {seq:record.lastInsertRowid};
  }
  const body = JSON.stringify(payload);
  await Bun.sleep(rtt / 2 + leg(bytes(body),downloadBits));
  return new Response(body);
}});
async function call(path,body) {
  const response = await fetch(new URL(path,server.url),body === undefined ? {} : {method:"POST",body});
  const result = await response.text();
  return {parsed:JSON.parse(result),bytes:bytes(result)};
}
async function time(action) {
  const start=performance.now();
  const size=await action();
  return {ms:Math.round(performance.now()-start), bytes:size};
}
try {
  const output=[];
  for(let i=0;i<3;i++){
    const commit=await time(async()=>{await call("/commit",ordinaryCommit); return bytes(ordinaryCommit)});
    const history=await time(async()=>{const result=await call("/history"); if(result.parsed.length!==50) throw new Error("Unexpected page size"); return result.bytes});
    const handoff=await time(async()=>{
      await call("/claim","{}");
      const result=await call("/replay");
      const memory=new MemoryStorage();
      for(const row of result.parsed) memory.prepareCommit(JSON.parse(row.writes),row.seq).apply();
      return result.bytes;
    });
    output.push({commit,history,handoff});
  }
  console.log(JSON.stringify({profile:{rtt,uploadBits,downloadBits,entries:1000,largePayloads:100},output},null,2));
} finally {
  await server.stop();
  db.close();
}
