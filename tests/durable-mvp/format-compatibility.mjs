import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { Database } from "bun:sqlite";
import { testInstallation } from "./harness.mjs";

const root = await mkdtemp(path.join(tmpdir(), "konae-e2e-format-"));
const install = await testInstallation(process.argv[2], { root });
const saved = await Bun.file(new URL("./fixtures/native.json", import.meta.url)).json();
const versionCases = [
  {formatVersion:2,piVersion:"1.5.2",registryVersion:"1", label:"unknown Konae envelope"},
  {formatVersion:1,piVersion:"2.0.0",registryVersion:"1", label:"unknown Pi schema"},
  {formatVersion:1,piVersion:"1.5.2",registryVersion:"2", label:"unknown tool registry"},
];
const original = {formatVersion:1,piVersion:"1.5.2",registryVersion:"1"};
const projectId = "fixture-format-compatibility";
const id = "pinned-native-format-v1";
const projectPath = "/_konae/projects/" + projectId + "/conversations";
const href = projectPath + "/" + id;
const executorId = "fixed-format-fixture-executor";
let db;
try {
  const one = await install.owner("format-one");
  const two = await install.owner("format-two");
  await install.mcp(one.cookie,"project_create",{projectId,name:"Upgrade fixture"});
  const fetch = (endpoint,method="GET",body,cookie=one.cookie)=>install.request(endpoint,{method,body,cookie});
  const ok = async (response, label)=>{
    const result=await response;
    assert.ok(result.status>=200 && result.status<300,`${label} returned ${result.status}`);
    return result.json;
  };
  await ok(fetch(projectPath,"POST",{id,title:"First persisted format fixture",...original}),"create format-1 envelope");
  const claim={expectedGeneration:0,executorId,continueExplicitly:true};
  const lease=await ok(fetch(href+"/claim","POST",claim),"claim supported format");
  const receipt=await ok(fetch(href+"/commit","POST",{requestId:"stable-fixture-native-batch",generation:lease.generation,expectedSeq:1,writes:saved.batches,checkpoint:{id:"frozen-checkpoint",payload:{sourceRevision:1,blobs:[]}}}),"commit native fixture");
  assert.equal(receipt.stored,saved.batches.length);
  assert.equal(receipt.checkpointStored,true);
  await ok(fetch(href+"/release","POST",{generation:lease.generation,executorId}),"release prior execution");
  const exactHistory = await ok(fetch(href+"/download"),"download format 1");
  assert.deepEqual(exactHistory.records,saved.batches.flat());
  assert.equal(exactHistory.checkpoints[0].checkpointId,"frozen-checkpoint");
  assert.equal(exactHistory.checkpoints[0].sequence,receipt.lastSeq);
  const before=await ok(fetch(href+"/batches"),"exact native replay");
  assert.deepEqual(before.batches.map(batch=>batch.writes),saved.batches);
  db=new Database(path.join(root,"mcp.sqlite"));

  for (const {label,formatVersion,piVersion,registryVersion} of versionCases) {
    db.prepare("UPDATE conversations SET format_version=?,pi_version=?,registry_version=? WHERE id=?").run(formatVersion,piVersion,registryVersion,id);
    db.prepare("UPDATE conversation_records SET format_version=? WHERE conversation=?").run(formatVersion,id);
    const envelope=await ok(fetch(href),label+" envelope read");
    assert.equal(envelope.formatVersion,formatVersion);
    assert.equal(envelope.piVersion,piVersion);
    assert.equal(envelope.registryVersion,registryVersion);
    const display=await ok(fetch(href+"/records"),label+" timeline read");
    assert.ok(display.items.length>0);
    assert.ok(display.items.every(record=>record.formatVersion===formatVersion));
    const downloaded=await ok(fetch(href+"/download"),label+" download");
    assert.deepEqual(downloaded.records,exactHistory.records);
    assert.deepEqual(downloaded.checkpoints,exactHistory.checkpoints);
    const denied=await fetch(href+"/claim","POST",{...claim,expectedGeneration:lease.generation});
    assert.equal(denied.status,409,label);
    assert.match(denied.text,/readable but cannot be resumed/i);
    assert.equal((await ok(fetch(href+"/lease"),label+" no activation")).live,false);
    assert.ok([403,404].includes((await fetch(href+"/download","GET",undefined,two.cookie)).status));
    console.log(label+": latest timeline and download preserve raw records; fresh execution refused; cross-owner denied.");
  }
} finally {
  db?.close();
  await install.close();
}
