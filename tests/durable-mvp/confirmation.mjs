import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const repo = process.argv[2];
const { createPlatformTools } = await import(pathToFileURL(join(repo, "src/agent/platform-tools.ts")).href);
const fixtureProject = "confirmation-fixture";
const effectTool = {
  name:"module_set",
  inputSchema:{type:"object",properties:{projectId:{type:"string"},module:{type:"string"},on:{type:"boolean"}},required:["projectId","module","on"]},
};

async function testSetup() {
  let current = true;
  let publish = true;
  let failReadback = "";
  let opened;
  let decide;
  let inspections=0;
  const openedPromise = new Promise((resolve) => { opened=resolve; });
  const decision = new Promise((resolve) => { decide=resolve; });
  const calls = [];
  const binding = {
    async listTools() { return [effectTool]; },
    async call(name) {
      if (name==="session_inspect") { inspections++; return { can:{read:true,write:true,manage_users:true,publish}, projectId:fixtureProject }; }
      if (name==="platform_guide") return {readBack:{map:{module_set:"modules_read"}}};
      throw Error("The staged tool should carry execution metadata");
    },
    async callAgent(name,args,id,_signal,observes) {
      if (name==="session_inspect") { inspections++; return { can:{read:true,write:true,manage_users:true,publish}, projectId:fixtureProject }; }
      if (name==="platform_guide") return {readBack:{map:{module_set:"modules_read"}}};
      calls.push({ name, args, id, observes });
      if (name==="module_set") return {writeAccepted:true};
      if (name==="modules_read") {
        if (failReadback==="sign out") throw Object.assign(new Error("The fixture owner session is no longer present"),{status:401});
        if (failReadback) throw new TypeError("fixture: response cannot be observed");
        return { modules:{ seo:{on:true} } };
      }
      throw Error("Unexpected observed tool "+name);
    },
    deploymentForProject(id) { return id===fixtureProject ? "fixture.example.test" : null; },
    async confirm(record) { opened(record); return decision; },
  };
  const [tool]=await createPlatformTools({projectId:fixtureProject,isCurrent:()=>current,binding,refreshSession:true});
  const run=()=>tool.execute("fixture-confirmed-call",{module:"seo",on:true});
  function prepare() {
    const reply=run().then(value=>({value}),error=>({error}));
    return {reply,opened:openedPromise};
  }
  return { calls, prepare, decide, inspections:()=>inspections, changeProject:()=>{current=false;}, revoke:()=>{publish=false;}, loseReadback:(cause)=>{failReadback=cause;} };
}

for (const policy of ["decline","project changed","publisher revoked","observed","unknown","sign out"]) {
  const test=await testSetup();
  const {reply,opened}=test.prepare();
  const confirmation=await opened;
  assert.equal(confirmation.projectId,fixtureProject);
  assert.equal(confirmation.tool,"module_set");
  if (policy==="project changed") test.changeProject();
  if (policy==="publisher revoked") test.revoke();
  if (policy==="unknown" || policy==="sign out") test.loseReadback(policy);
  test.decide(policy!=="decline");
  const outcome=await reply;
  if (policy==="decline" || policy==="project changed" || policy==="publisher revoked") {
    assert.equal(test.calls.length,0,policy+" authorizes no mutation");
    assert.ok(outcome.error,policy+" results in refusal");
    if (policy==="publisher revoked") assert.ok(test.inspections()>=2,"Permission is re-read after the confirmation opens");
  } else {
    assert.deepEqual(test.calls,[{name:"module_set",args:{module:"seo",on:true,projectId:fixtureProject},id:"fixture-confirmed-call",observes:undefined},
      {name:"modules_read",args:{projectId:fixtureProject},id:test.calls[1]?.id,observes:"fixture-confirmed-call"}]);
    const outcomeText=outcome.error ? outcome.error.message : JSON.stringify(outcome.value);
    if (policy==="unknown" || policy==="sign out") {
      assert.match(outcomeText,/unknown|unobserved|could not confirm/i);
      assert.doesNotMatch(outcomeText,/succeeded|nothing was changed/i);
    } else {
      assert.ok(!outcome.error,outcomeText);
      assert.match(outcomeText,/seo/);
    }
  }
  console.log("Confirmed fixture case:",policy);
}
