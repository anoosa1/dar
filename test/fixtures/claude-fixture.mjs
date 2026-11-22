#!/usr/bin/env node
// CLI fixture only: the stream-json control protocol shapes Dar uses from `claude -p`.
import assert from "node:assert/strict";
import {readFile,writeFile,access} from "node:fs/promises";
import {randomUUID} from "node:crypto";
import path from "node:path";
const args=process.argv.slice(2),send=value=>process.stdout.write(JSON.stringify(value)+"\n"),home=process.env.CLAUDE_CONFIG_DIR;
assert.ok(home);assert.equal(process.env.ANTHROPIC_API_KEY,undefined);assert.equal(process.env.DAR_DATABASE_TOKEN_FILE,undefined);assert.equal(process.env.HOME,home);
const option=name=>{const index=args.indexOf(name);return index<0?undefined:args[index+1];};
if(args[0]==="auth"&&args[1]==="status"){
 const loggedOut=await access(path.join(home,"logged-out")).then(()=>true,()=>false);
 send({loggedIn:!loggedOut,authMethod:"claude.ai",apiProvider:"firstParty",email:"fixture@example.test",subscriptionType:"max"});if(loggedOut)process.exitCode=1;
}else{
 for(const flag of ["-p","--restricted","--strict-mcp-config","--disable-slash-commands","--verbose"])assert.ok(args.includes(flag),flag);
 assert.equal(option("--input-format"),"stream-json");assert.equal(option("--output-format"),"stream-json");assert.equal(option("--permission-prompt-tool"),"stdio");
 const tools=(option("--tools")??"").split(",").filter(Boolean),settings=JSON.parse(option("--settings")??"{}");
 for(const tool of tools)assert.ok(["Read","Edit","Write","Glob","Grep","TodoWrite","AskUserQuestion","Bash"].includes(tool),tool);
 if(tools.includes("Bash")){assert.equal(settings.sandbox.failIfUnavailable,true);assert.equal(settings.sandbox.allowUnsandboxedCommands,false);assert.deepEqual(settings.sandbox.network.allowedDomains,[]);}
 const sessionsFile=path.join(home,"fixture-sessions.json"),sessions=JSON.parse(await readFile(sessionsFile,"utf8").catch(()=>"{}")),save=()=>writeFile(sessionsFile,JSON.stringify(sessions));
 const resume=option("--resume");if(resume&&!sessions[resume]){process.stderr.write("No conversation found\n");process.exit(1);}
 const session=resume??randomUUID();sessions[session]??=[];
 const replies=new Map(),wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));let interrupted,steered,announced=false,message=0;
 const request=request=>new Promise(resolve=>{const id=randomUUID();replies.set(id,resolve);send({type:"control_request",request_id:id,request});});
 const assistant=content=>send({type:"assistant",message:{id:"msg_"+ ++message,role:"assistant",content,usage:{input_tokens:3,cache_read_input_tokens:4,cache_creation_input_tokens:5,output_tokens:6}},parent_tool_use_id:null,session_id:session});
 const result=(text,error)=>send({type:"result",subtype:error?"error_during_execution":"success",is_error:!!error,result:error??text,session_id:session,usage:{input_tokens:10,cache_read_input_tokens:4,cache_creation_input_tokens:1,output_tokens:5},modelUsage:{"fixture-model":{contextWindow:200000}}});
 const reply=text=>{send({type:"stream_event",event:{type:"message_start",message:{id:"msg_"+(message+1)}},parent_tool_use_id:null,session_id:session});send({type:"stream_event",event:{type:"content_block_start",index:0,content_block:{type:"text",text:""}},parent_tool_use_id:null,session_id:session});send({type:"stream_event",event:{type:"content_block_delta",index:0,delta:{type:"text_delta",text:text.slice(0,4)}},parent_tool_use_id:null,session_id:session});assistant([{type:"text",text}]);};
 const toolResult=(id,content,error=false)=>send({type:"user",message:{role:"user",content:[{type:"tool_result",tool_use_id:id,content,is_error:error}]},parent_tool_use_id:null,session_id:session});
 async function turn(content){
  const text=content.find(item=>item.type==="text")?.text??"",images=content.filter(item=>item.type==="image");
  sessions[session].push(text);await save();
  if(!announced){announced=true;send({type:"system",subtype:"init",session_id:session,cwd:process.cwd(),tools,model:option("--model"),permissionMode:option("--permission-mode")});}
  if(text.includes("WAIT_FOR_CANCEL")){assistant([{type:"tool_use",id:"toolu_wait",name:"Grep",input:{pattern:"fixture waiting for cancellation"}}]);await new Promise(resolve=>{interrupted=resolve;});result("","Interrupted");return;}
  if(text.includes("FAIL_CLI")){result("","Fixture failure");return;}
  if(text.includes("EXIT_WITHOUT_RESULT")){reply("Unconfirmed response");await wait(50);process.exit(0);}
  if(text.includes("ASK_QUESTION")){const input={questions:[{question:"Which color should the greeting use?",header:"Color",multiSelect:false,options:[{label:"Blue",description:"Calm"},{label:"Red",description:"Bold"}]}]};assistant([{type:"tool_use",id:"toolu_ask",name:"AskUserQuestion",input}]);const decision=await request({subtype:"can_use_tool",tool_name:"AskUserQuestion",input,tool_use_id:"toolu_ask"});assert.equal(decision.behavior,"allow");toolResult("toolu_ask","answered");reply(`You chose ${decision.updatedInput.answers["Which color should the greeting use?"]}.`);result("done");return;}
  if(text.includes("REQUEST_NETWORK")){const decision=await request({subtype:"can_use_tool",tool_name:"SandboxNetworkAccess",input:{host:"example.test"},tool_use_id:randomUUID()});assert.equal(decision.behavior,"deny");reply("Network was declined.");result("done");return;}
  if(text.includes("WAIT_FOR_STEER")){assistant([{type:"tool_use",id:"toolu_steer",name:"Grep",input:{pattern:"fixture waiting for guidance"}}]);const guidance=await new Promise(resolve=>{steered=resolve;});toolResult("toolu_steer","found");reply(`Steered: ${guidance}`);result("done");return;}
  if(text.includes("PLAN_ONLY")){assert.equal(option("--permission-mode"),"plan");const decision=await request({subtype:"can_use_tool",tool_name:"ExitPlanMode",input:{plan:"1. Read greeting\n2. Update greeting"},tool_use_id:"toolu_plan"});assert.equal(decision.behavior,"deny");reply("Planned without editing.");result("done");return;}
  if(text.includes("DESCRIBE_IMAGE")){for(const image of images)assert.equal(Buffer.from(image.source.data,"base64").subarray(1,4).toString(),"PNG");reply(`Saw ${images.length} image${images.length===1?"":"s"}.`);result("done");return;}
  if(text.includes("Review the uncommitted changes")){assert.equal(option("--permission-mode"),"plan");assert.ok(text.includes("+Updated by Claude fixture."));reply("Review: greeting.txt changed; no issues found.");result("done");return;}
  if(text.includes("RESUME_CHECK")){reply(`Turns: ${sessions[session].length}. Input: ${text.replace(/\s+/g," ").trim()}`);result("done");return;}
  assert.equal(option("--permission-mode"),"acceptEdits");
  const file=path.join(process.cwd(),"greeting.txt"),before=await readFile(file,"utf8");
  assistant([{type:"thinking",thinking:"Checking the greeting file before editing."}]);
  assistant([{type:"tool_use",id:"toolu_todo",name:"TodoWrite",input:{todos:[{content:"Read greeting",status:"completed"},{content:"Update greeting",status:"pending"}]}}]);
  assistant([{type:"tool_use",id:"toolu_edit",name:"Write",input:{file_path:file,content:"Updated by Claude fixture.\n"}}]);
  await writeFile(file,before.includes("Updated")?"Updated again by Claude fixture.\n":"Updated by Claude fixture.\n");toolResult("toolu_edit","File written");
  assistant([{type:"tool_use",id:"toolu_read",name:"Read",input:{file_path:"/etc/hostname"}}]);toolResult("toolu_read","/etc/hostname is outside the working directory.",true);
  reply("The edit is ready.");result("The edit is ready.");
 }
 process.stdin.setEncoding("utf8");let buffer="",queue=Promise.resolve(),running=false;
 for await(const chunk of process.stdin){
  buffer+=chunk;let index;
  while((index=buffer.indexOf("\n"))>=0){
   const line=buffer.slice(0,index);buffer=buffer.slice(index+1);if(!line.trim())continue;const value=JSON.parse(line);
   if(value.type==="control_response"){replies.get(value.response.request_id)?.(value.response.response);replies.delete(value.response.request_id);continue;}
   if(value.type==="control_request"){
    const subtype=value.request.subtype,ok=response=>send({type:"control_response",response:{subtype:"success",request_id:value.request_id,response}});
    if(subtype==="initialize")ok({models:[{value:"default",resolvedModel:"fixture-model",displayName:"Default (recommended)"},{value:"fixture",resolvedModel:"fixture-model",displayName:"Fixture Claude",supportedEffortLevels:["low","high"]},{value:"bad model;",displayName:"Rejected"}],account:{email:"fixture@example.test"}});
    else if(subtype==="get_usage")ok({subscription_type:"max",rate_limits_available:true,rate_limits:{limits:[{kind:"session",group:"session",percent:42,resets_at:"2030-03-17T17:46:40Z"},{kind:"weekly_all",group:"weekly",percent:7,resets_at:"2030-03-23T08:00:00Z"}]}});
    else if(subtype==="interrupt"){ok({});interrupted?.();}
    else send({type:"control_response",response:{subtype:"error",request_id:value.request_id,error:"unsupported"}});
    continue;
   }
   if(value.type==="user"){
    const content=value.message.content;
    // A message arriving mid-turn steers it, as the CLI does with queued input.
    if(running&&steered){const resolve=steered;steered=undefined;resolve(content.find(item=>item.type==="text")?.text??"");continue;}
    running=true;queue=queue.then(()=>turn(content)).then(()=>{running=false;},error=>{process.stderr.write(String(error?.stack??error));process.exit(3);});
   }
  }
 }
 await queue;
}
