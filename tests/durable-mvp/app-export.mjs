import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { testInstallation } from "./harness.mjs";

const repo = process.argv[2];
const { McpClient } = await import(pathToFileURL(join(repo,"src/mcp/client.ts")).href);
const { readTar } = await import(pathToFileURL(join(repo,"src/lib/tar.ts")).href);
const { release } = await import(pathToFileURL(join(repo,"tests/kernel-harness.ts")).href);
const fixture = await testInstallation(join(repo,"kernel-rs/target/debug/konae"));
try {
  const {cookie}=await fixture.owner("export");
  const mcp = new McpClient(`${fixture.origin}/mcp`,{fetch:(input,init)=>{
    const headers=new Headers(init?.headers); headers.set("cookie",cookie);
    return fetch(input,{...init,headers});
  }});
  const projectId = "fixture-export-project";
  await mcp.call("project_create",{projectId,name:"Export fixture"});
  assert.equal((await mcp.call("project_inspect",{projectId})).project.id,projectId);
  const conversationId=crypto.randomUUID();
  const path=`/_konae/projects/${projectId}/conversations`;
  const href=path+"/"+conversationId;
  const post=async(href,body)=>{
    const result=await fixture.request(href,{method:"POST",cookie,body});
    assert.equal(result.status,200,result.text.slice(0,300));
    return result.json;
  };
  const privateTitle="Private evidence marker gate27";
  await post(path,{id:conversationId,title:privateTitle,formatVersion:1,piVersion:"1.5.2",registryVersion:"1"});
  const executorId="export-fixture-writer";
  const {generation}=await post(href+"/claim",{expectedGeneration:0,executorId,continueExplicitly:true});
  await post(href+"/commit",{requestId:"export-case",expectedSeq:1,generation,writes:[[{type:"entry",value:{id:2,conversationId:1,kind:"pi.user",model:[{role:"user",content:privateTitle}]}}]]});
  await post(href+"/release",{generation,executorId});
  const privateDownload=await fixture.request(href+"/download",{cookie});
  assert.equal(privateDownload.status,200);
  assert.ok(privateDownload.text.includes(privateTitle));
  const source = {
    project:{id:projectId,name:"Export fixture",slug:"gate27-export",manifest:{version:1,visibility:"public"},files:{
      "schema.sql":"CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);",
      "actions.json":JSON.stringify({version:1,actions:{"notes.list":{kind:"query",access:"anonymous",statement:"SELECT id,body FROM notes LIMIT 50",bindings:[]}}}),
    }},
    visibility:"public",
    artifact:{version:1,entrypoint:"index.html",files:{"index.html":"<!doctype html><main>Local Export Fixture</main>"}},
  };
  await release(mcp,source,projectId);
  const readback=await mcp.call("project_inspect",{projectId});
  assert.ok(readback.deployment);
  const exported=await mcp.call("project_export",{projectId});
  try {
    const entries=readTar(await mcp.download(exported.folder));
    assert.ok(entries.some(({path})=>path==="konae.json"));
    assert.ok(entries.some(({path})=>path==="dist/index.html"));
    const markers=[privateTitle,conversationId,"conversation_records","conversation_live_authority"];
    for(const {path,contents} of entries){
      assert.ok(!path.includes("mcp.sqlite")&&!path.includes("conversation"),path);
      assert.ok(markers.every((marker)=>!new TextDecoder().decode(contents).includes(marker)),path);
    }
    for(const {name} of exported.manifest.contents) {
      const bytes=await mcp.download(exported.downloads[name]);
      assert.ok(markers.every((marker)=>!new TextDecoder().decode(bytes).includes(marker)),name);
    }
  } finally {
    for(const transfer of [exported.folder,...Object.values(exported.downloads)]) await mcp.cancel(transfer.transferId).catch(()=>{});
  }
  console.log("Real app Export and separate owner download were inspected. App sources, artifact, and every exported data component exclude the owner conversation.");
} finally { await fixture.close(); }
