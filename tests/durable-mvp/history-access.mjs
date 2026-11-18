import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { testInstallation } from "./harness.mjs";

const repo=process.argv[2];
const { McpClient } = await import(pathToFileURL(join(repo,"src/mcp/client.ts")).href);
const { release, account, setSignupPolicy } = await import(pathToFileURL(join(repo,"tests/kernel-harness.ts")).href);
const fixture = await testInstallation(process.argv[3]);
try {
  const alice=await fixture.owner("access-owner-a");
  const bob=await fixture.owner("access-owner-b");
  const mcp = new McpClient(fixture.origin+"/mcp",{fetch:(input,init)=>{
    const headers=new Headers(init?.headers);
    headers.set("cookie",alice.cookie);
    return fetch(input,{...init,headers});
  }});
  const projectId="owner-history-access-fixture";
  await mcp.call("project_create",{projectId,name:"History access fixture"});
  const payload={
    project:{id:projectId,name:"History access fixture",slug:"history-access",manifest:{version:1,visibility:"public"},files:{
      "schema.sql":"CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY);",
      "actions.json":JSON.stringify({version:1,actions:{"items.list":{kind:"query",access:"anonymous",statement:"SELECT id FROM items LIMIT 1",bindings:[]}}}),
    }},
    visibility:"public",
    artifact:{version:1,entrypoint:"index.html",files:{"index.html":"<!doctype html><main>App account fixture</main>"}},
  };
  const {receipt}=await release(mcp,payload,projectId);
  const before=await mcp.call("modules_read",{projectId});
  if (!before.modules.auth.state.on) await mcp.call("module_set",{projectId,module:"auth",on:true,expected:before.modules.auth.state});
  await setSignupPolicy(mcp,projectId,{mode:"open"});
  const appCookie=await account({url:fixture.origin},receipt.hostname,"signup","app-actor@fixture.test");
  assert.match(appCookie,/konae_app_session/);
  const {token}=await mcp.call("token_create",{scopes:["read"],projectIds:[projectId],ttlSeconds:600});
  assert.equal((await new McpClient(fixture.origin+"/mcp",{token}).call("project_inspect",{projectId})).project.id,projectId);

  const base="/_konae/projects/"+projectId+"/conversations";
  const id=crypto.randomUUID();
  const href=base+"/"+id;
  const request=async(path,method,body)=>fixture.request(path,{cookie:alice.cookie,method,body});
  const ok=async(response,label)=>{
    const result=await response;
    assert.ok(result.status>=200 && result.status<300,label+" returned "+result.status);
    return result;
  };
  await ok(request(base,"POST",{id,title:"owner-only synthetic marker",formatVersion:1,piVersion:"1.5.2",registryVersion:"1"}),"create");
  const {generation}=(await ok(request(href+"/claim","POST",{expectedGeneration:0,executorId:"auth-fixture-executor",continueExplicitly:true}),"claim")).json;
  const raw=new TextEncoder().encode("owner-only synthetic marker");
  const digest=new Bun.CryptoHasher("sha256").update(raw).digest("hex");
  await ok(request(href+"/blobs/"+digest,"PUT",raw),"immutable blob");
  const result=(await ok(request(href+"/commit","POST",{
    requestId:"fixture-owner-native",
    expectedSeq:1,
    generation,
    writes:[[{type:"conversation",value:{id:1}}],[{type:"entry",value:{id:2,conversationId:1,kind:"pi.user",model:[{role:"user",content:"owner-only synthetic marker"}]}}]],
    checkpoint:{id:"fixture-checkpoint",payload:{sourceRevision:1,blobs:[digest]}},
  }),"commit")).json;
  assert.equal(result.stored,2);
  const routes=[base,href,...["/records","/records/2/details","/checkpoints","/blobs/"+digest,"/download","/batches","/effects","/lease"].map(suffix=>href+suffix)];
  for (const route of routes) {
    await ok(request(route,"GET"),"positive owner access");
    for (const input of [
      {cookie:bob.cookie},
      {cookie:appCookie},
      {headers:{authorization:"Bearer "+token}},
    ]) {
      const denied=await fixture.request(route,input);
      assert.ok([401,403,404].includes(denied.status),route+" gave unauthorized status "+denied.status);
      assert.ok(!denied.text.includes("owner-only synthetic marker"));
    }
  }
  console.log("Real loopback Konae: positive owner access and denial for another owner, a live app-account cookie, and a delegated token that can read this Project. "+routes.length+" routes per principal.");
} finally {
  await fixture.close();
}
