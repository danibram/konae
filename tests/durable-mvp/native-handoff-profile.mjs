import assert from "node:assert/strict";
import { MemoryStorage } from "@earendil-works/pi-durable/storage/memory";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";
import { testInstallation } from "./harness.mjs";

const fixture=await testInstallation(process.argv[2]);
try {
  const writer=await fixture.owner("native-profile");
  const next=await fixture.owner("native-profile",true);
  const projectId="native-mobile-profile";
  await fixture.mcp(writer.cookie,"project_create",{projectId,name:"Native mobile profile fixture"});
  const recipient=crypto.randomUUID();
  const collection="/_konae/projects/"+projectId+"/conversations";
  const entry=collection+"/"+recipient;
  const firstExecutor="native-profile-writer";
  const nextExecutor="native-profile-next";
  const okay=async(res)=>{
    const value=await res;
    assert.ok(value.status>=200 && value.status<300,`Status ${value.status}: ${value.text.slice(0,140)}`);
    return value;
  };
  const request=(cookie,path,method="GET",body)=>fixture.request(path,{cookie,method,body});
  await okay(request(writer.cookie,collection,"POST",{id:recipient,title:"Synthetic #26 wire profile",formatVersion:1,piVersion:"1.5.2",registryVersion:"1"}));
  const {generation}=(await okay(request(writer.cookie,entry+"/claim","POST",{expectedGeneration:0,executorId:firstExecutor,continueExplicitly:true}))).json;
  const data=[[
    {type:"conversation",value:{id:ROOT_CONVERSATION_ID}},
    {type:"entry",value:{id:2,conversationId:ROOT_CONVERSATION_ID,kind:"user",data:{baseRevision:17,baseFiles:{"index.html":"<main>Before</main>"}}}},
  ]];
  for(let index=1; index<1000; index++){
    const tool=index%10===7;
    const excerpt="Fixture entry "+index+" "+"a".repeat(230);
    data.push([{type:"entry",value:{id:index+2,conversationId:ROOT_CONVERSATION_ID,kind:tool?"tool":"assistant",data:tool?{excerpt,modelVisible:"b".repeat(20000),originalDetailId:"detail-"+index}:{excerpt}}}]);
  }
  for(let offset=0;offset<data.length;offset+=48) {
    const writes=data.slice(offset,offset+48);
    const response=await okay(request(writer.cookie,entry+"/commit","POST",{requestId:"profile-"+offset,generation,expectedSeq:offset+1,writes}));
    assert.equal(response.json.stored,writes.length);
  }
  await okay(request(writer.cookie,entry+"/release","POST",{generation,executorId:firstExecutor}));
  let bytes=0, calls=0;
  const pageSizes=[];
  const shaped=async(path,method,body)=>{
    await Bun.sleep(60+(body?Buffer.byteLength(JSON.stringify(body))*8/2000:0));
    const response=await okay(request(next.cookie,path,method,body));
    const size=Buffer.byteLength(response.text);
    bytes+=size;calls++;
    if(path.includes("/batches?")) {
      pageSizes.push(size);
      if(maxBytes) assert.ok(size<=Number(maxBytes) || response.json.batches.length===1,"a page may exceed its target only for one atomic batch");
    }
    await Bun.sleep(60+size*8/10000);
    return response.json;
  };
  const started=performance.now();
  const lease=await shaped(entry+"/claim","POST",{expectedGeneration:generation,executorId:nextExecutor,continueExplicitly:true});
  assert.equal(lease.generation,generation+1);
  const memory=new MemoryStorage();
  const pageLimit = process.env.KONAE_PROFILE_NATIVE_LIMIT ?? "50";
  const maxBytes = process.env.KONAE_PROFILE_NATIVE_BYTES;
  let seq=0;
  while(seq<data.length){
    const query = new URLSearchParams({limit:pageLimit,after:String(seq),...(maxBytes ? {maxBytes} : {})});
    const {batches}=await shaped(entry+"/batches?"+query);
    assert.ok(batches.length>0);
    for(const {writes,sequence} of batches){
      assert.equal(sequence,++seq);
      memory.prepareCommit(writes,sequence).apply();
    }
  }
  const durationMs=performance.now()-started;
  assert.equal(seq,1000);
  console.log(JSON.stringify({scope:"authenticated claim and paged native replay through SDK MemoryStorage only",profile:{rtt:120,uploadMbps:2,downloadMbps:10},limit:Number(pageLimit),maxBytes:maxBytes?Number(maxBytes):null,calls,bytes,pageSizes,ms:Math.round(durationMs)}));
} finally { await fixture.close(); }
