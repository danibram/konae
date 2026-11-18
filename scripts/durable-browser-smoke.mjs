import assert from "node:assert/strict";
import { chromium, webkit, devices } from "playwright";

const repo = process.argv[2];
const config = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response() });
const port = config.port;
config.stop(true);
const origin = `http://127.0.0.1:${port}`;
const vite = Bun.spawn([process.execPath, "run", "dev", "--", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],{cwd:repo,stdout:"ignore",stderr:"inherit"});

function event(type,data) {
  return `event: ${type}\ndata: ${JSON.stringify(data)}\n\n`;
}
function response(step, tool, lastText) {
  const events = [event("message_start", { type:"message_start", message:{id:`fixture-msg-${step}`,type:"message",role:"assistant",model:"claude-fixture",content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:1,output_tokens:0}} })];
  let index = 0;
  if(step===0) {
    events.push(event("content_block_start",{type:"content_block_start",index,content_block:{type:"thinking",thinking:""}}));
    events.push(event("content_block_delta",{type:"content_block_delta",index,delta:{type:"thinking_delta",thinking:"PRIVATE-REASONING-DO-NOT-PERSIST"}}));
    events.push(event("content_block_delta",{type:"content_block_delta",index,delta:{type:"signature_delta",signature:"fixture-opaque-continuation"}}));
    events.push(event("content_block_stop",{type:"content_block_stop",index}));
    index++;
  }
  events.push(event("content_block_start",{type:"content_block_start",index,content_block: tool
    ? {type:"tool_use", id:`fixture-tool-${step}`,name:tool.name,input:{}}
    : {type:"text",text:""}
  }));
  events.push(event("content_block_delta",{type:"content_block_delta",index,delta: tool
    ? {type:"input_json_delta",partial_json:JSON.stringify(tool.input)}
    : {type:"text_delta",text:lastText}
  }));
  events.push(event("content_block_stop",{type:"content_block_stop",index}));
  events.push(event("message_delta",{type:"message_delta",delta:{stop_reason:tool?"tool_use":"end_turn",stop_sequence:null},usage:{output_tokens:2}}));
  events.push(event("message_stop",{type:"message_stop"}));
  return events.join("");
}
const providerKey="explicit-test-key-5fdccb25";
const fileText = "<main>ORIGINAL-START" + "x".repeat(26050) + "ORIGINAL-END</main>";
const candidate = "<main><h1>Fixture candidate</h1></main>";
const failures = [];
try {
  const deadline = Date.now()+60_000;
  while(Date.now()<deadline){
    if(await fetch(origin).then((res)=>res.ok,()=>false)) break;
    if(vite.exitCode!==null) throw Error("Vite exited during local smoke test startup");
    await Bun.sleep(100);
  }
  for (const [engine, device] of [[chromium, {}], [webkit,devices["iPhone 13"]]]) {
    const browser = await engine.launch();
    try {
      const context = await browser.newContext(device);
      await context.addCookies([{ name: "owner-session-fixture", value: "present", url: origin }]);
      await context.route(origin+"/__gate28_probe",route=>route.fulfill({contentType:"text/html",body:"<!doctype html><title>Local durable fixture</title>"}));
      const ownerRequests = [];
      await context.route(origin+"/_konae/**", async route=>{ ownerRequests.push(route.request().url()); await route.fulfill({ status: 401, body: "not used" }); });
      await context.route(origin+"/mcp", async route=>{ ownerRequests.push(route.request().url()); await route.fulfill({ status: 401, body: "not used" }); });
      const requests = [];
      await context.route("https://api.anthropic.com/**",async route=>{
        const req = route.request();
        const headers = {"access-control-allow-origin":origin,"access-control-allow-headers":"*","access-control-allow-methods":"POST,OPTIONS","content-type":"text/event-stream"};
        if(req.method()==="OPTIONS") return route.fulfill({status:204,headers});
        const body=req.postDataJSON();
        const index=requests.length;
        requests.push({body,authorized:req.headers()["x-api-key"]===providerKey});
        const tools=[
          {name:"read",input:{path:"/workspace/index.html"}},
          {name:"write",input:{path:"/workspace/index.html",content:candidate}},
          {name:"validate_capsule",input:{}},
        ];
        const lifecycleTool = index===4 || index===6 ? {name:"session_inspect",input:{}} : undefined;
        return route.fulfill({status:200,headers,body:response(index,tools[index] ?? lifecycleTool,`Fixture reply includes ${providerKey}.`)});
      });
      await context.route("https://api.openai.com/**", async route=>{
        const req=route.request();
        const headers={"access-control-allow-origin":origin,"access-control-allow-headers":"*","access-control-allow-methods":"POST,OPTIONS","content-type":"text/event-stream"};
        if(req.method()==="OPTIONS") return route.fulfill({status:204,headers});
        const body=req.postDataJSON();
        requests.push({body,authorized:req.headers().authorization==="Bearer fixture-openai-key"});
        const events=[
          {type:"response.created",response:{id:"openai-lifecycle",object:"response",created_at:1,status:"in_progress",model:body.model,output:[],usage:null}},
          {type:"response.output_item.added",output_index:0,item:{id:"openai-message",type:"message",role:"assistant",content:[]}},
          {type:"response.content_part.added",item_id:"openai-message",output_index:0,content_index:0,part:{type:"output_text",text:""}},
          {type:"response.output_text.delta",item_id:"openai-message",output_index:0,content_index:0,delta:"provider changed on the live handle"},
          {type:"response.output_text.done",item_id:"openai-message",output_index:0,content_index:0,text:"provider changed on the live handle"},
          {type:"response.content_part.done",item_id:"openai-message",output_index:0,content_index:0,part:{type:"output_text",text:"provider changed on the live handle"}},
          {type:"response.output_item.done",output_index:0,item:{id:"openai-message",type:"message",role:"assistant",content:[{type:"output_text",text:"provider changed on the live handle",annotations:[]}]}},
          {type:"response.completed",response:{id:"openai-lifecycle",object:"response",created_at:1,status:"completed",model:body.model,output:[{id:"openai-message",type:"message",role:"assistant",content:[{type:"output_text",text:"provider changed on the live handle",annotations:[]}]}],usage:{input_tokens:1,output_tokens:4,total_tokens:5}}},
        ].map(data=>`event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`).join("");
        return route.fulfill({status:200,headers,body:events});
      });
      const page = await context.newPage();
      await page.goto(origin+"/__gate28_probe");
      assert.equal(requests.length,0);
      const result = await page.evaluate(async({secret,source})=>{
        const [{openLocalDurableEngine,createDurableKonaeAgent},{starterProject},{BACKGROUND_CONTEXT}] = await Promise.all([
          import("/src/agent/durable-engine.ts"), import("/src/lib/starter.ts"), import("/node_modules/@earendil-works/chord/dist/context/index.js"),
        ]);
        const project={...starterProject,files:{...starterProject.files,"index.html":source}};
        const keyStore={getKey:()=>secret,setKey(){},clearKey(){}};
        let validation;
        const handle=await openLocalDurableEngine({project,keyStore,sessionConfig:{provider:"anthropic",modelId:"fixture-unqualified-custom"},localOnly:true,onValidation(result){validation=result}});
        try{
          const settled = await handle.send("Read and revise the fixture.");
          const transcript=await handle.conversation.context(BACKGROUND_CONTEXT);
          const adapterKeys={anthropic:secret,openai:"fixture-openai-key"};
          const adapterKeyStore={getKey:(provider)=>adapterKeys[provider]??null,setKey(){},clearKey(){}};
          let currentChecks=0;
          let platformCalls=0;
          const binding={
            async listTools(){return [{name:"session_inspect",inputSchema:{type:"object",properties:{projectId:{type:"string"}}}}]},
            async call(name,args){if(name!=="session_inspect"||args.projectId!==project.id)throw new Error("platform arguments were not pinned to the current project");platformCalls++;return {can:{read:true}}},
            deploymentForProject(){return null},async confirm(){return false},
          };
          const agent=createDurableKonaeAgent(project,{provider:"anthropic",modelId:"fixture-adapter-custom"},adapterKeyStore,{isCurrent(){currentChecks++;return true},platform:binding});
          const ui=[]; agent.subscribe(event=>ui.push(event));
          await agent.send("Read the current platform session.");
          const firstTurns=ui.filter(event=>event.type==="turn-end").length;
          await agent.send("Read the current platform session again.");
          const twoTurnCount=ui.filter(event=>event.type==="turn-end").length;
          agent.configure({provider:"openai",modelId:"fixture-openai-unqualified"});
          await agent.send("Use the newly selected provider and model.");
          const adapterFacts={firstTurns,twoTurnCount,platformCalls,currentChecks,thirdTurnCount:ui.filter(event=>event.type==="turn-end").length};
          agent.dispose();
          let releaseInitialization;
          let markInitializationStarted;
          const initializationStarted=new Promise(resolve=>markInitializationStarted=resolve);
          const waitingBinding={...binding,listTools(){markInitializationStarted();return new Promise(resolve=>releaseInitialization=()=>resolve([]))}};
          const interrupted=createDurableKonaeAgent(project,{provider:"anthropic",modelId:"fixture-interrupted-custom"},adapterKeyStore,{isCurrent(){return true},platform:waitingBinding});
          const interruptedSend=interrupted.send("This initialization will be interrupted.");
          await initializationStarted;
          interrupted.interrupt();
          releaseInitialization();
          await interruptedSend;
          interrupted.dispose();
          adapterFacts.interruptedWhileOpening=true;
          return {settled,transcript,proposal:{...handle.proposal(),validation},project,adapterFacts};
        } finally { await handle.close(); }
      }, {secret:providerKey,source:fileText});
      assert.equal(result.settled.status,"done");
      assert.equal(result.adapterFacts.twoTurnCount,2,"the same agent remains usable after the first turn");
      assert.equal(result.adapterFacts.thirdTurnCount,3,"the live agent accepts a provider/model change");
      assert.equal(result.adapterFacts.platformCalls,2,"the same current project binding works on both turns");
      assert.ok(result.adapterFacts.currentChecks>=4,"the project/session current predicate is checked at both tool boundaries");
      assert.ok(requests.some(({body,authorized})=>authorized&&body.model==="fixture-openai-unqualified"),"the changed provider and unqualified model reach the live request");
      assert.equal(requests.length,9,"interrupting initialization prevents its provider request");
      assert.equal(result.proposal.files["index.html"],candidate);
      assert.equal(requests.length,9);
      assert.ok(requests.slice(0,4).every(({authorized,body})=>authorized && body.model==="fixture-unqualified-custom"));
      const persisted=JSON.stringify(result.transcript);
      function check(condition, name) {
        if (!condition) failures.push(engine.name()+": "+name);
      }
      check(!persisted.includes(providerKey),"Provider echo must not persist the active key");
      check(!persisted.includes("PRIVATE-REASONING-DO-NOT-PERSIST"),"Unexpected reasoning must never reach the transcript");
      check(!persisted.includes("fixture-opaque-continuation"),"Opaque provider continuation stays absent in MVP");
      const nextRequest=JSON.stringify(requests[1].body);
      check(!nextRequest.includes("PRIVATE-REASONING-DO-NOT-PERSIST"),"Reasoning absent from next provider request");
      const resultBlock=requests[1].body.messages.flatMap((record)=>Array.isArray(record.content)?record.content:[]).find((block)=>block.type==="tool_result");
      const stored=result.transcript.messages.find((message)=>message.role==="toolResult"&&message.toolName==="read");
      assert.ok(stored,"Tool result persisted into Durable context");
      assert.ok(stored.content.some(({text})=>text?.includes("ORIGINAL-START")));
      assert.ok(!stored.content.some(({text})=>text?.includes("ORIGINAL-END")));
      assert.ok(JSON.stringify(stored.details).includes("ORIGINAL-END"),"Full original text was preserved separately");
      assert.ok(resultBlock,"Actual next Anthropic request carries the result");
      const visible=typeof resultBlock.content==="string"?resultBlock.content:resultBlock.content.map(({text})=>text).join("\n");
      const expected=stored.content.map(({text})=>text).join("\n");
      assert.equal(visible,expected);
      const builtPreview = await page.evaluate(async ({proposal, baseProject}) => {
        const [{proposalPreviewProject},{compileProjectInBrowser},{createPreviewDocument}] = await Promise.all([
          import("/src/components/proposal-preview.ts"),
          import("/src/runtime/browser-build.ts"),
          import("/src/runtime/preview.ts"),
        ]);
        const proposed = proposalPreviewProject(baseProject, proposal);
        if (!proposed) throw new Error("The local proposal could not be applied to the fixture project.");
        const compiled = await compileProjectInBrowser(proposed);
        const built = { ...proposed, files: { ...proposed.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script } };
        const frame = document.createElement("iframe");
        frame.title = "Credential-free local Gate preview";
        frame.srcdoc = createPreviewDocument(built);
        document.body.append(frame);
        await new Promise((resolve, reject) => { frame.addEventListener("load", resolve, { once: true }); setTimeout(() => reject(new Error("local preview timed out")), 10_000); });
        return { heading: frame.contentDocument?.querySelector("h1")?.textContent, ownerCalls: performance.getEntriesByType("resource").filter((entry) => /\/_konae\/|\/mcp(?:\?|$)/.test(entry.name)).length };
      }, { proposal: result.proposal, baseProject: { ...result.project, files: { ...result.project.files, "index.html": fileText } } });
      assert.equal(builtPreview.heading, "Fixture candidate");
      assert.equal(builtPreview.ownerCalls, 0);
      assert.deepEqual(ownerRequests, []);
      console.log(engine.name(),browser.version(),"real browser Durable tool preview, local key and wire equality passed");
    } finally { await browser.close(); }
  }
  assert.deepEqual(failures,[]);
} finally { vite.kill(); await vite.exited; }
